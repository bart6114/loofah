use super::*;

#[test]
fn onnx_options_reject_unsupported_live_languages_and_keyword_promises() {
    assert!(options("speech", "speech", &[], &[], true).is_ok());
    assert!(options("speech", "speech", &["en".into()], &[], true).is_ok());
    assert!(options("speech", "speech", &["nl".into()], &[], false).is_ok());
    assert!(options("speech", "speech", &["nl".into()], &[], true).is_err());
    assert!(options("speech", "speech", &["zh".into()], &[], false).is_err());
    assert!(options("speech", "speech", &[], &["keyword".into()], false).is_err());
    assert!(options("speech", "/operator/models", &[], &[], false).is_err());
}

#[test]
fn onnx_live_ingress_is_bounded_exact_and_finish_survives_full_queue() {
    use hypr_loofah_core::capture_live::{CaptureLiveControl, CaptureLiveIngress};
    let (sender, mut receiver) = tokio::sync::mpsc::channel(1);
    let control = live::Control {
        sender,
        channels: 1,
        cursor: Mutex::new(live::Cursor {
            end: 0,
            last: None,
            finished: false,
        }),
        cancelled: AtomicBool::new(false),
    };
    let audio: Arc<[f32]> = vec![0.1; 1600].into();
    assert_eq!(
        control.try_accept(0, &audio).unwrap(),
        CaptureLiveIngress::Accepted
    );
    assert_eq!(
        control.try_accept(0, &audio).unwrap(),
        CaptureLiveIngress::Duplicate
    );
    assert_eq!(
        control.try_accept(1600, &audio).unwrap(),
        CaptureLiveIngress::Busy
    );
    assert!(control.try_accept(0, &vec![0.2; 1600].into()).is_err());
    control.finish(1600).unwrap();
    assert!(matches!(receiver.try_recv(), Ok(live::Command::Audio(_))));
    assert!(control.cursor.lock().unwrap().finished);
    assert!(control.try_accept(1600, &audio).is_err());
    control.cancel();
    assert!(control.try_accept(0, &audio).is_err());
}

