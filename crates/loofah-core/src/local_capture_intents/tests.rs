use super::*;
use crate::{
    search::SearchService, settings::ConfigState, startup::StartupState, transcription_jobs::*,
};
use hypr_loofah_protocol::{TranscriptionFailure, TranscriptionJobRequest};
use hypr_vault_read::{TranscriptWithData, TranscriptWord};
use std::sync::atomic::{AtomicUsize, Ordering};

async fn context(root: &std::path::Path) -> Arc<VaultContext> {
    let vault = root.join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let lease = match hypr_storage::vault::VaultLease::try_acquire_existing(&vault).unwrap() {
        hypr_storage::vault::LeaseAttempt::Acquired(lease) => lease,
        _ => panic!("busy fixture"),
    };
    let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
    if !vault.join("sessions/note/_meta.json").exists() {
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Note","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
        store.create_note_session(&meta, "Notes").await.unwrap();
    }
    Arc::new(
        VaultContext::new(
            "vault".into(),
            store,
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.join("operations"))
        .with_vault_lease(lease)
        .unwrap(),
    )
}
fn declaration() -> LocalCaptureDeclaration {
    LocalCaptureDeclaration {
        request_id: uuid::Uuid::new_v4().to_string(),
        session_id: "note".into(),
        append: false,
        batch_requested: false,
        transcription: None,
        summary: None,
    }
}
fn persistence() -> LocalCapturePersistence {
    LocalCapturePersistence {
        live_transcription_active: false,
        needs_batch_repair: false,
        new_transcript_id: None,
    }
}
async fn terminal(service: &LocalCaptureIntents, id: &str) -> LocalCaptureStatus {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let state = service.state.lock().await;
            let entry = state.get(id).unwrap();
            if !entry.running && entry.admission.is_none() {
                return entry.record.status.clone();
            }
            drop(state);
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn original_native_physical_and_persistence_barriers_both_precede_source_binding() {
    for physical_first in [true, false] {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let context = context(&root).await;
        let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
        let input = declaration();
        let token = service.declare(input.clone()).await.unwrap();
        std::fs::write(
            root.join("vault/sessions/note/audio.wav"),
            b"native completed audio",
        )
        .unwrap();
        if physical_first {
            token.physical_finished(Ok(())).await.unwrap();
        } else {
            token.persistence_finished(Ok(persistence())).await.unwrap();
        }
        assert!(context.try_session_lifecycle_change("note").is_err());
        assert!(
            service
                .state
                .lock()
                .await
                .get(&input.request_id)
                .unwrap()
                .record
                .source
                .is_none()
        );
        if physical_first {
            token.persistence_finished(Ok(persistence())).await.unwrap();
        } else {
            token.physical_finished(Ok(())).await.unwrap();
        }
        assert_eq!(
            terminal(&service, &input.request_id).await.state,
            LocalCaptureState::Skipped
        );
        assert!(context.try_session_lifecycle_change("note").is_ok());
        service.shutdown().await;
    }
}
#[tokio::test]
async fn process_restart_requires_review_and_never_reconstructs_unconfirmed_producer_completion() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let first = context(&path).await;
    let service = LocalCaptureIntents::open(first.clone()).await.unwrap();
    let input = declaration();
    let token = service.declare(input.clone()).await.unwrap();
    assert!(service.declare(input.clone()).await.is_err());
    drop(token);
    drop(service);
    drop(first);
    let next = context(&path).await;
    let restored = LocalCaptureIntents::open(next.clone()).await.unwrap();
    assert_eq!(
        restored.status(&input.request_id).await.unwrap().state,
        LocalCaptureState::Review
    );
    assert!(restored.recover(input.request_id).await.is_err());
    assert!(next.ensure_session_jobs_idle("note").await.is_err());
    restored.shutdown().await;
}
#[tokio::test]
async fn declaration_binds_authoritative_append_state_and_original_summary_revision() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let mut input = declaration();
    input.append = true;
    assert!(service.declare(input.clone()).await.is_err());
    input.append = false;
    input.summary = Some(CaptureProcessingSummary {
        target: hypr_loofah_protocol::DocumentTarget::Summary,
        title_policy: Default::default(),
    });
    let token = service.declare(input.clone()).await.unwrap();
    let record = service
        .state
        .lock()
        .await
        .get(&input.request_id)
        .unwrap()
        .record
        .clone();
    assert!(record.summary.unwrap().only_if_empty);
    token.not_started().await.unwrap();
    assert_eq!(
        service.status(&input.request_id).await.unwrap().state,
        LocalCaptureState::NotStarted
    );
    service.shutdown().await;
}
#[derive(Clone, Default)]
struct Engine {
    calls: Arc<AtomicUsize>,
    busy: Arc<std::sync::atomic::AtomicBool>,
    entered: Arc<Notify>,
    release: Arc<Notify>,
}
struct Prepared(Engine);
impl TranscriptionEngine for Engine {
    fn prepare<'a>(
        &'a self,
        request: &'a TranscriptionJobRequest,
        _: CancellationToken,
    ) -> TranscriptionFuture<'a, Result<Box<dyn PreparedTranscription>, TranscriptionFailure>> {
        Box::pin(async move {
            assert_eq!(request.model, "frozen-model");
            assert_eq!(request.languages, vec!["nl"]);
            if self.busy.load(Ordering::SeqCst) {
                return Err(TranscriptionFailure::ResourceLimit);
            }
            Ok(Box::new(Prepared(self.clone())) as Box<dyn PreparedTranscription>)
        })
    }
}
impl PreparedTranscription for Prepared {
    fn transcribe(
        self: Box<Self>,
        input: TranscriptionInput,
        _: CancellationToken,
    ) -> TranscriptionFuture<'static, Result<TranscriptWithData, TranscriptionFailure>> {
        Box::pin(async move {
            self.0.calls.fetch_add(1, Ordering::SeqCst);
            self.0.entered.notify_one();
            self.0.release.notified().await;
            assert_eq!(input.metadata.user_id, TRANSCRIPTION_OWNER);
            assert_eq!(
                std::fs::read(input.source.path()).unwrap(),
                b"original native audio"
            );
            let mut transcript = input.metadata.empty_transcript();
            transcript.words = (0..5)
                .map(|i| TranscriptWord {
                    id: Some(uuid::Uuid::new_v4().to_string()),
                    text: format!("{i}{}", "界".repeat(40)),
                    start_ms: i as f64,
                    end_ms: i as f64 + 1.0,
                    channel: 0.0,
                    speaker: None,
                    metadata: None,
                })
                .collect();
            drop(input);
            Ok(transcript)
        })
    }
}
struct Auth;
impl TranscriptionAuthorizer for Auth {
    fn observe<'a>(
        &'a self,
        work: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>> {
        Box::pin(async move {
            assert_eq!(work.principal, TRANSCRIPTION_OWNER);
            Ok(TranscriptionAccess {
                cancelled: CancellationToken::new(),
                interrupted: CancellationToken::new(),
                owner: Box::new(()),
            })
        })
    }
    fn authorize<'a>(
        &'a self,
        work: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        Box::pin(async move {
            assert_eq!(work.principal, TRANSCRIPTION_OWNER);
            Ok(Box::new(()) as Box<dyn Send>)
        })
    }
}
#[tokio::test]
async fn changed_canonical_source_retains_output_and_explicit_recovery_never_repeats_inference() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let engine = Engine::default();
    let worker = context
        .start_transcription(Arc::new(engine.clone()), Arc::new(Auth), Default::default())
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let mut input = declaration();
    input.batch_requested = true;
    input.transcription = Some(LocalTranscriptionOptions {
        model: "frozen-model".into(),
        languages: vec!["nl".into()],
        keywords: vec![],
    });
    let token = service.declare(input.clone()).await.unwrap();
    let audio = path.join("vault/sessions/note/audio.wav");
    std::fs::write(&audio, b"original native audio").unwrap();
    token.physical_finished(Ok(())).await.unwrap();
    token.persistence_finished(Ok(persistence())).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine.entered.notified(),
    )
    .await
    .unwrap();
    std::fs::write(&audio, b"replacement audio").unwrap();
    engine.release.notify_one();
    assert_eq!(
        terminal(&service, &input.request_id).await.state,
        LocalCaptureState::Review
    );
    assert!(
        service
            .root
            .join(&input.request_id)
            .join("result.json")
            .is_file()
    );
    assert!(!path.join("vault/sessions/note/transcript.json").exists());
    std::fs::write(&audio, b"original native audio").unwrap();
    service.recover(input.request_id.clone()).await.unwrap();
    assert_eq!(
        terminal(&service, &input.request_id).await.state,
        LocalCaptureState::Review
    );
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    assert!(
        context
            .session_transcripts("note")
            .await
            .unwrap()
            .is_empty()
    );
    service.shutdown().await;
    worker.shutdown().await.unwrap();
}
#[test]
fn eligibility_counts_unicode_characters_and_requires_five_canonical_words() {
    let mut transcript:TranscriptWithData=serde_json::from_value(serde_json::json!({"id":"t","user_id":"u","created_at":"2026-11-08T00:00:00Z","session_id":"note","started_at":0.0,"ended_at":null,"memo_md":"","words":[],"speaker_hints":[]})).unwrap();
    transcript.words = (0..4)
        .map(|_| TranscriptWord {
            id: None,
            text: "界".repeat(40),
            start_ms: 0.0,
            end_ms: 1.0,
            channel: 0.0,
            speaker: None,
            metadata: None,
        })
        .collect();
    assert!(!worker::automatic_eligible(&[transcript.clone()]));
    transcript.words.push(transcript.words[0].clone());
    assert!(worker::automatic_eligible(&[transcript.clone()]));
    for word in &mut transcript.words {
        word.text = "  界\n\t".repeat(10);
    }
    assert!(!worker::automatic_eligible(&[transcript]));
}

