use super::*;
use hypr_loofah_core::transcription_jobs::{
    PreparedTranscription, TranscriptionEngine, TranscriptionFuture, TranscriptionInput,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    path::Path,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;
mod capture_live;

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn seed(context: &Arc<hypr_loofah_core::VaultContext>, audio: &[u8]) -> std::path::PathBuf {
    let metadata = serde_json::from_value(json!({
        "id":"speech", "title":"Saved capture", "created_at":"2026-11-08T10:00:00Z", "tags":[],
        "audio":{"source":"recording", "layout":"mic_system"}
    }))
    .unwrap();
    context
        .store
        .create_note_session(&metadata, "# Saved capture\n\nKeep the original note.")
        .await
        .unwrap();
    let directory = context.store.vault_base().join("sessions/speech");
    std::fs::write(directory.join("audio.wav"), audio).unwrap();
    std::fs::write(
        directory.join("unknown-user-file.txt"),
        b"Private user attachment",
    )
    .unwrap();
    std::fs::create_dir_all(directory.join("attachments")).unwrap();
    std::fs::write(
        directory.join("attachments/owned-image.png"),
        b"Managed attachment",
    )
    .unwrap();
    let meta_path = directory.join("_meta.json");
    let mut raw: Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    raw["future"] = json!({"preserve":true});
    raw["audio"]["future"] = json!({"preserve":"nested"});
    std::fs::write(meta_path, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
    context.store.refresh_session("speech").await.unwrap();
    directory
}

fn request(audio: &[u8]) -> TranscriptionJobRequest {
    TranscriptionJobRequest {
        session_id: "speech".into(),
        expected_recording_sha256: digest(audio),
        expected_transcript: AbsentTranscriptExpectation::Absent,
        model: "operator-base".into(),
        languages: vec!["en".into()],
        keywords: vec![],
    }
}

async fn terminal(
    context: &Arc<hypr_loofah_core::VaultContext>,
    member: &str,
    id: &str,
    deadline: Duration,
) -> TranscriptionJobSnapshot {
    tokio::time::timeout(deadline, async {
        loop {
            let snapshot = context
                .transcription_job(member.into(), id.into())
                .await
                .unwrap();
            if snapshot.state.is_terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

struct Fake {
    capacity: Arc<Semaphore>,
    started: Arc<Semaphore>,
    release: Arc<Semaphore>,
    calls: AtomicUsize,
    source_released: Arc<AtomicBool>,
}
impl Fake {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            capacity: Arc::new(Semaphore::new(1)),
            started: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
            calls: AtomicUsize::new(0),
            source_released: Arc::new(AtomicBool::new(false)),
        })
    }
    async fn wait_started(&self) {
        tokio::time::timeout(Duration::from_secs(15), self.started.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}
struct PreparedFake {
    permit: OwnedSemaphorePermit,
    release: Arc<Semaphore>,
    source_released: Arc<AtomicBool>,
    started: Arc<Semaphore>,
}
impl TranscriptionEngine for Fake {
    fn prepare<'a>(
        &'a self,
        _: &'a TranscriptionJobRequest,
        _: CancellationToken,
    ) -> TranscriptionFuture<
        'a,
        std::result::Result<Box<dyn PreparedTranscription>, TranscriptionFailure>,
    > {
        Box::pin(async move {
            let permit = self
                .capacity
                .clone()
                .try_acquire_owned()
                .map_err(|_| TranscriptionFailure::ResourceLimit)?;
            self.calls.fetch_add(1, Ordering::AcqRel);
            Ok(Box::new(PreparedFake {
                permit,
                release: self.release.clone(),
                source_released: self.source_released.clone(),
                started: self.started.clone(),
            }) as Box<dyn PreparedTranscription>)
        })
    }
}
struct Released(Arc<AtomicBool>);
impl Drop for Released {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
struct RetainedSource {
    _source: Box<dyn Send>,
    _released: Released,
}
impl PreparedTranscription for PreparedFake {
    fn transcribe(
        self: Box<Self>,
        input: TranscriptionInput,
        _: CancellationToken,
    ) -> TranscriptionFuture<
        'static,
        std::result::Result<hypr_vault_read::TranscriptWithData, TranscriptionFailure>,
    > {
        Box::pin(async move {
            tokio::spawn(async move {
                let (_, source) = input.source.into_parts();
                let source = RetainedSource { _source: source, _released: Released(self.source_released.clone()) };
                self.started.add_permits(1);
                self.release.acquire().await.unwrap().forget();
                let mut result = input.metadata.empty_transcript();
                result.words.push(serde_json::from_value(json!({"id":uuid::Uuid::new_v4().to_string(),"text":"Late completed speech.","start_ms":0,"end_ms":100,"channel":2,"metadata":{"timing_source":"provider_word"}})).unwrap());
                drop(self.permit);
                drop(source);
                Ok(result)
            }).await.unwrap()
        })
    }
}

#[tokio::test]
async fn absent_operator_configuration_keeps_notes_without_a_transcription_runtime_or_model() {
    let fixture = Fixture::new().await;
    assert!(fixture.service.transcription_resource_observer().is_none());
    let (_, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        !capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("batch_transcription_jobs"))
    );
    let me = fixture.login("alice").await;
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    seed(&context, b"Canonical audio").await;
    assert!(
        matches!(context.submit_transcription_job(me.member.id, uuid::Uuid::new_v4().to_string(), request(b"Canonical audio")).await, Err(error) if error.code == ErrorCode::Unavailable)
    );
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn injected_global_capacity_is_shared_between_vaults_and_shutdown_retains_actual_source_work()
{
    let fixture = Fixture::new().await;
    let engine = Fake::new();
    assert!(
        fixture
            .service
            .transcription_test_engine
            .set(engine.clone())
            .is_ok()
    );
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let first = fixture
        .service
        .vault(&alice.personal_vault_id)
        .await
        .unwrap();
    let second = fixture.service.vault(&bob.personal_vault_id).await.unwrap();
    let directory = seed(&first, b"First recording").await;
    seed(&second, b"Second recording").await;
    first
        .submit_transcription_job(
            alice.member.id.clone(),
            uuid::Uuid::new_v4().to_string(),
            request(b"First recording"),
        )
        .await
        .unwrap();
    engine.wait_started().await;
    // Preparation reserves the same global native slot before the second vault copies a source.
    let busy = second
        .submit_transcription_job(
            bob.member.id.clone(),
            uuid::Uuid::new_v4().to_string(),
            request(b"Second recording"),
        )
        .await
        .unwrap();
    let busy = terminal(&second, &bob.member.id, &busy.id, Duration::from_secs(15)).await;
    assert_eq!(busy.failure, Some(TranscriptionFailure::ResourceLimit));
    assert_eq!(engine.calls.load(Ordering::Acquire), 1);
    let startup = first.startup.clone();
    drop(first);
    drop(second);
    assert!(
        fixture
            .service
            .shutdown_with_deadline(Duration::from_millis(20))
            .await
            .is_err()
    );
    assert!(!engine.source_released.load(Ordering::Acquire));
    let lease = File::options()
        .read(true)
        .write(true)
        .open(
            fixture
                .service
                .config
                .storage_root
                .join("vaults")
                .join(&alice.personal_vault_id)
                .join(".loofah-vault.lock"),
        )
        .unwrap();
    assert!(matches!(
        lease.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    let service = fixture.service.clone();
    let waiter = tokio::spawn(async move { service.wait_for_shutdown().await });
    tokio::task::yield_now().await;
    waiter.abort();
    let _ = waiter.await;
    assert!(!engine.source_released.load(Ordering::Acquire));
    engine.release.add_permits(1);
    tokio::time::timeout(Duration::from_secs(15), fixture.service.wait_for_shutdown())
        .await
        .unwrap()
        .unwrap();
    assert!(engine.source_released.load(Ordering::Acquire));
    assert_eq!(
        startup.snapshot().phase,
        hypr_loofah_core::startup::StartupPhase::Stopped
    );
    assert!(lease.try_lock().is_ok());
    assert!(!directory.join("transcript.json").exists());
    assert_eq!(
        std::fs::read(directory.join("unknown-user-file.txt")).unwrap(),
        b"Private user attachment"
    );
}

fn model_config(path: std::path::PathBuf, bytes: &[u8]) -> crate::config::TranscriptionConfig {
    serde_json::from_value(json!({"model":"operator-base","model_path":path,"model_size_bytes":bytes.len(),"model_sha256":digest(bytes)})).unwrap()
}

#[tokio::test]
async fn failed_configured_model_keeps_notes_and_reports_engine_unavailable_without_fallback() {
    let model = tempfile::tempdir().unwrap();
    let fixture = Fixture::with_transcription(
        None,
        None,
        Some(model_config(
            model.path().join("missing.bin"),
            b"Expected model bytes",
        )),
    )
    .await;
    let mut resources = fixture.service.transcription_resource_observer().unwrap();
    let (_, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("batch_transcription_jobs"))
    );
    assert!(
        capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("capture_live_transcription"))
    );
    tokio::time::timeout(Duration::from_secs(15), async {
        while !matches!(
            resources.snapshot().phase,
            hypr_transcribe_whisper_local::resources::ResourcePhase::Failed(_)
        ) {
            resources.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let (_, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("batch_transcription_jobs"))
    );
    let me = fixture.login("alice").await;
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    let directory = seed(&context, b"Canonical audio").await;
    let job = context
        .submit_transcription_job(
            me.member.id.clone(),
            uuid::Uuid::new_v4().to_string(),
            request(b"Canonical audio"),
        )
        .await
        .unwrap();
    let job = terminal(&context, &me.member.id, &job.id, Duration::from_secs(15)).await;
    assert_eq!(job.failure, Some(TranscriptionFailure::EngineUnavailable));
    assert!(!directory.join("transcript.json").exists());
    assert_eq!(
        std::fs::read(directory.join("audio.wav")).unwrap(),
        b"Canonical audio"
    );
    fixture.service.shutdown().await.unwrap();
    assert_eq!(
        resources.snapshot().phase,
        hypr_transcribe_whisper_local::resources::ResourcePhase::Stopped
    );
}

fn speech_wav() -> Vec<u8> {
    let path =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/data/src/english_1/audio.wav");
    let mut reader = hound::WavReader::open(path).unwrap();
    let spec = reader.spec();
    assert_eq!(spec.channels, 1);
    assert_eq!(spec.bits_per_sample, 16);
    let mut bytes = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 2,
                sample_rate: spec.sample_rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for sample in reader.samples::<i16>().take(spec.sample_rate as usize * 20) {
            let sample = sample.unwrap() as f32 / 32768.0;
            writer.write_sample(sample).unwrap();
            writer.write_sample(sample * 0.55).unwrap();
        }
        writer.finalize().unwrap();
    }
    bytes.into_inner()
}

#[tokio::test]
#[ignore = "requires the existing provisioned LOOFAH_WHISPER_MODEL; real service CPU activation"]
async fn real_configured_cpu_service_publishes_canonical_dual_channel_transcript_and_preserves_source()
 {
    let path = std::fs::canonicalize(std::env::var("LOOFAH_WHISPER_MODEL").unwrap()).unwrap();
    let model = std::fs::read(&path).unwrap();
    assert_eq!(
        digest(&model),
        "c577b9a86e7e048a0b7eada054f4dd79a56bbfa911fbdacf900ac5b567cbb7d9"
    );
    let fixture =
        Fixture::with_transcription(None, None, Some(model_config(path.clone(), &model))).await;
    let mut resources = fixture.service.transcription_resource_observer().unwrap();
    tokio::time::timeout(Duration::from_secs(180), async {
        while resources.snapshot().phase
            != hypr_transcribe_whisper_local::resources::ResourcePhase::Ready
        {
            assert!(!matches!(
                resources.snapshot().phase,
                hypr_transcribe_whisper_local::resources::ResourcePhase::Failed(_)
            ));
            resources.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let me = fixture.login("alice").await;
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    let audio = speech_wav();
    let directory = seed(&context, &audio).await;
    let metadata = std::fs::read(directory.join("_meta.json")).unwrap();
    let note = std::fs::read(directory.join("notes.md")).unwrap();
    let before = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as f64;
    let accepted = context
        .submit_transcription_job(
            me.member.id.clone(),
            uuid::Uuid::new_v4().to_string(),
            request(&audio),
        )
        .await
        .unwrap();
    let done = terminal(
        &context,
        &me.member.id,
        &accepted.id,
        Duration::from_secs(180),
    )
    .await;
    assert_eq!(done.state, TranscriptionJobState::Succeeded, "{done:?}");
    let retained = context
        .transcription_job_result(me.member.id.clone(), done.id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.session_id, "speech");
    assert_eq!(retained.job_id, done.id);
    let retained = retained.transcript;
    assert_eq!(retained.user_id, me.member.id);
    assert_eq!(retained.created_at, accepted.created_at);
    assert!(retained.started_at >= before);
    assert_eq!(retained.ended_at, None);
    assert!(retained.memo_md.contains("Keep the original note."));
    assert!(!retained.words.is_empty());
    assert!(retained.words.iter().any(|word| word.channel == 0.0));
    assert!(retained.words.iter().any(|word| word.channel == 1.0));
    assert!(retained.words.iter().all(|word| word.start_ms.is_finite()
        && word.end_ms.is_finite()
        && word.start_ms <= word.end_ms
        && word.metadata.as_ref().unwrap().get("capture_source") == Some(&json!("recording"))));
    let canonical =
        hypr_vault_read::transcript::read_transcript_json(context.store.vault_base(), "speech")
            .unwrap();
    assert_eq!(canonical.transcripts.len(), 1);
    assert_eq!(
        serde_json::to_value(&canonical.transcripts[0]).unwrap(),
        serde_json::to_value(&retained).unwrap()
    );
    assert_eq!(
        std::fs::read(directory.join("_meta.json")).unwrap(),
        metadata
    );
    assert_eq!(std::fs::read(directory.join("notes.md")).unwrap(), note);
    assert_eq!(std::fs::read(directory.join("audio.wav")).unwrap(), audio);
    assert_eq!(
        std::fs::read(directory.join("unknown-user-file.txt")).unwrap(),
        b"Private user attachment"
    );
    assert_eq!(
        std::fs::read(directory.join("attachments/owned-image.png")).unwrap(),
        b"Managed attachment"
    );
    println!("CPU_HOST_MODEL_SHA256={}", digest(&model));
    println!("CPU_HOST_CANONICAL_WAV_SHA256={}", digest(&audio));
    println!(
        "CPU_HOST_TRANSCRIPT_SHA256={}",
        digest(&std::fs::read(directory.join("transcript.json")).unwrap())
    );
    fixture.service.shutdown().await.unwrap();
    assert_eq!(digest(&std::fs::read(path).unwrap()), digest(&model));
    assert_eq!(
        resources.snapshot().phase,
        hypr_transcribe_whisper_local::resources::ResourcePhase::Stopped
    );
}
