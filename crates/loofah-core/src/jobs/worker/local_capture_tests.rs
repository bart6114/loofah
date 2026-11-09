use super::*;
use crate::{
    local_capture_intents::{
        LocalCaptureDeclaration, LocalCaptureIntent, LocalCaptureIntents, LocalCapturePersistence,
        LocalCaptureState, LocalTranscriptionOptions, SUMMARY_OWNER, TRANSCRIPTION_OWNER,
    },
    search::SearchService,
    settings::ConfigState,
    startup::StartupState,
};
use hypr_loofah_protocol::{CaptureProcessingSummary, ProcessingTitleStatus};
use std::{
    collections::HashMap,
    sync::{
        Mutex, OnceLock, Weak,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::sync::Semaphore;

#[derive(Default)]
struct Pauses(Mutex<HashMap<(String, &'static str), Weak<Pause>>>);
static PAUSES: OnceLock<Pauses> = OnceLock::new();
struct Pause {
    entered: Semaphore,
    release: Semaphore,
}
impl Pause {
    fn install(session: &str, stage: &'static str) -> Arc<Self> {
        let pause = Arc::new(Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
        });
        PAUSES
            .get_or_init(Pauses::default)
            .0
            .lock()
            .unwrap()
            .insert((session.into(), stage), Arc::downgrade(&pause));
        pause
    }
    async fn entered(&self) {
        bounded(self.entered.acquire()).await.unwrap().forget();
    }
    fn release(&self) {
        self.release.add_permits(1);
    }
}
pub(crate) async fn checkpoint(session: &str, stage: &'static str) {
    let pause = PAUSES
        .get_or_init(Pauses::default)
        .0
        .lock()
        .unwrap()
        .remove(&(session.into(), stage))
        .and_then(|value| value.upgrade());
    if let Some(pause) = pause {
        pause.entered.add_permits(1);
        pause.release.acquire().await.unwrap().forget();
    }
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(std::time::Duration::from_secs(10), future)
        .await
        .unwrap()
}
struct Auth;
impl JobAuthorizer for Auth {
    fn authorize<'a>(
        &'a self,
        work: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async move {
            checkpoint(&work.request.session_id, "authorization").await;
            Ok(Box::new(()) as Box<dyn Send>)
        })
    }
}
const BODY: &str = "# Generated body\n\n- [ ] Keep this action.";
#[derive(Default)]
struct Model {
    summaries: AtomicUsize,
    titles: Arc<AtomicUsize>,
    title_drain: Option<Arc<TitleDrain>>,
}
struct TitleDrain {
    entered: Semaphore,
    cancelled: Semaphore,
    release: Semaphore,
}
impl JobGenerator for Model {
    fn generate(
        &self,
        _: JobWork,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>> {
        self.summaries.fetch_add(1, Ordering::SeqCst);
        Box::pin(async {
            Ok(GeneratedDocument {
                markdown: BODY.into(),
                suggested_tags: None,
            })
        })
    }
    fn generate_prepared(
        &self,
        work: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<PreparedGeneration, ProcessingJobFailure>> {
        Box::pin(async move {
            Ok(PreparedGeneration {
                document: self.generate(work, cancel).await?,
                title: Some(Box::new(Title(
                    self.titles.clone(),
                    self.title_drain.clone(),
                ))),
            })
        })
    }
}
struct Title(Arc<AtomicUsize>, Option<Arc<TitleDrain>>);
impl TitleRunner for Title {
    fn generate(
        self: Box<Self>,
        body: String,
        cancel: CancellationToken,
    ) -> JobFuture<'static, Result<Option<String>, ProcessingJobFailure>> {
        self.0.fetch_add(1, Ordering::SeqCst);
        assert_eq!(body, BODY);
        Box::pin(async move {
            if let Some(drain) = self.1 {
                drain.entered.add_permits(1);
                cancel.cancelled().await;
                drain.cancelled.add_permits(1);
                drain.release.acquire().await.unwrap().forget();
            }
            Ok(Some("Retained title candidate".into()))
        })
    }
}
struct Fixture {
    _root: tempfile::TempDir,
    context: Arc<VaultContext>,
    service: Arc<LocalCaptureIntents>,
    worker: JobWorker,
    model: Arc<Model>,
    session: String,
    request: String,
    token: LocalCaptureIntent,
}
impl Fixture {
    async fn new(title: bool) -> Self {
        Self::with_model(title, Arc::new(Model::default())).await
    }
    async fn with_model(title: bool, model: Arc<Model>) -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let vault = root.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        let hypr_storage::vault::LeaseAttempt::Acquired(lease) =
            hypr_storage::vault::VaultLease::try_acquire_existing(&vault).unwrap()
        else {
            panic!("fixture lease")
        };
        let context = Arc::new(
            VaultContext::new(
                "local-capture-fence".into(),
                Arc::new(hypr_vault_write::SessionStore::new(vault.clone())),
                StartupState::new(Some(&vault)),
                Arc::new(ConfigState::load_or_default(&vault)),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(root.path().join("operations"))
            .with_vault_lease(lease)
            .unwrap(),
        );
        let session = uuid::Uuid::new_v4().to_string();
        let meta = serde_json::from_value(serde_json::json!({"id":session,"title":"","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
        context
            .store
            .create_note_session(&meta, "Original note")
            .await
            .unwrap();
        let worker = context
            .start_processing(model.clone(), Arc::new(Auth))
            .await
            .unwrap();
        let service = LocalCaptureIntents::open(context.clone()).await.unwrap();
        let request = uuid::Uuid::new_v4().to_string();
        let token = service
            .declare(LocalCaptureDeclaration {
                request_id: request.clone(),
                session_id: session.clone(),
                append: false,
                batch_requested: false,
                transcription: Some(LocalTranscriptionOptions {
                    model: "unused-live-fixture".into(),
                    languages: vec![],
                    keywords: vec![],
                }),
                summary: Some(CaptureProcessingSummary {
                    target: DocumentTarget::Summary,
                    title_policy: if title {
                        TitlePolicy::GenerateIfEmpty
                    } else {
                        TitlePolicy::Preserve
                    },
                }),
            })
            .await
            .unwrap();
        Self {
            _root: root,
            context,
            service,
            worker,
            model,
            session,
            request,
            token,
        }
    }
    async fn start(&self) {
        std::fs::write(
            self.context
                .store
                .vault_base()
                .join("sessions")
                .join(&self.session)
                .join("audio.wav"),
            b"original native recording",
        )
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let transcript = serde_json::from_value(serde_json::json!({
            "id":id,"session_id":self.session,"user_id":TRANSCRIPTION_OWNER,"created_at":"2026-11-08T10:00:00Z",
            "started_at":0.0,"ended_at":null,"memo_md":"","speaker_hints":[],
            "words": (0..5).map(|i| serde_json::json!({"id":uuid::Uuid::new_v4().to_string(),"text":"x".repeat(40),"start_ms":i as f64,"end_ms":i as f64+1.0,"channel":0.0,"speaker":null,"metadata":null})).collect::<Vec<_>>()
        })).unwrap();
        self.context
            .store
            .write_transcript(&self.session, transcript)
            .await
            .unwrap();
        self.token.physical_finished(Ok(())).await.unwrap();
        self.token
            .persistence_finished(Ok(LocalCapturePersistence {
                live_transcription_active: true,
                needs_batch_repair: false,
                new_transcript_id: Some(id),
            }))
            .await
            .unwrap();
    }
    async fn finish(&self) -> ProcessingJobSnapshot {
        bounded(async {
            loop {
                let parent = self.service.status(&self.request).await.unwrap();
                if parent.state == LocalCaptureState::Cancelled {
                    return self
                        .context
                        .processing_job(SUMMARY_OWNER.into(), parent.summary_job_id.unwrap())
                        .await
                        .unwrap();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
    }
    async fn summary(&self) -> Option<String> {
        self.context
            .read_document(&self.session, &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .map(|document| document.markdown)
    }
    async fn shutdown(self) {
        self.service.shutdown().await;
        self.worker.shutdown().await.unwrap();
    }
    async fn reopen_result(self, id: String, expected: hypr_loofah_protocol::ProcessingJobResult) {
        let events = self.context.events.clone();
        let directory = self
            .context
            .store
            .vault_base()
            .join("sessions")
            .join(&self.session);
        let before_meta = std::fs::read(directory.join("_meta.json")).unwrap();
        let before_body = std::fs::read(directory.join("summary.md")).unwrap();
        let Self {
            _root: root,
            context,
            service,
            worker,
            model,
            token,
            ..
        } = self;
        service.shutdown().await;
        worker.shutdown().await.unwrap();
        drop(token);
        drop(service);
        drop(context);
        let journal = JobJournal::open(root.path().join("operations/jobs"), events)
            .await
            .unwrap();
        journal
            .run(|state, events| state.interrupt_active(events))
            .await
            .unwrap();
        assert_eq!(
            journal
                .owned_result(SUMMARY_OWNER.into(), id)
                .await
                .unwrap(),
            Some(expected)
        );
        assert_eq!(
            std::fs::read(directory.join("_meta.json")).unwrap(),
            before_meta
        );
        assert_eq!(
            std::fs::read(directory.join("summary.md")).unwrap(),
            before_body
        );
        assert_eq!(model.summaries.load(Ordering::SeqCst), 1);
        assert_eq!(model.titles.load(Ordering::SeqCst), 1);
    }
}

#[tokio::test]
async fn local_parent_cancel_wins_summary_generator_after_source_and_authorization() {
    let f = Fixture::new(false).await;
    let pause = Pause::install(&f.session, "summary_generate");
    f.start().await;
    pause.entered().await;
    f.service.cancel(f.request.clone()).await.unwrap();
    pause.release();
    let job = f.finish().await;
    assert_eq!(job.state, ProcessingJobState::Cancelled);
    assert_eq!(f.model.summaries.load(Ordering::SeqCst), 0);
    assert!(f.summary().await.is_none_or(|body| body.is_empty()));
    assert!(
        f.context
            .processing_job_result(SUMMARY_OWNER.into(), job.id)
            .await
            .unwrap()
            .is_none()
    );
    f.shutdown().await;
}

#[tokio::test]
async fn local_parent_cancel_wins_summary_apply_after_source_and_retains_result() {
    let f = Fixture::new(false).await;
    let pause = Pause::install(&f.session, "summary_apply");
    f.start().await;
    pause.entered().await;
    f.service.cancel(f.request.clone()).await.unwrap();
    pause.release();
    let job = f.finish().await;
    assert_eq!(job.state, ProcessingJobState::Cancelled);
    assert_eq!(f.model.summaries.load(Ordering::SeqCst), 1);
    assert!(f.summary().await.is_none_or(|body| body.is_empty()));
    let result = f
        .context
        .processing_job_result(SUMMARY_OWNER.into(), job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.markdown, BODY);
    f.shutdown().await;
}

#[tokio::test]
async fn local_parent_cancel_wins_optional_title_model_admission() {
    let f = Fixture::new(true).await;
    let pause = Pause::install(&f.session, "title_generate");
    f.start().await;
    pause.entered().await;
    f.service.cancel(f.request.clone()).await.unwrap();
    pause.release();
    let job = f.finish().await;
    assert_eq!(job.state, ProcessingJobState::Succeeded);
    assert_eq!(f.summary().await.as_deref(), Some(BODY));
    assert_eq!(f.model.titles.load(Ordering::SeqCst), 0);
    let result = f
        .context
        .processing_job_result(SUMMARY_OWNER.into(), job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        result.title.unwrap().status,
        ProcessingTitleStatus::Cancelled
    );
    assert_eq!(
        f.context
            .read_metadata(&f.session)
            .await
            .unwrap()
            .meta
            .title,
        ""
    );
    f.shutdown().await;
}

#[tokio::test]
async fn local_parent_cancel_wins_title_metadata_apply_and_retains_candidate() {
    let f = Fixture::new(true).await;
    let pause = Pause::install(&f.session, "title_apply");
    f.start().await;
    pause.entered().await;
    f.service.cancel(f.request.clone()).await.unwrap();
    pause.release();
    let job = f.finish().await;
    assert_eq!(job.state, ProcessingJobState::Succeeded);
    assert_eq!(f.summary().await.as_deref(), Some(BODY));
    assert_eq!(f.model.titles.load(Ordering::SeqCst), 1);
    let title = f
        .context
        .processing_job_result(SUMMARY_OWNER.into(), job.id)
        .await
        .unwrap()
        .unwrap()
        .title
        .unwrap();
    assert_eq!(title.status, ProcessingTitleStatus::Cancelled);
    assert_eq!(title.candidate.as_deref(), Some("Retained title candidate"));
    assert_eq!(
        f.context
            .read_metadata(&f.session)
            .await
            .unwrap()
            .meta
            .title,
        ""
    );
    f.shutdown().await;
}

#[tokio::test]
async fn local_parent_cancel_waits_for_admitted_summary_write() {
    let f = Fixture::new(false).await;
    let pause = Pause::install(&f.session, "summary_apply_admitted");
    let written = Pause::install(&f.session, "summary_written");
    f.start().await;
    pause.entered().await;
    let service = f.service.clone();
    let request = f.request.clone();
    let mut cancellation = Box::pin(async move { service.cancel(request).await });
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(cancellation.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    pause.release();
    written.entered().await;
    bounded(cancellation).await.unwrap();
    written.release();
    let job = f.finish().await;
    assert_eq!(job.state, ProcessingJobState::Succeeded);
    assert_eq!(f.summary().await.as_deref(), Some(BODY));
    f.shutdown().await;
}

#[tokio::test]
async fn local_parent_cancel_waits_for_admitted_title_metadata_write() {
    let f = Fixture::new(true).await;
    let pause = Pause::install(&f.session, "title_apply_admitted");
    let written = Pause::install(&f.session, "title_written");
    f.start().await;
    pause.entered().await;
    let service = f.service.clone();
    let request = f.request.clone();
    let mut cancellation = Box::pin(async move { service.cancel(request).await });
    assert!(
        std::future::poll_fn(|cx| std::task::Poll::Ready(cancellation.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    pause.release();
    written.entered().await;
    bounded(cancellation).await.unwrap();
    written.release();
    let job = f.finish().await;
    assert_eq!(job.state, ProcessingJobState::Succeeded);
    assert_eq!(
        f.context
            .read_metadata(&f.session)
            .await
            .unwrap()
            .meta
            .title,
        "Retained title candidate"
    );
    let result = f
        .context
        .processing_job_result(SUMMARY_OWNER.into(), job.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.markdown, BODY);
    assert_eq!(result.title.unwrap().status, ProcessingTitleStatus::Applied);
    f.shutdown().await;
}

#[tokio::test]
async fn local_parent_cancel_during_source_and_authorization_never_dispatches_summary() {
    for stage in ["summary_source_ready", "authorization"] {
        let f = Fixture::new(false).await;
        let pause = Pause::install(&f.session, stage);
        f.start().await;
        pause.entered().await;
        f.service.cancel(f.request.clone()).await.unwrap();
        pause.release();
        let job = f.finish().await;
        assert!(matches!(
            job.state,
            ProcessingJobState::Cancelled | ProcessingJobState::Interrupted
        ));
        assert_eq!(f.model.summaries.load(Ordering::SeqCst), 0);
        assert!(f.summary().await.is_none_or(|body| body.is_empty()));
        f.shutdown().await;
    }
}

#[tokio::test]
async fn local_parent_cancel_drains_optional_title_and_retains_late_candidate() {
    let drain = Arc::new(TitleDrain {
        entered: Semaphore::new(0),
        cancelled: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let model = Arc::new(Model {
        title_drain: Some(drain.clone()),
        ..Default::default()
    });
    let f = Fixture::with_model(true, model).await;
    f.start().await;
    bounded(drain.entered.acquire()).await.unwrap().forget();
    f.service.cancel(f.request.clone()).await.unwrap();
    bounded(drain.cancelled.acquire()).await.unwrap().forget();
    assert_eq!(
        f.service.status(&f.request).await.unwrap().state,
        LocalCaptureState::Cancelling
    );
    assert!(f.context.try_session_lifecycle_change(&f.session).is_err());
    drain.release.add_permits(1);
    let job = f.finish().await;
    assert_eq!(job.state, ProcessingJobState::Succeeded);
    assert_eq!(
        f.context
            .read_metadata(&f.session)
            .await
            .unwrap()
            .meta
            .title,
        ""
    );
    let result = f
        .context
        .processing_job_result(SUMMARY_OWNER.into(), job.id.clone())
        .await
        .unwrap()
        .unwrap();
    let title = result.title.as_ref().unwrap();
    assert_eq!(title.status, ProcessingTitleStatus::Cancelled);
    assert_eq!(title.candidate.as_deref(), Some("Retained title candidate"));
    assert!(f.context.try_session_lifecycle_change(&f.session).is_ok());
    f.reopen_result(job.id, result).await;
}
