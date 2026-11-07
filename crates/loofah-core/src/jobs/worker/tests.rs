use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use hypr_loofah_protocol::{DocumentSaveOutcome, SaveDocumentRequest};
use hypr_vault_write::SessionStore;
use tokio::sync::{RwLock, Semaphore};

struct Fixture {
    root: tempfile::TempDir,
    context: Arc<VaultContext>,
    journal: JobJournal,
    generator: Arc<Generator>,
    authorizer: Arc<Authorizer>,
}
struct Generator {
    calls: AtomicUsize,
    release: Semaphore,
    drain: Semaphore,
    hold_after_cancel: bool,
    cancelled: AtomicBool,
    output: Result<GeneratedDocument, ProcessingJobFailure>,
}
impl JobGenerator for Generator {
    fn generate(
        &self,
        _: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            tokio::select! {
                permit = self.release.acquire() => permit.unwrap().forget(),
                _ = cancel.cancelled() => {
                    self.cancelled.store(true, Ordering::SeqCst);
                    if self.hold_after_cancel { self.drain.acquire().await.unwrap().forget(); }
                },
            }
            self.output.clone()
        })
    }
}
struct Authorizer {
    allowed: Arc<RwLock<bool>>,
}
impl JobAuthorizer for Authorizer {
    fn authorize<'a>(
        &'a self,
        _: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async move {
            let guard = self.allowed.clone().read_owned().await;
            if !*guard {
                return Err(ProcessingJobFailure::AccessDenied);
            }
            Ok(Box::new(guard) as Box<dyn Send>)
        })
    }
}
impl Fixture {
    async fn new(output: &str, hold_after_cancel: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        let vault = root.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        let context = Arc::new(
            VaultContext::new(
                "vault-one".into(),
                Arc::new(SessionStore::new(vault.clone())),
                StartupState::new(Some(&vault)),
                Arc::new(ConfigState::load_or_default(&vault)),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(root.path().join("operations")),
        );
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Release plan","created_at":"2026-11-07T10:00:00Z","tags":[]})).unwrap();
        context
            .store
            .create_note_session(&meta, "Preserve the user's original note.")
            .await
            .unwrap();
        context.store.ensure_summary("note").await.unwrap();
        let journal = JobJournal::open(root.path().join("jobs"), context.events.clone())
            .await
            .unwrap();
        Self {
            root,
            context,
            journal,
            generator: Arc::new(Generator {
                calls: AtomicUsize::new(0),
                release: Semaphore::new(0),
                drain: Semaphore::new(0),
                hold_after_cancel,
                cancelled: AtomicBool::new(false),
                output: Ok(GeneratedDocument {
                    markdown: output.into(),
                    suggested_tags: None,
                }),
            }),
            authorizer: Arc::new(Authorizer {
                allowed: Arc::new(RwLock::new(true)),
            }),
        }
    }
    async fn request(&self, target: DocumentTarget) -> SummaryJobRequest {
        let baseline = self.context.read_document("note", &target).await.unwrap();
        SummaryJobRequest {
            session_id: "note".into(),
            target,
            expected_revision: baseline.revision,
        }
    }
    async fn submit(&self) -> ProcessingJobSnapshot {
        self.journal
            .submit(
                "member-one".into(),
                uuid::Uuid::new_v4().to_string(),
                self.request(DocumentTarget::Summary).await,
            )
            .await
            .unwrap()
    }
    async fn worker(&self) -> JobWorker {
        JobWorker::start(
            self.context.clone(),
            self.journal.clone(),
            self.generator.clone(),
            self.authorizer.clone(),
        )
        .await
        .unwrap()
    }
    async fn terminal(&self, id: &str) -> ProcessingJobSnapshot {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let job = self.journal.get(id.into()).await.unwrap().unwrap();
                if job.state.is_terminal() {
                    break job;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap()
    }
    async fn summary(&self) -> String {
        self.context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .unwrap()
            .markdown
    }
    async fn save(&self, text: &str) {
        let baseline = self
            .context
            .read_document("note", &DocumentTarget::Summary)
            .await
            .unwrap();
        assert!(matches!(
            self.context
                .save_document(
                    "note",
                    &DocumentTarget::Summary,
                    SaveDocumentRequest {
                        markdown: text.into(),
                        expected_revision: baseline.revision
                    }
                )
                .await
                .unwrap(),
            DocumentSaveOutcome::Saved(_)
        ));
    }
}
async fn until(mut check: impl FnMut() -> bool) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !check() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn worker_wakes_for_new_jobs_and_commits_real_vault_documents_without_a_desktop() {
    let f = Fixture::new("# Decisions\n- Ship Friday", false).await;
    let notes = std::fs::read(f.context.store.vault_base().join("sessions/note/notes.md")).unwrap();
    let meta = std::fs::read(
        f.context
            .store
            .vault_base()
            .join("sessions/note/_meta.json"),
    )
    .unwrap();
    let worker = f.worker().await;
    let job = f.submit().await;
    until(|| f.generator.calls.load(Ordering::SeqCst) == 1).await;
    f.generator.release.add_permits(1);
    assert_eq!(
        f.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(f.summary().await, "# Decisions\n- Ship Friday");
    assert_eq!(
        f.journal.result(job.id).await.unwrap().unwrap().markdown,
        f.summary().await
    );
    assert_eq!(
        std::fs::read(f.context.store.vault_base().join("sessions/note/notes.md")).unwrap(),
        notes
    );
    assert_eq!(
        std::fs::read(
            f.context
                .store
                .vault_base()
                .join("sessions/note/_meta.json")
        )
        .unwrap(),
        meta
    );
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_conflict_retains_generated_output_and_never_overwrites_the_newer_summary() {
    let f = Fixture::new("# Generated result", false).await;
    let job = f.submit().await;
    let worker = f.worker().await;
    until(|| f.generator.calls.load(Ordering::SeqCst) == 1).await;
    // Canonical comparison must notice external file edits without an index/watcher refresh.
    std::fs::write(
        f.context
            .store
            .vault_base()
            .join("sessions/note/summary.md"),
        "# User's newer summary",
    )
    .unwrap();
    f.generator.release.add_permits(1);
    assert_eq!(
        f.terminal(&job.id).await.state,
        ProcessingJobState::Conflict
    );
    assert_eq!(f.summary().await, "# User's newer summary");
    assert_eq!(
        f.journal.result(job.id).await.unwrap().unwrap().markdown,
        "# Generated result"
    );
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn stale_input_is_refused_before_a_provider_is_called() {
    let f = Fixture::new("# Never apply", false).await;
    let job = f.submit().await;
    f.save("# Edited before execution").await;
    let worker = f.worker().await;
    assert_eq!(
        f.terminal(&job.id).await.state,
        ProcessingJobState::Conflict
    );
    assert_eq!(f.generator.calls.load(Ordering::SeqCst), 0);
    assert!(f.journal.result(job.id).await.unwrap().is_none());
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancellation_stops_generation_and_retains_a_late_result_without_applying_it() {
    let f = Fixture::new("# Late cancelled output", false).await;
    let job = f.submit().await;
    let worker = f.worker().await;
    until(|| f.generator.calls.load(Ordering::SeqCst) == 1).await;
    f.journal.cancel(job.id.clone()).await.unwrap();
    assert_eq!(
        f.terminal(&job.id).await.state,
        ProcessingJobState::Cancelled
    );
    assert!(f.generator.cancelled.load(Ordering::SeqCst));
    assert_eq!(f.summary().await, "");
    assert!(f.journal.result(job.id).await.unwrap().is_some());
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn revocation_does_not_wait_for_generation_and_prevents_result_application() {
    let f = Fixture::new("# Revoked result", false).await;
    let job = f.submit().await;
    let worker = f.worker().await;
    until(|| f.generator.calls.load(Ordering::SeqCst) == 1).await;
    let mut access = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        f.authorizer.allowed.write(),
    )
    .await
    .unwrap();
    *access = false;
    drop(access);
    let result = f.terminal(&job.id).await;
    assert_eq!(result.state, ProcessingJobState::Failed);
    assert_eq!(result.failure, Some(ProcessingJobFailure::AccessDenied));
    assert!(f.generator.cancelled.load(Ordering::SeqCst));
    assert_eq!(f.summary().await, "");
    assert!(f.journal.result(job.id).await.unwrap().is_some());
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_retains_worker_ownership_until_actual_generation_finishes() {
    let f = Fixture::new("# Unacknowledged result", true).await;
    let job = f.submit().await;
    let worker = f.worker().await;
    until(|| f.generator.calls.load(Ordering::SeqCst) == 1).await;
    let shutdown = tokio::spawn(worker.shutdown());
    until(|| f.generator.cancelled.load(Ordering::SeqCst)).await;
    assert!(!shutdown.is_finished());
    assert_eq!(
        JobWorker::start(
            f.context.clone(),
            f.journal.clone(),
            f.generator.clone(),
            f.authorizer.clone()
        )
        .await
        .err()
        .unwrap()
        .code,
        ErrorCode::Conflict
    );
    f.generator.drain.add_permits(1);
    shutdown.await.unwrap().unwrap();
    assert_eq!(
        f.terminal(&job.id).await.state,
        ProcessingJobState::Interrupted
    );
    assert_eq!(f.summary().await, "");
    assert!(f.journal.result(job.id).await.unwrap().is_some());
}

#[tokio::test]
async fn a_commit_grant_is_held_through_apply_and_cancellation_cannot_undo_a_committed_result() {
    let f = Fixture::new("# Committed result", false).await;
    let job = f.submit().await;
    let worker = f.worker().await;
    until(|| f.generator.calls.load(Ordering::SeqCst) == 1).await;
    let held = f.context.store.lock_snapshot().await;
    f.generator.release.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if f.journal.get(job.id.clone()).await.unwrap().unwrap().phase
                == Some(ProcessingPhase::Applying)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(f.authorizer.allowed.try_write().is_err());
    f.journal.cancel(job.id.clone()).await.unwrap();
    drop(held);
    assert_eq!(
        f.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    assert_eq!(f.summary().await, "# Committed result");
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn regenerated_summary_bodies_preserve_existing_task_identity_and_completion() {
    let f = Fixture::new("# Actions\n- [ ] Ship Friday", false).await;
    f.save("# Actions\n- [x] Ship Friday").await;
    let before = f.context.session_tasks("note").await.unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].status, "done");
    let job = f.submit().await;
    let worker = f.worker().await;
    until(|| f.generator.calls.load(Ordering::SeqCst) == 1).await;
    f.generator.release.add_permits(1);
    assert_eq!(
        f.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    let after = f.context.session_tasks("note").await.unwrap();
    assert_eq!(after[0].id, before[0].id);
    assert_eq!(after[0].status, "done");
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn retained_results_and_successful_submission_receipts_survive_reopening() {
    let f = Fixture::new("# Durable result", false).await;
    let input = f.request(DocumentTarget::Summary).await;
    let request_id = uuid::Uuid::new_v4().to_string();
    let job = f
        .journal
        .submit("member-one".into(), request_id.clone(), input.clone())
        .await
        .unwrap();
    let worker = f.worker().await;
    until(|| f.generator.calls.load(Ordering::SeqCst) == 1).await;
    f.generator.release.add_permits(1);
    let completed = f.terminal(&job.id).await;
    worker.shutdown().await.unwrap();
    drop(f.journal);
    let reopened = JobJournal::open(f.root.path().join("jobs"), f.context.events.clone())
        .await
        .unwrap();
    assert_eq!(
        reopened.result(job.id).await.unwrap().unwrap().markdown,
        "# Durable result"
    );
    assert_eq!(
        reopened
            .submit("member-one".into(), request_id, input)
            .await
            .unwrap(),
        completed
    );
    assert!(reopened.queued().await.unwrap().is_empty());
}

#[tokio::test]
async fn invalid_output_cannot_touch_canonical_content() {
    let f = Fixture::new(" ", false).await;
    let job = f.submit().await;
    let worker = f.worker().await;
    until(|| f.generator.calls.load(Ordering::SeqCst) == 1).await;
    f.generator.release.add_permits(1);
    assert_eq!(
        f.terminal(&job.id).await.failure,
        Some(ProcessingJobFailure::InvalidOutput)
    );
    assert_eq!(f.summary().await, "");
    assert!(f.journal.result(job.id).await.unwrap().is_none());
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn template_body_generation_preserves_metadata_and_completed_tasks() {
    let f = Fixture::new("# Actions\n- [ ] Ship Friday", false).await;
    let target = DocumentTarget::TemplateOutput {
        document_id: "output-one".into(),
    };
    f.context
        .store
        .write_enhanced_doc(&hypr_loofah_protocol::EnhancedDoc {
            id: "output-one".into(),
            session_id: "note".into(),
            kind: "template_output".into(),
            title: "Custom output".into(),
            template_id: "custom-template".into(),
            sort_order: 7,
            markdown: String::new(),
        })
        .await
        .unwrap();
    let initial = f.request(target.clone()).await;
    f.context
        .save_document(
            "note",
            &target,
            SaveDocumentRequest {
                markdown: "# Actions\n- [x] Ship Friday".into(),
                expected_revision: initial.expected_revision,
            },
        )
        .await
        .unwrap();
    let before = f.context.session_tasks("note").await.unwrap();
    assert_eq!(before.len(), 1);
    let job = f
        .journal
        .submit(
            "member-one".into(),
            uuid::Uuid::new_v4().to_string(),
            f.request(target.clone()).await,
        )
        .await
        .unwrap();
    let worker = f.worker().await;
    until(|| f.generator.calls.load(Ordering::SeqCst) == 1).await;
    f.generator.release.add_permits(1);
    assert_eq!(
        f.terminal(&job.id).await.state,
        ProcessingJobState::Succeeded
    );
    let output = f
        .context
        .read_document("note", &target)
        .await
        .unwrap()
        .document
        .unwrap();
    assert_eq!(output.title, "Custom output");
    assert_eq!(output.template_id, "custom-template");
    assert_eq!(output.sort_order, 7);
    let after = f.context.session_tasks("note").await.unwrap();
    assert_eq!(after[0].id, before[0].id);
    assert_eq!(after[0].status, "done");
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn queued_access_denial_never_invokes_the_generator() {
    let f = Fixture::new("# Forbidden", false).await;
    *f.authorizer.allowed.write().await = false;
    let job = f.submit().await;
    let worker = f.worker().await;
    assert_eq!(
        f.terminal(&job.id).await.failure,
        Some(ProcessingJobFailure::AccessDenied)
    );
    assert_eq!(f.generator.calls.load(Ordering::SeqCst), 0);
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn restarting_a_worker_marks_orphaned_execution_interrupted_without_regeneration() {
    let f = Fixture::new("# Never regenerate", false).await;
    let job = f.submit().await;
    f.journal.claim(job.id.clone()).await.unwrap();
    let worker = f.worker().await;
    assert_eq!(
        f.terminal(&job.id).await.state,
        ProcessingJobState::Interrupted
    );
    assert_eq!(f.generator.calls.load(Ordering::SeqCst), 0);
    worker.shutdown().await.unwrap();
}

#[tokio::test]
async fn retained_output_is_immutable_and_invalid_records_are_preserved_for_recovery() {
    let f = Fixture::new("# First result", false).await;
    let job = f.submit().await;
    f.journal.claim(job.id.clone()).await.unwrap();
    let output = GeneratedDocument {
        markdown: "# First result".into(),
        suggested_tags: None,
    };
    f.journal
        .retain_result(job.id.clone(), output.clone())
        .await
        .unwrap();
    f.journal
        .retain_result(job.id.clone(), output.clone())
        .await
        .unwrap();
    assert_eq!(
        f.journal
            .retain_result(
                job.id.clone(),
                GeneratedDocument {
                    markdown: "# Different result".into(),
                    suggested_tags: None,
                }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        f.journal.result(job.id.clone()).await.unwrap(),
        Some(output)
    );
    let path = f
        .root
        .path()
        .join("jobs/results")
        .join(format!("{}.json", job.id));
    let mut record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    record["vault_id"] = "another-vault".into();
    let corrupt = serde_json::to_vec(&record).unwrap();
    std::fs::write(&path, &corrupt).unwrap();
    assert!(f.journal.result(job.id.clone()).await.is_err());
    assert!(
        f.journal
            .retain_result(
                job.id,
                GeneratedDocument {
                    markdown: "# First result".into(),
                    suggested_tags: None,
                }
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), corrupt);
}

#[tokio::test]
async fn unconfirmed_canonical_writes_are_recorded_for_review_without_exposing_raw_errors() {
    let f = Fixture::new("# Review this output", false).await;
    let job = f.submit().await;
    f.journal.claim(job.id.clone()).await.unwrap();
    for outcome in [MutationOutcome::Unknown, MutationOutcome::Persisted] {
        let mut failure = storage();
        failure.message = "sensitive provider error or server path".into();
        failure.mutation_outcome = Some(outcome);
        assert!(matches!(
            completion_from_apply(Err(failure)),
            JobCompletion::Interrupted
        ));
    }
    assert!(matches!(
        completion_from_apply(Err(rejected(ErrorCode::Unavailable, "Vault is stopping"))),
        JobCompletion::Interrupted
    ));
    let mut failure = storage();
    failure.message = "sensitive provider error or server path".into();
    failure.mutation_outcome = Some(MutationOutcome::Unknown);
    let terminal = f
        .journal
        .complete(job.id, completion_from_apply(Err(failure)))
        .await
        .unwrap();
    assert_eq!(terminal.state, ProcessingJobState::Interrupted);
    assert!(
        !serde_json::to_string(&terminal)
            .unwrap()
            .contains("sensitive")
    );
}

#[tokio::test]
async fn invalid_generated_metadata_fails_before_persistence_without_poisoning_the_worker() {
    let mut f = Fixture::new("# Preserve the old body", false).await;
    Arc::get_mut(&mut f.generator).unwrap().output = Ok(GeneratedDocument {
        markdown: "# Preserve the old body".into(),
        suggested_tags: Some(vec![hypr_vault_write::ScoredTagSuggestion {
            name: "topic".into(),
            confidence: f64::NAN,
        }]),
    });
    let worker = f.worker().await;
    for _ in 0..2 {
        let job = f.submit().await;
        f.generator.release.add_permits(1);
        let snapshot = f.terminal(&job.id).await;
        assert_eq!(snapshot.state, ProcessingJobState::Failed);
        assert_eq!(snapshot.failure, Some(ProcessingJobFailure::InvalidOutput));
        assert!(f.journal.result(job.id).await.unwrap().is_none());
        assert_eq!(f.summary().await, "");
    }
    worker.shutdown().await.unwrap();
}
