use super::*;
use crate::{
    search::SearchService, settings::ConfigState, startup::StartupState, transcription_jobs::*,
};
use hypr_loofah_protocol::{TranscriptionFailure, TranscriptionJobRequest};
use hypr_vault_read::{TranscriptWithData, TranscriptWord};
use std::{
    future::Future,
    sync::atomic::{AtomicUsize, Ordering},
};

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
    prepares: Arc<AtomicUsize>,
    returned: Arc<Notify>,
    owner_release: Option<Arc<Notify>>,
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
            self.prepares.fetch_add(1, Ordering::SeqCst);
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
            if let Some(release) = &self.0.owner_release {
                let release = release.clone();
                tokio::spawn(async move {
                    release.notified().await;
                    drop(input);
                });
                self.0.returned.notify_one();
            } else {
                drop(input);
            }
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
    // Replace the path as an external editor does; the original file remains
    // shared-locked through model work and Windows correctly rejects truncation.
    hypr_storage::fs::atomic_write(&audio, "replacement audio").unwrap();
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
    hypr_storage::fs::atomic_write(&audio, "original native audio").unwrap();
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

async fn batch_capture(
    context: &Arc<VaultContext>,
    service: &Arc<LocalCaptureIntents>,
    summary: bool,
) -> LocalCaptureDeclaration {
    let mut input = declaration();
    input.batch_requested = true;
    input.transcription = Some(LocalTranscriptionOptions {
        model: "frozen-model".into(),
        languages: vec!["nl".into()],
        keywords: vec![],
    });
    input.summary = summary.then_some(CaptureProcessingSummary {
        target: hypr_loofah_protocol::DocumentTarget::Summary,
        title_policy: hypr_loofah_protocol::TitlePolicy::GenerateIfEmpty,
    });
    let token = service.declare(input.clone()).await.unwrap();
    std::fs::write(
        context.store.vault_base().join("sessions/note/audio.wav"),
        b"original native audio",
    )
    .unwrap();
    token.physical_finished(Ok(())).await.unwrap();
    token.persistence_finished(Ok(persistence())).await.unwrap();
    input
}
async fn notified(notification: &Notify) {
    tokio::time::timeout(std::time::Duration::from_secs(10), notification.notified())
        .await
        .unwrap();
}

