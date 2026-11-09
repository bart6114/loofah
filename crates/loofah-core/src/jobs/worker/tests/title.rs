use hypr_loofah_protocol::{ProcessingTitleResult, ProcessingTitleStatus};

use super::*;

#[derive(Default)]
struct TitleState {
    body_calls: AtomicUsize,
    prepared_calls: AtomicUsize,
    title_calls: AtomicUsize,
    cancelled: AtomicBool,
}

struct PreparedGenerator {
    state: Arc<TitleState>,
    started: Arc<Semaphore>,
    release: Arc<Semaphore>,
    drain: Arc<Semaphore>,
    blocked: bool,
    hold_after_cancel: bool,
    title: Result<Option<String>, ProcessingJobFailure>,
}

impl PreparedGenerator {
    fn new(
        title: Result<Option<String>, ProcessingJobFailure>,
        blocked: bool,
        hold_after_cancel: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::new(TitleState::default()),
            started: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
            drain: Arc::new(Semaphore::new(0)),
            blocked,
            hold_after_cancel,
            title,
        })
    }
}

const BODY: &str = "# Saved body\n- [ ] Ship Friday.";

impl JobGenerator for PreparedGenerator {
    fn generate(
        &self,
        _: JobWork,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>> {
        self.state.body_calls.fetch_add(1, Ordering::SeqCst);
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
        self.state.prepared_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let document = self.generate(work, cancel).await?;
            Ok(PreparedGeneration {
                document,
                title: Some(Box::new(Runner {
                    state: self.state.clone(),
                    started: self.started.clone(),
                    release: self.release.clone(),
                    drain: self.drain.clone(),
                    blocked: self.blocked,
                    hold_after_cancel: self.hold_after_cancel,
                    output: self.title.clone(),
                })),
            })
        })
    }
}

struct Runner {
    state: Arc<TitleState>,
    started: Arc<Semaphore>,
    release: Arc<Semaphore>,
    drain: Arc<Semaphore>,
    blocked: bool,
    hold_after_cancel: bool,
    output: Result<Option<String>, ProcessingJobFailure>,
}

impl TitleRunner for Runner {
    fn generate(
        self: Box<Self>,
        body: String,
        cancel: CancellationToken,
    ) -> JobFuture<'static, Result<Option<String>, ProcessingJobFailure>> {
        Box::pin(async move {
            assert_eq!(body, BODY);
            self.state.title_calls.fetch_add(1, Ordering::SeqCst);
            self.started.add_permits(1);
            if self.blocked {
                tokio::select! {
                    permit = self.release.acquire() => permit.unwrap().forget(),
                    _ = cancel.cancelled() => {
                        self.state.cancelled.store(true, Ordering::Release);
                        if self.hold_after_cancel { self.drain.acquire().await.unwrap().forget(); }
                    }
                }
            }
            self.output
        })
    }
}

async fn blank_fixture() -> Fixture {
    let fixture = Fixture::new(BODY, false).await;
    let path = fixture
        .context
        .store
        .vault_base()
        .join("sessions/note/_meta.json");
    let mut meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    meta["title"] = serde_json::json!("");
    std::fs::write(path, serde_json::to_vec(&meta).unwrap()).unwrap();
    fixture
}

async fn submit(fixture: &Fixture) -> ProcessingJobSnapshot {
    let mut input = fixture.request(DocumentTarget::Summary).await;
    input.title_policy = TitlePolicy::GenerateIfEmpty;
    fixture
        .journal
        .submit("member-one".into(), uuid::Uuid::new_v4().to_string(), input)
        .await
        .unwrap()
}

async fn worker(fixture: &Fixture, generator: Arc<PreparedGenerator>) -> JobWorker {
    JobWorker::start(
        fixture.context.clone(),
        fixture.journal.clone(),
        generator,
        fixture.authorizer.clone(),
    )
    .await
    .unwrap()
}

async fn title_result(fixture: &Fixture, id: &str) -> ProcessingTitleResult {
    fixture
        .journal
        .owned_result("member-one".into(), id.into())
        .await
        .unwrap()
        .unwrap()
        .title
        .unwrap()
}

