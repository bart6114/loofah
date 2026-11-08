use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::jobs::{GeneratedDocument, JobFuture, JobWork};
use hypr_loofah_protocol::{
    DocumentTarget, ProcessingJobFailure, ProcessingJobSnapshot, ProcessingJobState,
    SummaryJobRequest,
};
use tokio::sync::Semaphore;

struct Generator {
    calls: AtomicUsize,
    cancelled: AtomicBool,
    hold: bool,
    panic: bool,
    release: Semaphore,
}

impl Generator {
    fn new(hold: bool, panic: bool) -> Arc<Self> {
        Arc::new(Self {
            calls: AtomicUsize::new(0),
            cancelled: AtomicBool::new(false),
            hold,
            panic,
            release: Semaphore::new(0),
        })
    }
}

impl JobGenerator for Generator {
    fn generate(
        &self,
        _: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(
                !self.panic,
                "private provider diagnostic must not escape shutdown"
            );
            if self.hold {
                cancel.cancelled().await;
                self.cancelled.store(true, Ordering::SeqCst);
                self.release.acquire().await.unwrap().forget();
            }
            Ok(GeneratedDocument {
                markdown: "# Runtime result".into(),
                suggested_tags: None,
            })
        })
    }
}

struct TrustedLocal;

impl JobAuthorizer for TrustedLocal {
    fn authorize<'a>(
        &'a self,
        _: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async { Ok(Box::new(()) as Box<dyn Send>) })
    }
}