#[tokio::test]
async fn local_capture_cancel_waits_for_native_source_drain_retains_late_result_and_survives_restart()
 {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let summary = Arc::new(Summary::default());
    let summary_worker = context
        .start_processing(summary.clone(), Arc::new(Auth))
        .await
        .unwrap();
    let release = Arc::new(Notify::new());
    let engine = Engine {
        owner_release: Some(release.clone()),
        ..Default::default()
    };
    let worker = context
        .start_transcription(Arc::new(engine.clone()), Arc::new(Auth), Default::default())
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let input = batch_capture(&context, &service, true).await;
    notified(&engine.entered).await;
    assert_eq!(
        service
            .control_status(&input.request_id)
            .await
            .unwrap()
            .actions,
        vec![LocalCaptureAction::Cancel]
    );
    let cancelling = service.cancel(input.request_id.clone()).await.unwrap();
    assert_eq!(cancelling.status.state, LocalCaptureState::Cancelling);
    assert!(cancelling.actions.is_empty());
    assert!(context.try_session_lifecycle_change("note").is_err());
    let record = service
        .state
        .lock()
        .await
        .get(&input.request_id)
        .unwrap()
        .record
        .clone();
    assert!(record.cancel_requested);
    assert!(!record.processing_drained);
    // A separate stopped snapshot has no live capability and cannot infer a drain.
    let second = tempfile::tempdir().unwrap();
    let second_root = second.path().canonicalize().unwrap();
    let second_context = super::tests::context(&second_root).await;
    let directory = second_root.join("operations/local-capture-intents");
    journal::persist(&directory, &record, true).unwrap();
    let restored = LocalCaptureIntents::open(second_context.clone())
        .await
        .unwrap();
    let snapshot = restored.control_status(&input.request_id).await.unwrap();
    assert_eq!(snapshot.status.state, LocalCaptureState::Review);
    assert!(snapshot.actions.is_empty());
    assert!(restored.recover(input.request_id.clone()).await.is_err());
    assert!(
        second_context
            .ensure_session_jobs_idle("note")
            .await
            .is_err()
    );
    restored.shutdown().await;
    engine.release.notify_one();
    notified(&engine.returned).await;
    assert_eq!(
        service.status(&input.request_id).await.unwrap().state,
        LocalCaptureState::Cancelling
    );
    assert!(context.try_session_lifecycle_change("note").is_err());
    release.notify_one();
    assert_eq!(
        terminal(&service, &input.request_id).await.state,
        LocalCaptureState::Cancelled
    );
    let record = service
        .state
        .lock()
        .await
        .get(&input.request_id)
        .unwrap()
        .record
        .clone();
    assert!(record.processing_drained && record.inference_drained);
    assert!(record.published_revision.is_none());
    assert!(record.status.summary_job_id.is_none());
    let retained = service.root.join(&input.request_id).join("result.json");
    assert_eq!(
        record.result_sha256,
        Some(ledger::hash(&std::fs::read(retained).unwrap()))
    );
    assert!(
        context
            .session_transcripts("note")
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        context.read_note("note").await.unwrap().markdown.as_deref(),
        Some("Notes")
    );
    assert_eq!(
        std::fs::read(context.store.vault_base().join("sessions/note/audio.wav")).unwrap(),
        b"original native audio"
    );
    assert!(context.try_session_lifecycle_change("note").is_ok());
    assert_eq!(
        service
            .cancel(input.request_id.clone())
            .await
            .unwrap()
            .status
            .state,
        LocalCaptureState::Cancelled
    );
    assert!(service.recover(input.request_id).await.is_err());
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    assert_eq!(summary.calls.load(Ordering::SeqCst), 0);
    service.shutdown().await;
    worker.shutdown().await.unwrap();
    summary_worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_capture_cancel_waits_for_actual_summary_child_and_preserves_saved_content() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let summary = Arc::new(Summary::default());
    let worker = context
        .start_processing(summary.clone(), Arc::new(Auth))
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let (input, _) = live_summary_capture(&context, &service, &path).await;
    notified(&summary.entered).await;
    let before = std::fs::read(path.join("vault/sessions/note/transcript.json")).unwrap();
    assert_eq!(
        service
            .cancel(input.request_id.clone())
            .await
            .unwrap()
            .status
            .state,
        LocalCaptureState::Cancelling
    );
    assert!(context.try_session_lifecycle_change("note").is_err());
    summary.release.notify_one();
    let terminal = terminal(&service, &input.request_id).await;
    assert_eq!(terminal.state, LocalCaptureState::Cancelled);
    assert_eq!(
        std::fs::read(path.join("vault/sessions/note/transcript.json")).unwrap(),
        before
    );
    assert!(!path.join("vault/sessions/note/summary.md").exists());
    assert_eq!(summary.calls.load(Ordering::SeqCst), 1);
    assert!(
        context
            .processing_job(SUMMARY_OWNER.into(), terminal.summary_job_id.unwrap())
            .await
            .unwrap()
            .state
            .is_terminal()
    );
    assert!(context.try_session_lifecycle_change("note").is_ok());
    service.shutdown().await;
    worker.shutdown().await.unwrap();
}