#[derive(Default)]
struct Summary {
    entered: Notify,
    release: Notify,
    calls: AtomicUsize,
}
impl crate::jobs::JobGenerator for Summary {
    fn generate(
        &self,
        work: crate::jobs::JobWork,
        _: CancellationToken,
    ) -> crate::jobs::JobFuture<
        '_,
        Result<crate::jobs::GeneratedDocument, hypr_loofah_protocol::ProcessingJobFailure>,
    > {
        Box::pin(async move {
            assert_eq!(work.principal, SUMMARY_OWNER);
            assert!(work.request.only_if_empty);
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.notify_one();
            self.release.notified().await;
            Ok(crate::jobs::GeneratedDocument {
                markdown: "Generated summary".into(),
                suggested_tags: None,
            })
        })
    }
}
impl crate::jobs::JobAuthorizer for Auth {
    fn authorize<'a>(
        &'a self,
        work: &'a crate::jobs::JobWork,
    ) -> crate::jobs::JobFuture<'a, Result<Box<dyn Send>, hypr_loofah_protocol::ProcessingJobFailure>>
    {
        Box::pin(async move {
            assert_eq!(work.principal, SUMMARY_OWNER);
            Ok(Box::new(()) as Box<dyn Send>)
        })
    }
}
async fn live_summary_capture(
    context: &Arc<VaultContext>,
    service: &Arc<LocalCaptureIntents>,
    root: &std::path::Path,
) -> (LocalCaptureDeclaration, LocalCaptureIntent) {
    let mut declaration = declaration();
    declaration.transcription = Some(LocalTranscriptionOptions {
        model: "frozen-model".into(),
        languages: vec!["nl".into()],
        keywords: vec![],
    });
    declaration.summary = Some(CaptureProcessingSummary {
        target: hypr_loofah_protocol::DocumentTarget::Summary,
        title_policy: Default::default(),
    });
    let token = service.declare(declaration.clone()).await.unwrap();
    std::fs::write(
        root.join("vault/sessions/note/audio.wav"),
        b"native completed audio",
    )
    .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let transcript:TranscriptWithData=serde_json::from_value(serde_json::json!({"id":id,"session_id":"note","user_id":TRANSCRIPTION_OWNER,"created_at":"2026-11-08T00:00:00Z","started_at":0.0,"ended_at":null,"memo_md":"","words":(0..5).map(|i|serde_json::json!({"id":uuid::Uuid::new_v4().to_string(),"text":"界".repeat(40),"start_ms":i as f64,"end_ms":i as f64+1.0,"channel":0.0,"speaker":null,"metadata":null})).collect::<Vec<_>>(),"speaker_hints":[]})).unwrap();
    context
        .store
        .write_transcript("note", transcript)
        .await
        .unwrap();
    token.physical_finished(Ok(())).await.unwrap();
    token
        .persistence_finished(Ok(LocalCapturePersistence {
            live_transcription_active: true,
            needs_batch_repair: false,
            new_transcript_id: Some(id),
        }))
        .await
        .unwrap();
    (declaration, token)
}
#[tokio::test]
async fn summary_keeps_original_lifecycle_admission_until_actual_cancelled_generation_drains() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let context = context(&root).await;
    let summary = Arc::new(Summary::default());
    let worker = context
        .start_processing(summary.clone(), Arc::new(Auth))
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let (declaration, token) = live_summary_capture(&context, &service, &root).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        summary.entered.notified(),
    )
    .await
    .unwrap();
    token.physical_finished(Ok(())).await.unwrap();
    assert!(context.try_session_lifecycle_change("note").is_err());
    service.request_shutdown();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), service.shutdown())
            .await
            .is_err()
    );
    assert!(context.try_session_lifecycle_change("note").is_err());
    summary.release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(10), service.shutdown())
        .await
        .unwrap();
    assert_ne!(
        service.status(&declaration.request_id).await.unwrap().state,
        LocalCaptureState::Succeeded
    );
    assert!(
        context
            .read_document("note", &hypr_loofah_protocol::DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .is_none()
    );
    worker.shutdown().await.unwrap();
}
#[tokio::test]
async fn changed_transcript_during_summary_keeps_the_target_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let context = context(&root).await;
    let summary = Arc::new(Summary::default());
    let worker = context
        .start_processing(summary.clone(), Arc::new(Auth))
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let (declaration, _token) = live_summary_capture(&context, &service, &root).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        summary.entered.notified(),
    )
    .await
    .unwrap();
    let path = root.join("vault/sessions/note/transcript.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["future_user_edit"] = serde_json::json!(true);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    summary.release.notify_one();
    assert_eq!(
        terminal(&service, &declaration.request_id).await.state,
        LocalCaptureState::Conflict
    );
    assert!(
        context
            .read_document("note", &hypr_loofah_protocol::DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .is_none()
    );
    assert_eq!(summary.calls.load(Ordering::SeqCst), 1);
    service.shutdown().await;
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn confirmed_batch_capture_publishes_retained_output_once_under_original_owner() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let context = context(&root).await;
    let engine = Engine::default();
    let worker = context
        .start_transcription(Arc::new(engine.clone()), Arc::new(Auth), Default::default())
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let mut input = declaration();
    input.batch_requested = true;
    input.transcription = Some(LocalTranscriptionOptions {
        model: "frozen-model".into(),
        languages: vec!["nl".into()],
        keywords: vec![],
    });
    let token = service.declare(input.clone()).await.unwrap();
    std::fs::write(
        root.join("vault/sessions/note/audio.wav"),
        b"original native audio",
    )
    .unwrap();
    token.physical_finished(Ok(())).await.unwrap();
    token.persistence_finished(Ok(persistence())).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine.entered.notified(),
    )
    .await
    .unwrap();
    engine.release.notify_one();
    assert_eq!(
        terminal(&service, &input.request_id).await.state,
        LocalCaptureState::Succeeded
    );
    let output = context.session_transcripts("note").await.unwrap();
    assert_eq!(output.len(), 1);
    assert_eq!(output[0].words.len(), 5);
    assert_eq!(output[0].user_id, TRANSCRIPTION_OWNER);
    assert_eq!(
        service.recover(input.request_id).await.unwrap().state,
        LocalCaptureState::Succeeded
    );
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    service.shutdown().await;
    worker.shutdown().await.unwrap();
}
#[tokio::test]
async fn lost_parent_summary_completion_replays_original_child_without_new_generation_or_write() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let context = context(&root).await;
    let summary = Arc::new(Summary::default());
    let worker = context
        .start_processing(summary.clone(), Arc::new(Auth))
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let (declaration, _token) = live_summary_capture(&context, &service, &root).await;
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        summary.entered.notified(),
    )
    .await
    .unwrap();
    summary.release.notify_one();
    assert_eq!(
        terminal(&service, &declaration.request_id).await.state,
        LocalCaptureState::Succeeded
    );
    let before = std::fs::read(root.join("vault/sessions/note/summary.md")).unwrap();
    service
        .update(&declaration.request_id, |record| {
            record.status.state = LocalCaptureState::Review;
            record.status.summary_job_id = None;
        })
        .await
        .unwrap();
    service
        .recover(declaration.request_id.clone())
        .await
        .unwrap();
    assert_eq!(
        terminal(&service, &declaration.request_id).await.state,
        LocalCaptureState::Succeeded
    );
    assert_eq!(summary.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        std::fs::read(root.join("vault/sessions/note/summary.md")).unwrap(),
        before
    );
    service.shutdown().await;
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn busy_model_preflight_copies_no_audio_and_explicit_retry_dispatches_only_once() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let context = context(&root).await;
    let engine = Engine::default();
    engine.busy.store(true, Ordering::SeqCst);
    let worker = context
        .start_transcription(Arc::new(engine.clone()), Arc::new(Auth), Default::default())
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let mut input = declaration();
    input.batch_requested = true;
    input.transcription = Some(LocalTranscriptionOptions {
        model: "frozen-model".into(),
        languages: vec!["nl".into()],
        keywords: vec![],
    });
    let token = service.declare(input.clone()).await.unwrap();
    std::fs::write(
        root.join("vault/sessions/note/audio.wav"),
        b"original native audio",
    )
    .unwrap();
    token.physical_finished(Ok(())).await.unwrap();
    token.persistence_finished(Ok(persistence())).await.unwrap();
    assert_eq!(
        terminal(&service, &input.request_id).await.state,
        LocalCaptureState::Review
    );
    assert!(
        !service
            .root
            .join(&input.request_id)
            .join("source.wav")
            .exists()
    );
    assert!(
        !service
            .state
            .lock()
            .await
            .get(&input.request_id)
            .unwrap()
            .record
            .inference_dispatched
    );
    engine.busy.store(false, Ordering::SeqCst);
    service.recover(input.request_id.clone()).await.unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        engine.entered.notified(),
    )
    .await
    .unwrap();
    engine.release.notify_one();
    assert_eq!(
        terminal(&service, &input.request_id).await.state,
        LocalCaptureState::Succeeded
    );
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    service.shutdown().await;
    worker.shutdown().await.unwrap();
}