async fn open(root: &Path) -> Result<VaultRuntime, StoreError> {
    VaultRuntime::open_with_changes(
        "vault-one".into(),
        &root.join("vault"),
        &root.join("cache"),
        &root.join("operations"),
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
}

async fn note(context: &VaultContext) -> SummaryJobRequest {
    let meta = serde_json::from_value(serde_json::json!({
        "id":"note", "title":"Personal note", "created_at":"2026-11-07T10:00:00Z", "tags":[]
    }))
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Keep the original note.")
        .await
        .unwrap();
    context.store.ensure_summary("note").await.unwrap();
    SummaryJobRequest {
        only_if_empty: false,
        title_policy: Default::default(),
        session_id: "note".into(),
        target: DocumentTarget::Summary,
        expected_revision: context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .revision,
    }
}

async fn until(mut check: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !check() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn terminal(journal: &JobJournal, id: &str) -> ProcessingJobSnapshot {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let job = journal.get(id.into()).await.unwrap().unwrap();
            if job.state.is_terminal() {
                break job;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn processing_is_opt_in_and_completed_work_survives_runtime_restart_without_regeneration() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = open(root.path()).await.unwrap();
    assert_eq!(
        runtime.context.processing_jobs().err().unwrap().code,
        ErrorCode::Unavailable
    );
    let request = note(&runtime.context).await;
    let generator = Generator::new(false, false);
    runtime
        .enable_processing(generator.clone(), Arc::new(TrustedLocal))
        .await
        .unwrap();
    assert_eq!(
        runtime
            .enable_processing(generator.clone(), Arc::new(TrustedLocal))
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let key = uuid::Uuid::new_v4().to_string();
    let journal = runtime.context.processing_jobs().unwrap().clone();
    let job = journal
        .submit("trusted-device".into(), key.clone(), request.clone())
        .await
        .unwrap();
    assert_eq!(
        terminal(&journal, &job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(
        runtime
            .context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .unwrap()
            .markdown,
        "# Runtime result"
    );
    let stale = runtime.context.clone();
    runtime.shutdown().await.unwrap();
    assert_eq!(stale.startup.snapshot().phase, StartupPhase::Stopped);
    assert_eq!(
        stale.processing_jobs().err().unwrap().code,
        ErrorCode::Unavailable
    );
    assert_eq!(
        stale
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    drop(stale);
    drop(journal);
    let mut reopened = open(root.path()).await.unwrap();
    reopened
        .enable_processing(generator.clone(), Arc::new(TrustedLocal))
        .await
        .unwrap();
    let journal = reopened.context.processing_jobs().unwrap();
    let replay = journal
        .submit("trusted-device".into(), key, request)
        .await
        .unwrap();
    assert_eq!(replay.id, job.id);
    assert_eq!(replay.state, ProcessingJobState::Succeeded);
    assert_eq!(
        journal.result(job.id).await.unwrap().unwrap().markdown,
        "# Runtime result"
    );
    assert_eq!(generator.calls.load(Ordering::SeqCst), 1);
    reopened.shutdown().await.unwrap();
}

#[tokio::test]
async fn graceful_shutdown_drains_processing_and_retains_late_output_without_applying_it() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = open(root.path()).await.unwrap();
    let request = note(&runtime.context).await;
    let generator = Generator::new(true, false);
    runtime
        .enable_processing(generator.clone(), Arc::new(TrustedLocal))
        .await
        .unwrap();
    let journal = runtime.context.processing_jobs().unwrap().clone();
    let finished = runtime.context.processing.get().unwrap().finished.clone();
    let job = journal
        .submit("device".into(), uuid::Uuid::new_v4().to_string(), request)
        .await
        .unwrap();
    until(|| generator.calls.load(Ordering::SeqCst) == 1).await;
    let shutting_down = tokio::spawn(runtime.shutdown());
    until(|| generator.cancelled.load(Ordering::SeqCst)).await;
    assert!(!shutting_down.is_finished());
    assert!(!finished.is_cancelled());
    generator.release.add_permits(1);
    shutting_down.await.unwrap().unwrap();
    assert!(finished.is_cancelled());
    assert_eq!(
        terminal(&journal, &job.id).await.state,
        ProcessingJobState::Interrupted
    );
    assert_eq!(
        journal.result(job.id).await.unwrap().unwrap().markdown,
        "# Runtime result"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("vault/sessions/note/summary.md")).unwrap(),
        ""
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("vault/sessions/note/notes.md")).unwrap(),
        "Keep the original note."
    );
}

#[tokio::test]
async fn shutdown_flushes_transcript_before_timeout_and_keeps_vault_owned_until_processing_drains()
{
    let root = tempfile::tempdir().unwrap();
    let mut runtime = open(root.path()).await.unwrap();
    let request = note(&runtime.context).await;
    let generator = Generator::new(true, false);
    runtime
        .enable_processing(generator.clone(), Arc::new(TrustedLocal))
        .await
        .unwrap();
    let journal = runtime.context.processing_jobs().unwrap().clone();
    let job = journal
        .submit("device".into(), uuid::Uuid::new_v4().to_string(), request)
        .await
        .unwrap();
    until(|| generator.calls.load(Ordering::SeqCst) == 1).await;
    let delta = serde_json::from_value(serde_json::json!({
        "transcript_id":"capture", "started_at_ms":1000.0,
        "new_words":[{"id":"w1","text":"Buffered recording", "start_ms":0.0,"end_ms":10.0,"channel":0.0,"speaker":null,"metadata":null}],
        "replaced_ids":[], "new_hints":[]
    })).unwrap();
    runtime
        .context
        .store
        .append_transcript("note", delta)
        .await
        .unwrap();
    let weak_context = Arc::downgrade(&runtime.context);
    let shutting_down = tokio::spawn(runtime.shutdown());
    until(|| generator.cancelled.load(Ordering::SeqCst)).await;
    let transcript = root.path().join("vault/sessions/note/transcript.json");
    until(|| {
        std::fs::read_to_string(&transcript).is_ok_and(|text| text.contains("Buffered recording"))
    })
    .await;
    assert!(!shutting_down.is_finished());
    let error = shutting_down.await.unwrap().unwrap_err();
    assert!(
        matches!(error, StoreError::Io(ref message) if message == "processing worker exceeded shutdown deadline")
    );
    assert!(weak_context.upgrade().is_some());
    assert!(matches!(
        open(root.path()).await,
        Err(StoreError::Conflict(_))
    ));
    generator.release.add_permits(1);
    assert_eq!(
        terminal(&journal, &job.id).await.state,
        ProcessingJobState::Interrupted
    );
    assert!(journal.result(job.id).await.unwrap().is_some());
    until(|| weak_context.upgrade().is_none()).await;
    drop(journal);
    let runtime = open(root.path()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("vault/sessions/note/summary.md")).unwrap(),
        ""
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropping_runtime_marks_retained_context_unavailable_and_drains_actual_processing() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = open(root.path()).await.unwrap();
    let request = note(&runtime.context).await;
    let generator = Generator::new(true, false);
    runtime
        .enable_processing(generator.clone(), Arc::new(TrustedLocal))
        .await
        .unwrap();
    let context = runtime.context.clone();
    let weak = Arc::downgrade(&context);
    let journal = context.processing_jobs().unwrap().clone();
    let job = journal
        .submit("device".into(), uuid::Uuid::new_v4().to_string(), request)
        .await
        .unwrap();
    until(|| generator.calls.load(Ordering::SeqCst) == 1).await;
    drop(runtime);
    assert_eq!(context.startup.snapshot().phase, StartupPhase::Stopping);
    assert_eq!(
        context.processing_jobs().err().unwrap().code,
        ErrorCode::Unavailable
    );
    assert_eq!(
        context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    drop(context);
    until(|| generator.cancelled.load(Ordering::SeqCst)).await;
    assert!(weak.upgrade().is_some());
    assert!(matches!(
        open(root.path()).await,
        Err(StoreError::Conflict(_))
    ));
    generator.release.add_permits(1);
    assert_eq!(
        terminal(&journal, &job.id).await.state,
        ProcessingJobState::Interrupted
    );
    until(|| weak.upgrade().is_none()).await;
    drop(journal);
    open(root.path()).await.unwrap().shutdown().await.unwrap();
}

#[tokio::test]
async fn crashed_worker_disables_processing_but_preserves_notes_and_recovers_interrupted_status() {
    let root = tempfile::tempdir().unwrap();
    let mut runtime = open(root.path()).await.unwrap();
    let request = note(&runtime.context).await;
    runtime
        .enable_processing(Generator::new(false, true), Arc::new(TrustedLocal))
        .await
        .unwrap();
    let journal = runtime.context.processing_jobs().unwrap().clone();
    let job = journal
        .submit("device".into(), uuid::Uuid::new_v4().to_string(), request)
        .await
        .unwrap();
    until(|| runtime.context.processing_jobs().is_err()).await;
    assert!(
        runtime
            .context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .is_ok()
    );
    assert_eq!(
        journal.get(job.id.clone()).await.unwrap().unwrap().state,
        ProcessingJobState::Running
    );
    assert_eq!(
        runtime
            .context
            .processing_job("device".into(), job.id.clone())
            .await
            .unwrap()
            .state,
        ProcessingJobState::Interrupted
    );
    assert!(
        runtime
            .context
            .list_processing_jobs("device".into(), Default::default())
            .await
            .is_ok()
    );
    assert!(
        runtime
            .context
            .processing_job_result("device".into(), job.id.clone())
            .await
            .unwrap()
            .is_none()
    );
    let error = runtime.shutdown().await.unwrap_err();
    assert!(
        matches!(error, StoreError::Io(ref message) if message == "processing worker did not stop cleanly")
    );
    drop(journal);
    let mut reopened = open(root.path()).await.unwrap();
    let generator = Generator::new(false, false);
    reopened
        .enable_processing(generator.clone(), Arc::new(TrustedLocal))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .context
            .processing_jobs()
            .unwrap()
            .get(job.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        ProcessingJobState::Interrupted
    );
    assert_eq!(generator.calls.load(Ordering::SeqCst), 0);
    reopened.shutdown().await.unwrap();
}
