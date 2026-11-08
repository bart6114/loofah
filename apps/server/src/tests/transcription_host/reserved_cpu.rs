use super::*;
use futures_util::FutureExt;
use hypr_loofah_core::capture_live::{CaptureLiveEngineSpec, CaptureLiveIngress};
use hypr_loofah_protocol::{CaptureFormat, CaptureLiveRequest, TranscriptionAvailability};
use std::{io::Cursor, panic::AssertUnwindSafe, time::Instant};

struct LiveSource {
    _audio: Arc<[f32]>,
    released: Arc<AtomicBool>,
}
impl Drop for LiveSource {
    fn drop(&mut self) {
        self.released.store(true, Ordering::Release);
    }
}

#[tokio::test]
#[ignore = "requires provisioned LOOFAH_ONNX_MODELS; real reserved CPU overlap on the configured service"]
async fn real_reserved_cpu_delivers_live_caption_while_batch_owns_model_and_http_stays_responsive()
{
    let models = std::path::PathBuf::from(std::env::var("LOOFAH_ONNX_MODELS").unwrap());
    let snapshots = tempfile::tempdir().unwrap();
    let config = serde_json::from_value(json!({
        "model":"operator-base", "model_path":models.join("batch"),
        "workload_groups":2, "live_reserved_groups":1,
        "onnx":{"streaming_model_path":models.join("streaming"),
            "diarizer_model_path":models.join("diarizer"), "snapshot_root":snapshots.path()}
    }))
    .unwrap();
    let fixture = Fixture::with_transcription(None, None, Some(config)).await;
    let result = AssertUnwindSafe(async {
        let host = fixture.service.transcription_host_for_test().unwrap();
        tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let availability = host.policy("operator-base").availability;
                if availability == TranscriptionAvailability::Ready { break; }
                assert_eq!(availability, TranscriptionAvailability::Loading);
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }).await.unwrap();
        let me = fixture.login("alice").await;
        let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
        let short = speech_wav();
        let mut reader = hound::WavReader::new(Cursor::new(&short)).unwrap();
        let spec = reader.spec();
        assert_eq!((spec.sample_rate, spec.channels), (16_000, 2));
        let samples: Arc<[f32]> = reader.samples::<f32>().map(Result::unwrap).collect::<Vec<_>>().into();
        let mut long = Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut long, spec).unwrap();
            for _ in 0..16 { for sample in samples.iter() { writer.write_sample(*sample).unwrap(); } }
            writer.finalize().unwrap();
        }
        let audio = long.into_inner();
        let directory = seed(&context, &audio).await;
        let metadata = std::fs::read(directory.join("_meta.json")).unwrap();
        let note = std::fs::read(directory.join("notes.md")).unwrap();
        let batch = context.submit_transcription_job(me.member.id.clone(), uuid::Uuid::new_v4().to_string(), request(&audio)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(180), async {
            while !host.batch_inference_active_for_test() {
                assert!(!context.transcription_job(me.member.id.clone(), batch.id.clone()).await.unwrap().state.is_terminal());
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.unwrap();
        assert_eq!(host.policy("operator-base").availability, TranscriptionAvailability::Ready);
        let released = Arc::new(AtomicBool::new(false));
        let started = Instant::now();
        let mut live = tokio::time::timeout(Duration::from_secs(2), host.live_engine().start(
            CaptureLiveEngineSpec {
                generation_id: uuid::Uuid::new_v4().to_string(), format: CaptureFormat::stereo(),
                request: CaptureLiveRequest { capture_fingerprint:digest(&short), expected_transcript:AbsentTranscriptExpectation::Absent,
                    model:"operator-base".into(), languages:vec!["en".into()], keywords:vec![] },
            }, Box::new(LiveSource { _audio:samples.clone(), released:released.clone() }),
        )).await.unwrap().unwrap();
        let control = live.control.clone();
        let feed_audio = samples.clone();
        let feeder = tokio::spawn(async move {
            let mut frame = 0;
            for chunk in feed_audio.chunks(3200) {
                let pcm: Arc<[f32]> = chunk.into();
                loop {
                    match control.try_accept(frame, &pcm).unwrap() {
                        CaptureLiveIngress::Accepted => break,
                        CaptureLiveIngress::Busy => tokio::time::sleep(Duration::from_millis(5)).await,
                        other => panic!("unexpected ingress: {other:?}"),
                    }
                }
                frame += chunk.len() as u64 / 2;
            }
            control.finish(frame).unwrap();
        });
        tokio::time::timeout(Duration::from_secs(180), async {
            loop {
                let caption = live.captions.recv().await.expect("live owner ended without speech");
                if !caption.new_words.is_empty() || !caption.partials.is_empty() { break; }
            }
        }).await.unwrap();
        let caption_elapsed = started.elapsed();
        assert!(host.batch_inference_active_for_test(), "caption must precede actual batch model release");
        assert!(!released.load(Ordering::Acquire));
        let http_started = Instant::now();
        let health = tokio::time::timeout(Duration::from_secs(2), fixture.app.clone().oneshot(
            Request::builder().uri("/health").body(Body::empty()).unwrap(),
        )).await.unwrap().unwrap();
        assert_eq!(health.status(), StatusCode::OK);
        let (status, _) = tokio::time::timeout(Duration::from_secs(2), fixture.call("alice", "GET", &format!("/api/v1/vaults/{}/sessions/speech/metadata",me.personal_vault_id),None)).await.unwrap();
        assert_eq!(status, StatusCode::OK);
        let http_elapsed = http_started.elapsed();
        assert!(host.batch_inference_active_for_test());
        assert!(!directory.join("transcript.json").exists());
        let drain_started = Instant::now();
        context.cancel_transcription_job(me.member.id.clone(),batch.id.clone()).await.unwrap();
        let done = terminal(&context,&me.member.id,&batch.id,Duration::from_secs(180)).await;
        assert_eq!(done.state,TranscriptionJobState::Cancelled,"{done:?}");
        tokio::time::timeout(Duration::from_secs(180), async {
            while live.captions.recv().await.is_some() {}
            while live.completion.borrow().is_none() { live.completion.changed().await.unwrap(); }
            feeder.await.unwrap();
        }).await.unwrap();
        let receipt = live.completion.borrow().clone().unwrap();
        assert!(receipt.native_joined && receipt.owners_released);
        assert_eq!(receipt.failure,None);
        assert_eq!(receipt.completed_through_frame,Some(samples.len() as u64/2));
        assert!(released.load(Ordering::Acquire));
        assert!(!host.batch_inference_active_for_test());
        assert_eq!(std::fs::read(directory.join("audio.wav")).unwrap(),audio);
        assert_eq!(std::fs::read(directory.join("_meta.json")).unwrap(),metadata);
        assert_eq!(std::fs::read(directory.join("notes.md")).unwrap(),note);
        assert_eq!(std::fs::read(directory.join("unknown-user-file.txt")).unwrap(),b"Private user attachment");
        assert!(!directory.join("transcript.json").exists());
        eprintln!("reserved CPU acceptance: total=2 reserved_live=1 batch_frames={} source_sha256={} first_caption_ms={} concurrent_http_ms={} actual_drain_ms={}", samples.len()/2*16, digest(&audio),caption_elapsed.as_millis(),http_elapsed.as_millis(),drain_started.elapsed().as_millis());
    }).catch_unwind().await;
    fixture.service.wait_for_shutdown().await.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
    assert_eq!(std::fs::read_dir(snapshots.path()).unwrap().count(), 0);
}