async fn started(generator: &PreparedGenerator) {
    tokio::time::timeout(
        std::time::Duration::from_secs(10),
        generator.started.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
}

#[tokio::test]
async fn preserve_requests_use_only_the_legacy_body_path() {
    let fixture = blank_fixture().await;
    let generator = PreparedGenerator::new(Ok(Some("Must not run".into())), false, false);
    let worker = worker(&fixture, generator.clone()).await;
    let job = fixture.submit().await;
    assert_eq!(
        fixture.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(generator.state.body_calls.load(Ordering::SeqCst), 1);
    assert_eq!(generator.state.prepared_calls.load(Ordering::SeqCst), 0);
    assert_eq!(generator.state.title_calls.load(Ordering::SeqCst), 0);
    assert!(
        fixture
            .journal
            .owned_result("member-one".into(), job.id)
            .await
            .unwrap()
            .unwrap()
            .title
            .is_none()
    );
    assert_eq!(
        fixture
            .context
            .store
            .read_meta("note")
            .await
            .unwrap()
            .unwrap()
            .title,
        ""
    );
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn optional_title_outcomes_never_replace_confirmed_body_success_or_immutable_output() {
    for (output, status, failure) in [
        (
            Ok(Some("Suggested title".into())),
            ProcessingTitleStatus::Applied,
            None,
        ),
        (
            Ok(Some("\u{feff} <EMPTY> \u{feff}".into())),
            ProcessingTitleStatus::Empty,
            None,
        ),
        (Ok(None), ProcessingTitleStatus::Empty, None),
        (
            Err(ProcessingJobFailure::ProviderUnavailable),
            ProcessingTitleStatus::Failed,
            Some(ProcessingJobFailure::ProviderUnavailable),
        ),
        (
            Err(ProcessingJobFailure::Interrupted),
            ProcessingTitleStatus::Interrupted,
            None,
        ),
        (
            Ok(Some("Bad\nTitle".into())),
            ProcessingTitleStatus::Failed,
            Some(ProcessingJobFailure::InvalidOutput),
        ),
        (
            Ok(Some("x".repeat(4097))),
            ProcessingTitleStatus::Failed,
            Some(ProcessingJobFailure::InvalidOutput),
        ),
    ] {
        let fixture = blank_fixture().await;
        let generator = PreparedGenerator::new(output, true, false);
        let worker = worker(&fixture, generator.clone()).await;
        let job = submit(&fixture).await;
        started(&generator).await;
        assert_eq!(fixture.summary().await, BODY);
        assert_eq!(
            fixture
                .journal
                .get(job.id.clone())
                .await
                .unwrap()
                .unwrap()
                .phase,
            Some(ProcessingPhase::Applying)
        );
        let result_path = fixture
            .root
            .path()
            .join("jobs/results")
            .join(format!("{}.json", job.id));
        let result_bytes = std::fs::read(&result_path).unwrap();
        let tasks = std::fs::read(
            fixture
                .context
                .store
                .vault_base()
                .join("sessions/note/tasks.json"),
        )
        .unwrap();
        generator.release.add_permits(1);
        let terminal = fixture.terminal(&job.id).await;
        assert_eq!(terminal.state, ProcessingJobState::Succeeded);
        assert_eq!(terminal.failure, None);
        let title = title_result(&fixture, &job.id).await;
        assert_eq!(title.status, status);
        assert_eq!(title.failure, failure);
        assert_eq!(std::fs::read(&result_path).unwrap(), result_bytes);
        assert_eq!(
            std::fs::read(
                fixture
                    .context
                    .store
                    .vault_base()
                    .join("sessions/note/tasks.json")
            )
            .unwrap(),
            tasks
        );
        assert_eq!(generator.state.body_calls.load(Ordering::SeqCst), 1);
        assert_eq!(generator.state.title_calls.load(Ordering::SeqCst), 1);
        if status == ProcessingTitleStatus::Applied {
            assert_eq!(
                fixture.summary().await,
                format!("# Suggested title\n\n{BODY}")
            );
        } else {
            assert_eq!(fixture.summary().await, BODY);
        }
        worker.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn existing_titles_and_missing_ephemeral_runners_do_not_dispatch_paid_title_generation() {
    let fixture = Fixture::new(BODY, false).await;
    let generator = PreparedGenerator::new(Ok(Some("Must not run".into())), false, false);
    let worker = worker(&fixture, generator.clone()).await;
    let job = submit(&fixture).await;
    assert_eq!(
        fixture.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(
        title_result(&fixture, &job.id).await.status,
        ProcessingTitleStatus::AlreadyNamed
    );
    assert_eq!(generator.state.title_calls.load(Ordering::SeqCst), 0);
    worker.shutdown().await.unwrap();
    let fixture = blank_fixture().await;
    fixture.generator.release.add_permits(1);
    let worker = fixture.worker().await;
    let job = submit(&fixture).await;
    assert_eq!(
        fixture.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(
        title_result(&fixture, &job.id).await.status,
        ProcessingTitleStatus::Unsupported
    );
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn later_body_title_and_output_edits_are_preserved_with_the_candidate_retained() {
    for kind in ["body", "title", "output"] {
        let fixture = blank_fixture().await;
        let generator = PreparedGenerator::new(Ok(Some("Suggested title".into())), true, false);
        let worker = worker(&fixture, generator.clone()).await;
        let job = submit(&fixture).await;
        started(&generator).await;
        match kind {
            "body" => std::fs::write(
                fixture
                    .context
                    .store
                    .vault_base()
                    .join("sessions/note/summary.md"),
                "# User's newer body",
            )
            .unwrap(),
            "title" => {
                let path = fixture
                    .context
                    .store
                    .vault_base()
                    .join("sessions/note/_meta.json");
                let mut meta: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                meta["title"] = serde_json::json!("User's newer title");
                std::fs::write(path, serde_json::to_vec(&meta).unwrap()).unwrap();
            }
            _ => fixture
                .context
                .store
                .write_enhanced_doc(&hypr_vault_write::EnhancedDoc {
                    id: "new-output".into(),
                    session_id: "note".into(),
                    kind: "template_output".into(),
                    title: "New output".into(),
                    template_id: "retired".into(),
                    sort_order: 0,
                    markdown: "# User's new output".into(),
                })
                .await
                .unwrap(),
        }
        generator.release.add_permits(1);
        assert_eq!(
            fixture.terminal(&job.id).await.state,
            ProcessingJobState::Succeeded
        );
        let result = title_result(&fixture, &job.id).await;
        assert_eq!(
            result.status,
            if kind == "title" {
                ProcessingTitleStatus::AlreadyNamed
            } else {
                ProcessingTitleStatus::Conflict
            }
        );
        assert_eq!(result.candidate.as_deref(), Some("Suggested title"));
        assert_eq!(
            fixture.summary().await,
            if kind == "body" {
                "# User's newer body"
            } else {
                BODY
            }
        );
        if kind == "title" {
            assert_eq!(
                fixture
                    .context
                    .store
                    .read_meta("note")
                    .await
                    .unwrap()
                    .unwrap()
                    .title,
                "User's newer title"
            );
        }
        worker.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn cancellation_and_revocation_drain_actual_title_work_before_finishing_successfully() {
    for revoke in [false, true] {
        let fixture = blank_fixture().await;
        let generator = PreparedGenerator::new(Ok(Some("Late retained title".into())), true, true);
        let worker = worker(&fixture, generator.clone()).await;
        let job = submit(&fixture).await;
        started(&generator).await;
        if revoke {
            *fixture.authorizer.allowed.write().await = false;
        } else {
            fixture.journal.cancel(job.id.clone()).await.unwrap();
        }
        until(|| generator.state.cancelled.load(Ordering::Acquire)).await;
        assert!(
            !fixture
                .journal
                .get(job.id.clone())
                .await
                .unwrap()
                .unwrap()
                .state
                .is_terminal()
        );
        assert_eq!(fixture.summary().await, BODY);
        assert!(
            matches!(JobWorker::start(fixture.context.clone(), fixture.journal.clone(), generator.clone(), fixture.authorizer.clone()).await, Err(failure) if failure.code == ErrorCode::Conflict)
        );
        generator.drain.add_permits(1);
        let terminal = fixture.terminal(&job.id).await;
        assert_eq!(terminal.state, ProcessingJobState::Succeeded);
        assert_eq!(terminal.failure, None);
        let result = title_result(&fixture, &job.id).await;
        assert_eq!(
            result.status,
            if revoke {
                ProcessingTitleStatus::Failed
            } else {
                ProcessingTitleStatus::Cancelled
            }
        );
        assert_eq!(
            result.failure,
            revoke.then_some(ProcessingJobFailure::AccessDenied)
        );
        assert_eq!(result.candidate.as_deref(), Some("Late retained title"));
        assert_eq!(
            fixture
                .context
                .store
                .read_meta("note")
                .await
                .unwrap()
                .unwrap()
                .title,
            ""
        );
        worker.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn admitted_title_io_retains_the_authorization_grant_through_late_cancellation() {
    let fixture = blank_fixture().await;
    let generator = PreparedGenerator::new(Ok(Some("Suggested title".into())), true, false);
    let worker = worker(&fixture, generator.clone()).await;
    let job = submit(&fixture).await;
    started(&generator).await;
    let held = fixture.context.store.lock_snapshot().await;
    generator.release.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while title_result(&fixture, &job.id).await.status != ProcessingTitleStatus::Applying {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.journal.cancel(job.id.clone()).await.unwrap();
    assert!(
        fixture
            .authorizer
            .allowed
            .clone()
            .try_write_owned()
            .is_err()
    );
    assert_eq!(
        fixture
            .journal
            .get(job.id.clone())
            .await
            .unwrap()
            .unwrap()
            .state,
        ProcessingJobState::Cancelling
    );
    drop(held);
    assert_eq!(
        fixture.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(
        title_result(&fixture, &job.id).await.status,
        ProcessingTitleStatus::Applied
    );
    assert!(fixture.authorizer.allowed.clone().try_write_owned().is_ok());
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn optional_sidecar_storage_failure_preserves_body_success_and_unknown_private_files() {
    let fixture = blank_fixture().await;
    let generator = PreparedGenerator::new(Ok(Some("Suggested title".into())), true, false);
    let worker = worker(&fixture, generator.clone()).await;
    let job = submit(&fixture).await;
    started(&generator).await;
    let path = fixture.root.path().join("jobs/titles");
    std::fs::rename(&path, fixture.root.path().join("retained-titles")).unwrap();
    std::fs::write(&path, "Keep this unknown file").unwrap();
    generator.release.add_permits(1);
    assert_eq!(
        fixture.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(
        title_result(&fixture, &job.id).await.status,
        ProcessingTitleStatus::Unconfirmed
    );
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "Keep this unknown file"
    );
    assert_eq!(fixture.summary().await, BODY);
    assert_eq!(
        fixture
            .context
            .store
            .read_meta("note")
            .await
            .unwrap()
            .unwrap()
            .title,
        ""
    );
    worker.shutdown().await.unwrap();
}

mod recovery;
mod transport;

struct EditBeforeTitle {
    context: Arc<VaultContext>,
    journal: JobJournal,
    edited: AtomicBool,
}

impl JobAuthorizer for EditBeforeTitle {
    fn authorize<'a>(
        &'a self,
        _: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async move {
            let confirmed = self
                .journal
                .run(|state, _| {
                    Ok(state
                        .records
                        .values()
                        .any(|record| record.body_applied.is_some()))
                })
                .await
                .map_err(|_| ProcessingJobFailure::Storage)?;
            if confirmed && !self.edited.swap(true, Ordering::SeqCst) {
                std::fs::write(
                    self.context
                        .store
                        .vault_base()
                        .join("sessions/note/summary.md"),
                    "# Edited before baseline preparation",
                )
                .unwrap();
            }
            Ok(Box::new(()) as Box<dyn Send>)
        })
    }
}

#[tokio::test]
async fn a_body_changed_before_baseline_preparation_never_becomes_paid_title_input() {
    let fixture = blank_fixture().await;
    let generator = PreparedGenerator::new(Ok(Some("Must not run".into())), false, false);
    let authorizer = Arc::new(EditBeforeTitle {
        context: fixture.context.clone(),
        journal: fixture.journal.clone(),
        edited: AtomicBool::new(false),
    });
    let worker = JobWorker::start(
        fixture.context.clone(),
        fixture.journal.clone(),
        generator.clone(),
        authorizer.clone(),
    )
    .await
    .unwrap();
    let job = submit(&fixture).await;
    assert_eq!(
        fixture.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert!(authorizer.edited.load(Ordering::SeqCst));
    assert_eq!(generator.state.title_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        title_result(&fixture, &job.id).await.status,
        ProcessingTitleStatus::Conflict
    );
    assert_eq!(
        fixture.summary().await,
        "# Edited before baseline preparation"
    );
    worker.shutdown().await.unwrap();
}

struct CancelBeforeApply {
    journal: JobJournal,
    cancelled: AtomicBool,
}

impl JobAuthorizer for CancelBeforeApply {
    fn authorize<'a>(
        &'a self,
        _: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async move {
            let ready = self
                .journal
                .run(|state, _| {
                    Ok(state
                        .records
                        .values()
                        .find(|record| {
                            title_stage::project(state, record).is_some_and(|title| {
                                title.status == ProcessingTitleStatus::Generating
                                    && title.candidate.is_some()
                            })
                        })
                        .map(|record| record.snapshot.id.clone()))
                })
                .await
                .map_err(|_| ProcessingJobFailure::Storage)?;
            if let Some(id) = ready
                && !self.cancelled.swap(true, Ordering::SeqCst)
            {
                self.journal
                    .cancel(id)
                    .await
                    .map_err(|_| ProcessingJobFailure::Storage)?;
            }
            Ok(Box::new(()) as Box<dyn Send>)
        })
    }
}

#[tokio::test]
async fn cancellation_during_final_authorization_cannot_admit_title_application() {
    let fixture = blank_fixture().await;
    let generator =
        PreparedGenerator::new(Ok(Some("Retained cancelled title".into())), false, false);
    let authorizer = Arc::new(CancelBeforeApply {
        journal: fixture.journal.clone(),
        cancelled: AtomicBool::new(false),
    });
    let worker = JobWorker::start(
        fixture.context.clone(),
        fixture.journal.clone(),
        generator,
        authorizer.clone(),
    )
    .await
    .unwrap();
    let job = submit(&fixture).await;
    assert_eq!(
        fixture.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert!(authorizer.cancelled.load(Ordering::SeqCst));
    let result = title_result(&fixture, &job.id).await;
    assert_eq!(result.status, ProcessingTitleStatus::Cancelled);
    assert_eq!(
        result.candidate.as_deref(),
        Some("Retained cancelled title")
    );
    assert_eq!(
        fixture
            .context
            .store
            .read_meta("note")
            .await
            .unwrap()
            .unwrap()
            .title,
        ""
    );
    assert_eq!(fixture.summary().await, BODY);
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn existing_title_is_published_atomically_for_both_policies_and_targets() {
    for title_policy in [TitlePolicy::Preserve, TitlePolicy::GenerateIfEmpty] {
        for template in [false, true] {
            let fixture = Fixture::new(BODY, false).await;
            let target = if template {
                fixture
                    .context
                    .store
                    .write_enhanced_doc(&hypr_vault_write::EnhancedDoc {
                        id: "output".into(),
                        session_id: "note".into(),
                        kind: "template_output".into(),
                        title: "Keep template name".into(),
                        template_id: "custom".into(),
                        sort_order: 3,
                        markdown: "Original template".into(),
                    })
                    .await
                    .unwrap();
                DocumentTarget::TemplateOutput {
                    document_id: "output".into(),
                }
            } else {
                DocumentTarget::Summary
            };
            let generator = PreparedGenerator::new(Ok(Some("Must not run".into())), false, false);
            let worker = worker(&fixture, generator.clone()).await;
            let mut input = fixture.request(target.clone()).await;
            input.title_policy = title_policy;
            let job = fixture
                .journal
                .submit("member-one".into(), uuid::Uuid::new_v4().to_string(), input)
                .await
                .unwrap();
            assert_eq!(
                fixture.terminal(&job.id).await.state,
                ProcessingJobState::Succeeded
            );
            let snapshot = fixture
                .context
                .read_document("note", &target)
                .await
                .unwrap();
            let document = snapshot.document.unwrap();
            assert_eq!(document.markdown, format!("# Release plan\n\n{BODY}"));
            if template {
                assert_eq!(document.title, "Keep template name");
                assert_eq!(document.template_id, "custom");
            }
            let result = fixture
                .journal
                .owned_result("member-one".into(), job.id.clone())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(result.markdown, BODY);
            assert_eq!(generator.state.body_calls.load(Ordering::SeqCst), 1);
            assert_eq!(generator.state.title_calls.load(Ordering::SeqCst), 0);
            if title_policy == TitlePolicy::GenerateIfEmpty {
                assert_eq!(
                    result.title.unwrap().status,
                    ProcessingTitleStatus::AlreadyNamed
                );
                let id = job.id.clone();
                let recorded = fixture
                    .journal
                    .run(move |state, _| {
                        Ok(state
                            .record(&id)?
                            .body_applied
                            .as_ref()
                            .unwrap()
                            .revision
                            .clone())
                    })
                    .await
                    .unwrap();
                assert_eq!(recorded, snapshot.revision);
            } else {
                assert!(result.title.is_none());
            }
            worker.shutdown().await.unwrap();
        }
    }
}