#[derive(Default)]
struct AdmissionAuth {
    entered: Notify,
    release: Notify,
}
impl TranscriptionAuthorizer for AdmissionAuth {
    fn observe<'a>(
        &'a self,
        work: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            Auth.observe(work).await
        })
    }
    fn authorize<'a>(
        &'a self,
        work: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        Auth.authorize(work)
    }
}
#[tokio::test]
async fn local_capture_cancel_winning_final_inference_admission_never_calls_native_engine() {
    let root = tempfile::tempdir().unwrap();
    let context = context(&root.path().canonicalize().unwrap()).await;
    let auth = Arc::new(AdmissionAuth::default());
    let engine = Engine::default();
    let worker = context
        .start_transcription(Arc::new(engine.clone()), auth.clone(), Default::default())
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let input = batch_capture(&context, &service, false).await;
    notified(&auth.entered).await;
    service.cancel(input.request_id.clone()).await.unwrap();
    auth.release.notify_one();
    assert_eq!(
        terminal(&service, &input.request_id).await.state,
        LocalCaptureState::Cancelled
    );
    assert_eq!(engine.prepares.load(Ordering::SeqCst), 1);
    assert_eq!(engine.calls.load(Ordering::SeqCst), 0);
    assert!(
        !service
            .root
            .join(&input.request_id)
            .join("result.json")
            .exists()
    );
    service.shutdown().await;
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_capture_cancel_winning_dispatch_fence_never_polls_preparation_or_summary_submission()
{
    let root = tempfile::tempdir().unwrap();
    let context = context(&root.path().canonicalize().unwrap()).await;
    let service = LocalCaptureIntents::open(context).await.unwrap();
    let input = declaration();
    let _token = service.declare(input.clone()).await.unwrap();
    {
        let mut state = service.state.lock().await;
        let entry = state.get_mut(&input.request_id).unwrap();
        entry.record.physical = Some(Ok(()));
        entry.record.persistence = Some(Ok(persistence()));
        entry.record.status.state = LocalCaptureState::WaitingForPersistence;
        entry.running = true;
    }
    let (_, gate) = service.processing_control(&input.request_id).await.unwrap();
    let held = gate.lock().await;
    let calls = AtomicUsize::new(0);
    let mut cancellation = Box::pin(service.cancel_owned(input.request_id.clone()));
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(cancellation.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    let mut preparation = Box::pin(
        service.dispatch_processing(&input.request_id, |_| {}, async {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
    );
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(preparation.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    drop(held);
    let (cancelled, prepared) = futures_util::future::join(cancellation, preparation).await;
    assert_eq!(
        cancelled.unwrap().status.state,
        LocalCaptureState::Cancelling
    );
    assert!(prepared.is_err());
    assert!(
        service
            .dispatch_processing(&input.request_id, |_| {}, async {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
            .await
            .is_err()
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        service
            .state
            .lock()
            .await
            .get(&input.request_id)
            .unwrap()
            .record
            .cancel_requested
    );
}

#[tokio::test]
async fn local_capture_controls_never_offer_unknown_producer_or_conflict_recovery() {
    let root = tempfile::tempdir().unwrap();
    let context = context(&root.path().canonicalize().unwrap()).await;
    let service = LocalCaptureIntents::open(context).await.unwrap();
    let input = declaration();
    let token = service.declare(input.clone()).await.unwrap();
    assert!(service.cancel(input.request_id.clone()).await.is_err());
    assert!(
        service
            .control_status(&input.request_id)
            .await
            .unwrap()
            .actions
            .is_empty()
    );
    token.not_started().await.unwrap();
    for value in [LocalCaptureState::Review, LocalCaptureState::Conflict] {
        service
            .update(&input.request_id, |record| record.status.state = value)
            .await
            .unwrap();
        assert!(
            service
                .control_status(&input.request_id)
                .await
                .unwrap()
                .actions
                .is_empty()
        );
    }
    assert!(
        service
            .controls(Some("another-session"))
            .await
            .unwrap()
            .is_empty()
    );
    service.shutdown().await;
}

#[tokio::test]
async fn local_capture_private_schema_preserves_legacy_without_inventing_cancel_or_drain_proof() {
    let root = tempfile::tempdir().unwrap();
    let context = context(&root.path().canonicalize().unwrap()).await;
    let service = LocalCaptureIntents::open(context).await.unwrap();
    let input = declaration();
    service
        .declare(input.clone())
        .await
        .unwrap()
        .not_started()
        .await
        .unwrap();
    let mut old = service
        .state
        .lock()
        .await
        .get(&input.request_id)
        .unwrap()
        .record
        .clone();
    old.version = 1;
    let mut json = serde_json::to_value(&old).unwrap();
    for key in [
        "cancel_requested",
        "processing_drained",
        "inference_drained",
        "summary_dispatch_pending",
        "summary_drained",
        "resolution",
    ] {
        json.as_object_mut().unwrap().remove(key);
    }
    let other = tempfile::tempdir().unwrap();
    let other_root = other.path().canonicalize().unwrap();
    let directory = other_root.join(&input.request_id);
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(
        directory.join("intent.json"),
        serde_json::to_vec(&json).unwrap(),
    )
    .unwrap();
    let loaded = journal::load(&other_root, "vault").unwrap();
    assert_eq!(loaded[0].version, 1);
    assert_eq!(
        std::fs::read(directory.join("intent.json")).unwrap(),
        serde_json::to_vec(&json).unwrap()
    );
    assert!(
        !loaded[0].cancel_requested
            && !loaded[0].processing_drained
            && !loaded[0].inference_drained
    );
    assert_eq!(loaded[0].status.state, LocalCaptureState::NotStarted);
    let mut future = loaded[0].clone();
    future.version = 4;
    assert!(journal::persist(&other_root, &future, false).is_err());
    service.shutdown().await;
}

#[tokio::test]
async fn local_capture_controls_do_not_offer_recovery_for_a_terminal_cancelled_summary_child() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let summary = Arc::new(Summary::default());
    let worker = context
        .start_processing(summary.clone(), Arc::new(Auth))
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let (input, _) = live_summary_capture(&context, &service, &path).await;
    notified(&summary.entered).await;
    let child = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(child) = service
                .status(&input.request_id)
                .await
                .unwrap()
                .summary_job_id
            {
                break child;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    context
        .cancel_processing_job(SUMMARY_OWNER.into(), child)
        .await
        .unwrap();
    summary.release.notify_one();
    assert_eq!(
        terminal(&service, &input.request_id).await.state,
        LocalCaptureState::Review
    );
    assert!(
        service
            .control_status(&input.request_id)
            .await
            .unwrap()
            .actions
            .is_empty()
    );
    assert!(service.recover(input.request_id.clone()).await.is_err());
    service
        .update(&input.request_id, |record| {
            record.status.summary_job_id = None
        })
        .await
        .unwrap();
    assert!(
        service
            .control_status(&input.request_id)
            .await
            .unwrap()
            .actions
            .is_empty()
    );
    assert!(service.recover(input.request_id).await.is_err());
    assert_eq!(summary.calls.load(Ordering::SeqCst), 1);
    service.shutdown().await;
    worker.shutdown().await.unwrap();
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum PublicationPoint {
    BeforeCommit,
    AfterCommit,
    RecoveryBeforeCommit,
    RecoveryAfterCommit,
}
pub(super) struct PublicationPause {
    point: PublicationPoint,
    entered: Notify,
    release: Notify,
}
impl LocalCaptureIntents {
    pub(super) async fn pause_publication(&self, point: PublicationPoint) {
        let pause = self.publication_pause.lock().await.clone();
        if let Some(pause) = pause.filter(|pause| pause.point == point) {
            pause.entered.notify_one();
            pause.release.notified().await;
        }
    }
}

#[tokio::test]
async fn local_capture_cancel_after_preparation_prevents_transcript_commit_and_summary() {
    publication_cancellation(PublicationPoint::BeforeCommit).await;
}

#[tokio::test]
async fn local_capture_cancel_after_admitted_commit_preserves_publication_and_waits_for_receipt() {
    publication_cancellation(PublicationPoint::AfterCommit).await;
}

async fn publication_cancellation(point: PublicationPoint) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let summary = Arc::new(Summary::default());
    let summary_worker = context
        .start_processing(summary.clone(), Arc::new(Auth))
        .await
        .unwrap();
    let engine = Engine::default();
    let worker = context
        .start_transcription(Arc::new(engine.clone()), Arc::new(Auth), Default::default())
        .await
        .unwrap();
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let pause = Arc::new(PublicationPause {
        point,
        entered: Notify::new(),
        release: Notify::new(),
    });
    *service.publication_pause.lock().await = Some(pause.clone());
    let input = batch_capture(&context, &service, true).await;
    notified(&engine.entered).await;
    engine.release.notify_one();
    notified(&pause.entered).await;
    let retained = service.root.join(&input.request_id).join("result.json");
    let retained_bytes = std::fs::read(&retained).unwrap();
    assert!(!retained_bytes.is_empty());
    assert!(context.try_session_lifecycle_change("note").is_err());
    let admitted = point == PublicationPoint::AfterCommit;
    let canonical = context.session_transcripts("note").await.unwrap();
    assert_eq!(canonical.len(), usize::from(admitted));
    if admitted {
        // Poll the actual cancellation waiter into the held commit gate. It must
        // not acknowledge cancellation before the owned publication releases it.
        let mut cancelling = Box::pin(service.cancel_owned(input.request_id.clone()));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(cancelling.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        {
            let state = service.state.lock().await;
            let record = &state.get(&input.request_id).unwrap().record;
            assert!(!record.cancel_requested);
            assert!(record.published_revision.is_some());
        }
        pause.release.notify_one();
        assert_eq!(
            cancelling.await.unwrap().status.state,
            LocalCaptureState::Cancelling
        );
    } else {
        let cancelled = service.cancel(input.request_id.clone()).await.unwrap();
        assert_eq!(cancelled.status.state, LocalCaptureState::Cancelling);
        assert!(
            context
                .session_transcripts("note")
                .await
                .unwrap()
                .is_empty()
        );
        pause.release.notify_one();
    }
    assert_eq!(
        terminal(&service, &input.request_id).await.state,
        LocalCaptureState::Cancelled
    );
    let state = service.state.lock().await;
    let record = &state.get(&input.request_id).unwrap().record;
    assert!(record.cancel_requested && record.processing_drained && record.inference_drained);
    assert_eq!(record.published_revision.is_some(), admitted);
    assert!(record.status.summary_job_id.is_none());
    assert_eq!(record.result_sha256, Some(ledger::hash(&retained_bytes)));
    drop(state);
    assert_eq!(std::fs::read(retained).unwrap(), retained_bytes);
    let after = context.session_transcripts("note").await.unwrap();
    assert_eq!(
        serde_json::to_value(&canonical).unwrap(),
        serde_json::to_value(&after).unwrap()
    );
    assert_eq!(
        std::fs::read(context.store.vault_base().join("sessions/note/audio.wav")).unwrap(),
        b"original native audio"
    );
    assert_eq!(
        context.read_note("note").await.unwrap().markdown.as_deref(),
        Some("Notes")
    );
    assert_eq!(engine.calls.load(Ordering::SeqCst), 1);
    assert_eq!(summary.calls.load(Ordering::SeqCst), 0);
    assert!(context.try_session_lifecycle_change("note").is_ok());
    service.shutdown().await;
    worker.shutdown().await.unwrap();
    summary_worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn local_capture_cancel_before_retained_publication_commit_preserves_original_content() {
    retained_publication_cancellation(PublicationPoint::RecoveryBeforeCommit).await;
}
#[tokio::test]
async fn local_capture_cancel_after_retained_publication_commit_waits_for_original_receipt() {
    retained_publication_cancellation(PublicationPoint::RecoveryAfterCommit).await;
}
async fn retained_publication_cancellation(point: PublicationPoint) {
    use hypr_vault_write::transcript_publication::TranscriptSourcePreparation;
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let mut input = declaration();
    input.batch_requested = true;
    input.transcription = Some(LocalTranscriptionOptions {
        model: "frozen-model".into(),
        languages: vec!["nl".into()],
        keywords: vec![],
    });
    let _token = service.declare(input.clone()).await.unwrap();
    std::fs::write(
        path.join("vault/sessions/note/audio.wav"),
        b"original native audio",
    )
    .unwrap();
    let Some(TranscriptSourcePreparation::Ready(source)) = context
        .store
        .prepare_transcript_replacement_source("note")
        .await
        .unwrap()
    else {
        panic!("source unavailable")
    };
    let transcript: TranscriptWithData = serde_json::from_value(serde_json::json!({"id":uuid::Uuid::new_v4().to_string(),"session_id":"note","user_id":TRANSCRIPTION_OWNER,"created_at":"2026-11-08T00:00:00Z","started_at":0.0,"ended_at":null,"memo_md":"","words":[{"id":uuid::Uuid::new_v4().to_string(),"text":"retained","start_ms":0.0,"end_ms":1.0,"channel":0.0,"speaker":null,"metadata":null}],"speaker_hints":[]})).unwrap();
    let bytes = ledger::encode_result(&transcript).unwrap();
    ledger::write_new(
        &service.root.join(&input.request_id).join("result.json"),
        &bytes,
    )
    .unwrap();
    let binding = context
        .store
        .guard_processing_source("note", None)
        .await
        .unwrap()
        .unwrap()
        .binding()
        .clone();
    service
        .update(&input.request_id, |record| {
            record.physical = Some(Ok(()));
            record.persistence = Some(Ok(persistence()));
            record.source = Some(binding);
            record.inference_dispatched = true;
            record.inference_drained = true;
            record.result_sha256 = Some(ledger::hash(&bytes));
            record.status.state = LocalCaptureState::Transcribing;
        })
        .await
        .unwrap();
    service
        .state
        .lock()
        .await
        .get_mut(&input.request_id)
        .unwrap()
        .running = true;
    // A durable prepared transaction survives without an in-memory plan owner.
    let plan = context
        .store
        .prepare_transcript_publication(
            &path.join("operations/transcript-publications"),
            service.publication_identity(&input.request_id).unwrap(),
            source.baseline(),
            transcript,
        )
        .await
        .unwrap();
    drop(plan);
    drop(source);
    let pause = Arc::new(PublicationPause {
        point,
        entered: Notify::new(),
        release: Notify::new(),
    });
    *service.publication_pause.lock().await = Some(pause.clone());
    let owner = service.clone();
    let request = input.request_id.clone();
    let recovery = tokio::spawn(async move { owner.recover_publication(&request).await });
    notified(&pause.entered).await;
    let committed = point == PublicationPoint::RecoveryAfterCommit;
    assert_eq!(
        context.session_transcripts("note").await.unwrap().len(),
        usize::from(committed)
    );
    if committed {
        let mut cancellation = Box::pin(service.cancel_owned(input.request_id.clone()));
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(cancellation.as_mut().poll(cx)))
                .await
                .is_pending()
        );
        assert!(
            !service
                .state
                .lock()
                .await
                .get(&input.request_id)
                .unwrap()
                .record
                .cancel_requested
        );
        pause.release.notify_one();
        cancellation.await.unwrap();
    } else {
        service.cancel(input.request_id.clone()).await.unwrap();
        pause.release.notify_one();
    }
    let result = recovery.await.unwrap();
    assert_eq!(result.is_ok(), committed);
    let state = service.state.lock().await;
    let record = &state.get(&input.request_id).unwrap().record;
    assert!(record.cancel_requested);
    assert_eq!(record.published_revision.is_some(), committed);
    assert_eq!(record.result_sha256, Some(ledger::hash(&bytes)));
    drop(state);
    assert_eq!(
        context.session_transcripts("note").await.unwrap().len(),
        usize::from(committed)
    );
    assert_eq!(
        std::fs::read(service.root.join(&input.request_id).join("result.json")).unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read(path.join("vault/sessions/note/audio.wav")).unwrap(),
        b"original native audio"
    );
    assert_eq!(
        context.read_note("note").await.unwrap().markdown.as_deref(),
        Some("Notes")
    );
    let mut state = service.state.lock().await;
    let entry = state.get_mut(&input.request_id).unwrap();
    entry.running = false;
    entry.admission.take();
    drop(state);
    service.shutdown().await;
}

fn resolution_request(snapshot: &LocalCaptureControlSnapshot) -> LocalCaptureResolution {
    LocalCaptureResolution {
        resolution_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: snapshot.revision.clone(),
        decision: LocalCaptureResolutionDecision::KeepCurrentContent,
    }
}
fn fixture_files(root: &std::path::Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(
        root: &std::path::Path,
        directory: &std::path::Path,
        files: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, files);
            } else {
                files.insert(
                    path.strip_prefix(root).unwrap().to_owned(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[tokio::test]
async fn local_capture_reviewed_conflict_resolution_preserves_content_and_replays_after_reopen() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let (input, resolution, closed) = {
        let context = context(&path).await;
        let summary = Arc::new(Summary::default());
        let worker = context
            .start_processing(summary.clone(), Arc::new(Auth))
            .await
            .unwrap();
        let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
        let (input, token) = live_summary_capture(&context, &service, &path).await;
        notified(&summary.entered).await;
        assert_eq!(context.store.ensure_summary("note").await.unwrap(), "");
        context
            .store
            .save_summary("note", "User-edited summary", Some(""), None)
            .await
            .unwrap();
        context
            .store
            .save_note("note", "Current note ![](attachments/kept.png)", None)
            .await
            .unwrap();
        let attachment = path.join("vault/sessions/note/attachments");
        std::fs::create_dir_all(&attachment).unwrap();
        std::fs::write(attachment.join("kept.png"), b"user attachment").unwrap();
        std::fs::write(
            path.join("vault/sessions/note/.private"),
            b"unknown private bytes",
        )
        .unwrap();
        summary.release.notify_one();
        assert_eq!(
            terminal(&service, &input.request_id).await.state,
            LocalCaptureState::Conflict
        );
        let snapshot = service.control_status(&input.request_id).await.unwrap();
        assert_eq!(
            snapshot.actions,
            vec![LocalCaptureAction::KeepCurrentContent]
        );
        let stale = resolution_request(&snapshot);
        service
            .update(&input.request_id, |record| {
                record.status.message = Some("Reviewed conflict detail".into())
            })
            .await
            .unwrap();
        let refused = service
            .resolve(input.request_id.clone(), stale)
            .await
            .unwrap_err();
        assert_eq!(refused.code, ErrorCode::Conflict);
        assert_eq!(
            refused.mutation_outcome,
            Some(hypr_loofah_protocol::MutationOutcome::NotCommitted)
        );
        assert_eq!(
            refused.current_revision,
            Some(
                service
                    .control_status(&input.request_id)
                    .await
                    .unwrap()
                    .revision
            )
        );
        let snapshot = service.control_status(&input.request_id).await.unwrap();
        let resolution = resolution_request(&snapshot);
        let mut before = fixture_files(&path);
        let closed = service
            .resolve(input.request_id.clone(), resolution.clone())
            .await
            .unwrap();
        assert_eq!(closed.status.state, LocalCaptureState::Closed);
        assert!(closed.actions.is_empty());
        assert_eq!(closed.resolution.as_ref(), Some(&resolution));
        let closed_record = service
            .state
            .lock()
            .await
            .get(&input.request_id)
            .unwrap()
            .record
            .clone();
        for invalid in 0..4 {
            let mut record = closed_record.clone();
            match invalid {
                0 => record.version = 2,
                1 => record.resolution = None,
                2 => record.processing_drained = false,
                _ => {
                    record.resolution.as_mut().unwrap().expected_revision = "not-a-revision".into()
                }
            }
            assert!(journal::persist(&service.root, &record, false).is_err());
        }
        let mut after = fixture_files(&path);
        let intent = PathBuf::from(format!(
            "operations/local-capture-intents/{}/intent.json",
            input.request_id
        ));
        assert_ne!(before.remove(&intent), after.remove(&intent));
        assert_eq!(before, after);
        assert_eq!(summary.calls.load(Ordering::SeqCst), 1);
        // Closing cannot revive an old producer capability or replay processing.
        token.physical_finished(Ok(())).await.unwrap();
        assert!(token.not_started().await.is_err());
        assert_eq!(
            service
                .recover(input.request_id.clone())
                .await
                .unwrap()
                .state,
            LocalCaptureState::Closed
        );
        let mut changed = resolution.clone();
        changed.resolution_id = uuid::Uuid::new_v4().to_string();
        assert!(
            service
                .resolve(input.request_id.clone(), changed)
                .await
                .is_err()
        );
        let mut changed = resolution.clone();
        changed.expected_revision = closed.revision.clone();
        assert!(
            service
                .resolve(input.request_id.clone(), changed)
                .await
                .is_err()
        );
        context
            .store
            .save_note("note", "An unrelated later note edit", None)
            .await
            .unwrap();
        let before = fixture_files(&path);
        assert_eq!(
            serde_json::to_value(
                service
                    .resolve(input.request_id.clone(), resolution.clone())
                    .await
                    .unwrap()
            )
            .unwrap(),
            serde_json::to_value(&closed).unwrap()
        );
        assert_eq!(before, fixture_files(&path));
        service.shutdown().await;
        worker.shutdown().await.unwrap();
        drop(token);
        (input, resolution, closed)
    };
    let context = context(&path).await;
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let before = fixture_files(&path);
    let replayed = service.resolve(input.request_id, resolution).await.unwrap();
    assert_eq!(
        serde_json::to_value(&replayed).unwrap(),
        serde_json::to_value(&closed).unwrap()
    );
    assert_eq!(before, fixture_files(&path));
    assert_eq!(
        context.read_note("note").await.unwrap().markdown.as_deref(),
        Some("An unrelated later note edit")
    );
    service.shutdown().await;
}

#[tokio::test]
async fn local_capture_resolution_refuses_unknown_or_live_ownership_and_changed_session_generation()
{
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let context = context(&path).await;
    let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
    let input = declaration();
    let token = service.declare(input.clone()).await.unwrap();
    token.not_started().await.unwrap();
    let initial = service
        .state
        .lock()
        .await
        .get(&input.request_id)
        .unwrap()
        .record
        .clone();
    for (state, physical, persistence, processing_drained, pending_child, running, admitted) in [
        (
            LocalCaptureState::Review,
            None,
            None,
            false,
            false,
            false,
            false,
        ),
        (
            LocalCaptureState::Conflict,
            None,
            None,
            true,
            false,
            false,
            false,
        ),
        (
            LocalCaptureState::Conflict,
            Some(Ok(())),
            Some(Ok(super::tests::persistence())),
            false,
            false,
            false,
            false,
        ),
        (
            LocalCaptureState::Conflict,
            Some(Ok(())),
            Some(Ok(super::tests::persistence())),
            true,
            true,
            false,
            false,
        ),
        (
            LocalCaptureState::Conflict,
            Some(Ok(())),
            Some(Ok(super::tests::persistence())),
            true,
            false,
            true,
            false,
        ),
        (
            LocalCaptureState::Conflict,
            Some(Ok(())),
            Some(Ok(super::tests::persistence())),
            true,
            false,
            false,
            true,
        ),
    ] {
        let admission = if admitted {
            Some(Arc::new(context.admit_session_work("note").await.unwrap()))
        } else {
            None
        };
        {
            let mut values = service.state.lock().await;
            let entry = values.get_mut(&input.request_id).unwrap();
            entry.record = initial.clone();
            entry.record.status.state = state;
            entry.record.physical = physical;
            entry.record.persistence = persistence;
            entry.record.processing_drained = processing_drained;
            entry.record.summary_dispatch_pending = pending_child;
            entry.running = running;
            entry.admission = admission;
        }
        let snapshot = service.control_status(&input.request_id).await.unwrap();
        assert!(
            !snapshot
                .actions
                .contains(&LocalCaptureAction::KeepCurrentContent)
        );
        assert!(
            service
                .resolve(input.request_id.clone(), resolution_request(&snapshot))
                .await
                .is_err()
        );
        assert!(
            service
                .state
                .lock()
                .await
                .get(&input.request_id)
                .unwrap()
                .record
                .resolution
                .is_none()
        );
    }
    {
        let mut state = service.state.lock().await;
        let entry = state.get_mut(&input.request_id).unwrap();
        entry.admission.take();
        entry.running = false;
        entry.record.session_created_at = "2000-01-01T00:00:00Z".into();
    }
    let snapshot = service.control_status(&input.request_id).await.unwrap();
    assert_eq!(
        snapshot.actions,
        vec![LocalCaptureAction::KeepCurrentContent]
    );
    let before = fixture_files(&path);
    assert!(
        service
            .resolve(input.request_id, resolution_request(&snapshot))
            .await
            .is_err()
    );
    assert_eq!(before, fixture_files(&path));
    service.shutdown().await;
}