struct Dropped(Arc<AtomicBool>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

#[tokio::test]
#[ignore = "requires explicitly provisioned LOOFAH_ONNX_MODELS; real dual-channel CPU streaming and actual cancellation drain"]
async fn real_onnx_live_cpu_preserves_channels_and_source_through_join() {
    use hypr_loofah_core::capture_live::{
        CaptureLiveEngine, CaptureLiveEngineSpec, CaptureLiveIngress,
    };
    use hypr_loofah_protocol::{
        AbsentTranscriptExpectation, CaptureFormat, CaptureLiveFailure, CaptureLiveRequest,
    };
    let root = std::path::PathBuf::from(std::env::var("LOOFAH_ONNX_MODELS").unwrap());
    let scratch = tempfile::tempdir().unwrap();
    let config = serde_json::from_value(serde_json::json!({ "model":"speech", "model_path":root.join("batch"),
        "onnx":{"streaming_model_path":root.join("streaming"), "diarizer_model_path":root.join("diarizer"), "snapshot_root":scratch.path()} })).unwrap();
    let host = OnnxHost::start(&config);
    tokio::time::timeout(std::time::Duration::from_secs(180), async {
        loop {
            let state = host.policy().availability;
            if state == TranscriptionAvailability::Ready {
                break;
            }
            assert_eq!(state, TranscriptionAvailability::Loading);
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/data/src/english_2/audio.wav");
    let mut reader = hound::WavReader::open(fixture).unwrap();
    let samples = reader
        .samples::<i16>()
        .map(|sample| sample.unwrap() as f32 / 32768.0)
        .collect::<Vec<_>>();
    for cancel_after_caption in [false, true] {
        let dropped = Arc::new(AtomicBool::new(false));
        let generation = uuid::Uuid::new_v4().to_string();
        let mut live = host
            .start(
                CaptureLiveEngineSpec {
                    generation_id: generation.clone(),
                    format: CaptureFormat::stereo(),
                    request: CaptureLiveRequest {
                        capture_fingerprint: "a".repeat(64),
                        expected_transcript: AbsentTranscriptExpectation::Absent,
                        model: "speech".into(),
                        languages: vec!["en".into()],
                        keywords: Vec::new(),
                    },
                },
                Box::new(Dropped(dropped.clone())),
            )
            .await
            .unwrap();
        assert!(!dropped.load(Ordering::Acquire));
        let control = live.control.clone();
        let audio = samples.clone();
        let feeder = tokio::spawn(async move {
            let mut frame = 0;
            for mono in audio.chunks(1600) {
                let pcm: Arc<[f32]> = mono
                    .iter()
                    .flat_map(|sample| [*sample, -*sample])
                    .collect::<Vec<_>>()
                    .into();
                loop {
                    match control.try_accept(frame, &pcm) {
                        Ok(CaptureLiveIngress::Accepted) => break,
                        Ok(CaptureLiveIngress::Busy) => {
                            tokio::time::sleep(std::time::Duration::from_millis(5)).await
                        }
                        Err(CaptureLiveFailure::Interrupted) => return,
                        other => panic!("{other:?}"),
                    }
                }
                frame += mono.len() as u64;
            }
            control.finish(frame).unwrap();
        });
        let mut physical = std::collections::BTreeSet::new();
        let mut count = 0;
        while let Some(caption) = live.captions.recv().await {
            assert_eq!(caption.generation_id, generation);
            assert_eq!(caption.sequence, count);
            count += 1;
            if !caption.new_words.is_empty() || !caption.partials.is_empty() {
                physical.insert(caption.physical_channel);
            }
            if cancel_after_caption {
                assert!(!dropped.load(Ordering::Acquire));
                live.control.cancel();
                break;
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            while live.completion.borrow().is_none() {
                live.completion.changed().await.unwrap();
            }
        })
        .await
        .unwrap();
        let receipt = live.completion.borrow().clone().unwrap();
        assert!(receipt.native_joined && receipt.owners_released);
        assert!(dropped.load(Ordering::Acquire));
        if cancel_after_caption {
            assert_eq!(receipt.failure, Some(CaptureLiveFailure::Interrupted));
        } else {
            assert_eq!(receipt.failure, None);
            assert_eq!(receipt.completed_through_frame, Some(samples.len() as u64));
            assert_eq!(receipt.caption_count, count);
            assert_eq!(physical, [0, 1].into_iter().collect());
        }
        feeder.await.unwrap();
    }
    host.shutdown().await;
    assert_eq!(host.resources.phase.load(Ordering::Acquire), STOPPED);
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn onnx_shutdown_retains_original_model_admission_until_release() {
    let host = Arc::new(OnnxHost {
        model: "public-alias".into(),
        resources: Arc::new(Resources {
            phase: AtomicU8::new(READY),
            shutdown: CancellationToken::new(),
            stopping: AtomicBool::new(false),
            slot: Arc::new(Semaphore::new(1)),
            models: Mutex::new(None),
        }),
    });
    let admission = host.admit().unwrap();
    let closing_host = host.clone();
    let closing = tokio::spawn(async move {
        closing_host.shutdown().await;
    });
    host.resources.shutdown.cancelled().await;
    assert!(!closing.is_finished());
    assert!(matches!(
        host.admit(),
        Err(TranscriptionFailure::EngineUnavailable)
    ));
    assert_eq!(
        host.policy().availability,
        TranscriptionAvailability::Stopping
    );
    drop(admission);
    closing.await.unwrap();
    assert_eq!(host.resources.phase.load(Ordering::Acquire), STOPPED);
}

#[tokio::test]
async fn failed_onnx_verification_is_redacted_and_never_admits_work() {
    let models = tempfile::tempdir().unwrap();
    let scratch = tempfile::tempdir().unwrap();
    let config = serde_json::from_value(serde_json::json!({"model":"public-alias", "model_path":models.path(),
        "onnx":{"streaming_model_path":models.path(),"diarizer_model_path":models.path(),"snapshot_root":scratch.path()} })).unwrap();
    let host = OnnxHost::start(&config);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while host.resources.phase.load(Ordering::Acquire) == LOADING {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let policy = host.policy();
    assert_eq!(policy.availability, TranscriptionAvailability::Unavailable);
    assert!(
        !serde_json::to_string(&policy)
            .unwrap()
            .contains(models.path().to_str().unwrap())
    );
    assert!(matches!(
        host.admit(),
        Err(TranscriptionFailure::EngineUnavailable)
    ));
    host.shutdown().await;
    assert_eq!(std::fs::read_dir(scratch.path()).unwrap().count(), 0);
}
