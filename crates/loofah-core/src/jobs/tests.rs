use super::*;

fn request() -> SummaryJobRequest {
    SummaryJobRequest {
        only_if_empty: false,
        title_policy: Default::default(),
        session_id: "session-one".into(),
        target: DocumentTarget::Summary,
        expected_revision: "reviewed-revision".into(),
    }
}
async fn fixture() -> (tempfile::TempDir, JobJournal) {
    let root = tempfile::tempdir().unwrap();
    let journal = JobJournal::open(
        root.path().join("jobs"),
        EventHub::new("vault-one".into(), 256),
    )
    .await
    .unwrap();
    (root, journal)
}
async fn submit(journal: &JobJournal) -> ProcessingJobSnapshot {
    journal
        .submit(
            "member-one".into(),
            uuid::Uuid::new_v4().to_string(),
            request(),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn concurrent_retry_and_restart_keep_one_submission_per_member_and_body() {
    let (root, journal) = fixture().await;
    let id = uuid::Uuid::new_v4().to_string();
    let (a, b) = tokio::join!(
        journal.submit("member-one".into(), id.clone(), request()),
        journal.submit("member-one".into(), id.clone(), request())
    );
    let a = a.unwrap();
    assert_eq!(a, b.unwrap());
    assert_eq!(a.state, ProcessingJobState::Queued);
    let other = journal
        .submit("member-two".into(), id.clone(), request())
        .await
        .unwrap();
    assert_ne!(a.id, other.id);
    let mut changed = request();
    changed.expected_revision = "different-revision".into();
    assert_eq!(
        journal
            .submit("member-one".into(), id.clone(), changed)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    drop(journal);
    let restored = JobJournal::open(
        root.path().join("jobs"),
        EventHub::new("vault-one".into(), 16),
    )
    .await
    .unwrap();
    assert_eq!(
        restored
            .submit("member-one".into(), id, request())
            .await
            .unwrap(),
        a
    );
    assert_eq!(restored.page(200, None).await.unwrap().len(), 2);
}

#[tokio::test]
async fn startup_keeps_queued_work_and_marks_active_work_interrupted_without_replaying_it() {
    let (root, journal) = fixture().await;
    let queued = submit(&journal).await;
    let running = submit(&journal).await;
    let cancelling = submit(&journal).await;
    journal.claim(running.id.clone()).await.unwrap();
    journal
        .progress(running.id.clone(), ProcessingPhase::Applying)
        .await
        .unwrap();
    journal.claim(cancelling.id.clone()).await.unwrap();
    journal.cancel(cancelling.id.clone()).await.unwrap();
    drop(journal);
    let restored = JobJournal::open(
        root.path().join("jobs"),
        EventHub::new("vault-one".into(), 16),
    )
    .await
    .unwrap();
    assert_eq!(
        restored.get(queued.id).await.unwrap().unwrap().state,
        ProcessingJobState::Queued
    );
    for id in [running.id, cancelling.id] {
        let value = restored.get(id.clone()).await.unwrap().unwrap();
        assert_eq!(value.state, ProcessingJobState::Interrupted);
        assert_eq!(value.failure, Some(ProcessingJobFailure::Interrupted));
        assert!(value.phase.is_none());
        assert!(restored.claim(id).await.unwrap().is_none());
    }
}

#[tokio::test]
async fn cancellation_is_durable_and_does_not_claim_to_undo_a_completed_commit() {
    let (root, journal) = fixture().await;
    let queued = submit(&journal).await;
    assert_eq!(
        journal.cancel(queued.id.clone()).await.unwrap().state,
        ProcessingJobState::Cancelled
    );
    assert!(journal.claim(queued.id.clone()).await.unwrap().is_none());
    let running = submit(&journal).await;
    journal.claim(running.id.clone()).await.unwrap();
    journal
        .progress(running.id.clone(), ProcessingPhase::Generating)
        .await
        .unwrap();
    assert_eq!(
        journal.cancel(running.id.clone()).await.unwrap().state,
        ProcessingJobState::Cancelling
    );
    assert_eq!(
        journal
            .complete(running.id.clone(), JobCompletion::Cancelled)
            .await
            .unwrap()
            .state,
        ProcessingJobState::Cancelled
    );
    let applying = submit(&journal).await;
    journal.claim(applying.id.clone()).await.unwrap();
    journal
        .progress(applying.id.clone(), ProcessingPhase::Applying)
        .await
        .unwrap();
    journal.cancel(applying.id.clone()).await.unwrap();
    let committed = journal
        .complete(applying.id.clone(), JobCompletion::Succeeded)
        .await
        .unwrap();
    assert_eq!(committed.state, ProcessingJobState::Succeeded);
    assert_eq!(
        journal.cancel(applying.id.clone()).await.unwrap(),
        committed
    );
    assert_eq!(
        journal
            .complete(applying.id.clone(), JobCompletion::Succeeded)
            .await
            .unwrap(),
        committed
    );
    assert_eq!(
        journal
            .complete(applying.id, JobCompletion::Cancelled)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    drop(journal);
    let reopened = JobJournal::open(
        root.path().join("jobs"),
        EventHub::new("vault-one".into(), 16),
    )
    .await
    .unwrap();
    assert_eq!(
        reopened.get(running.id).await.unwrap().unwrap().state,
        ProcessingJobState::Cancelled
    );
}

#[tokio::test]
async fn exclusive_ownership_and_vault_binding_prevent_cross_vault_recovery() {
    let (root, journal) = fixture().await;
    submit(&journal).await;
    assert_eq!(
        JobJournal::open(
            root.path().join("jobs"),
            EventHub::new("vault-one".into(), 16)
        )
        .await
        .err()
        .unwrap()
        .code,
        ErrorCode::Conflict
    );
    drop(journal);
    let path = std::fs::read_dir(root.path().join("jobs"))
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(
        JobJournal::open(
            root.path().join("jobs"),
            EventHub::new("another-vault".into(), 16)
        )
        .await
        .is_err()
    );
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[tokio::test]
async fn notifications_follow_persistence_and_terminal_states_reject_late_workers() {
    let (_root, journal) = fixture().await;
    let mut changes = journal.0.events.subscribe(None);
    changes.recv().await;
    let job = submit(&journal).await;
    let event = changes.recv().await.unwrap();
    assert_eq!(
        event.change,
        Change::JobChanged {
            job_id: job.id.clone()
        }
    );
    assert_eq!(
        journal.get(job.id.clone()).await.unwrap(),
        Some(job.clone())
    );
    assert_eq!(
        journal
            .complete(job.id.clone(), JobCompletion::Succeeded)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    journal.claim(job.id.clone()).await.unwrap();
    assert!(journal.claim(job.id.clone()).await.unwrap().is_none());
    let failed = journal
        .complete(
            job.id.clone(),
            JobCompletion::Failed(ProcessingJobFailure::ProviderFailed),
        )
        .await
        .unwrap();
    assert_eq!(failed.failure, Some(ProcessingJobFailure::ProviderFailed));
    assert_eq!(
        journal
            .progress(job.id.clone(), ProcessingPhase::Applying)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(journal.request(job.id).await.unwrap(), request());
}

#[tokio::test]
async fn job_admission_is_bounded_and_read_pages_are_stable() {
    let (_root, journal) = fixture().await;
    let mut ids = vec![];
    for _ in 0..MAX_PENDING {
        ids.push(submit(&journal).await.id)
    }
    assert_eq!(
        journal
            .submit(
                "member-one".into(),
                uuid::Uuid::new_v4().to_string(),
                request()
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
    let first = journal.page(10, None).await.unwrap();
    let second = journal
        .page(10, Some(first.last().unwrap().id.clone()))
        .await
        .unwrap();
    assert_eq!(first.len(), 10);
    assert_eq!(second.len(), 10);
    assert!(first.last().unwrap().id < second[0].id);
    journal.cancel(ids.remove(0)).await.unwrap();
    submit(&journal).await;
    assert_eq!(
        journal.page(0, None).await.unwrap_err().code,
        ErrorCode::Validation
    );
}

#[tokio::test]
async fn malformed_future_and_oversized_owned_records_fail_closed_without_touching_unknown_files() {
    for variant in ["future", "malformed", "oversized"] {
        let (root, journal) = fixture().await;
        submit(&journal).await;
        drop(journal);
        let directory = root.path().join("jobs");
        let unknown = directory.join("user-keep.json");
        std::fs::write(&unknown, b"Unknown files are never job input").unwrap();
        let owned = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .find(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .and_then(|name| name.strip_suffix(".json"))
                    .is_some_and(is_key)
            })
            .unwrap();
        let contents = match variant {
            "future" => {
                let mut json: serde_json::Value =
                    serde_json::from_slice(&std::fs::read(&owned).unwrap()).unwrap();
                json["schema_version"] = 999.into();
                serde_json::to_vec(&json).unwrap()
            }
            "oversized" => vec![b' '; MAX_RECORD_BYTES as usize + 1],
            _ => b"Incomplete JSON".to_vec(),
        };
        std::fs::write(&owned, &contents).unwrap();
        let failure = JobJournal::open(directory, EventHub::new("vault-one".into(), 16))
            .await
            .err()
            .unwrap();
        if variant == "future" {
            assert_eq!(failure.code, ErrorCode::IncompatibleVersion)
        }
        assert_eq!(std::fs::read(owned).unwrap(), contents);
        assert_eq!(
            std::fs::read(unknown).unwrap(),
            b"Unknown files are never job input"
        );
    }
}

#[tokio::test]
async fn invalid_identifiers_and_targets_do_not_create_job_records() {
    let (_root, journal) = fixture().await;
    for input in [
        SummaryJobRequest {
            session_id: "../secret".into(),
            ..request()
        },
        SummaryJobRequest {
            target: DocumentTarget::TemplateOutput {
                document_id: "../secret".into(),
            },
            ..request()
        },
        SummaryJobRequest {
            expected_revision: String::new(),
            ..request()
        },
    ] {
        assert_eq!(
            journal
                .submit("member-one".into(), uuid::Uuid::new_v4().to_string(), input)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
    assert!(
        journal
            .submit("".into(), uuid::Uuid::new_v4().to_string(), request())
            .await
            .is_err()
    );
    assert!(journal.get("../secret".into()).await.is_err());
    assert!(journal.page(10, None).await.unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn symbolic_links_are_refused_and_private_records_keep_restrictive_permissions() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let (root, journal) = fixture().await;
    let job = submit(&journal).await;
    drop(journal);
    let directory = root.path().join("jobs");
    let owned = std::fs::read_dir(&directory)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    assert_eq!(
        std::fs::metadata(&directory).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&owned).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let original = std::fs::read(&owned).unwrap();
    let target = root.path().join("protected");
    std::fs::write(&target, &original).unwrap();
    std::fs::remove_file(&owned).unwrap();
    symlink(&target, &owned).unwrap();
    assert!(
        JobJournal::open(directory.clone(), EventHub::new("vault-one".into(), 16))
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&target).unwrap(), original);
    let alias = root.path().join("alias");
    symlink(&directory, &alias).unwrap();
    assert!(
        JobJournal::open(alias, EventHub::new("vault-one".into(), 16))
            .await
            .is_err()
    );
    assert!(uuid::Uuid::parse_str(&job.id).is_ok());
}

#[tokio::test]
async fn uncertain_persistence_stops_stale_cache_writes_until_recovery() {
    let (root, journal) = fixture().await;
    let request_id = uuid::Uuid::new_v4().to_string();
    journal.0.state.lock().unwrap().fail_after_persist = true;
    let failure = journal
        .submit("member-one".into(), request_id.clone(), request())
        .await
        .unwrap_err();
    assert_eq!(failure.mutation_outcome, Some(MutationOutcome::Unknown));
    assert_eq!(
        journal
            .submit("member-one".into(), request_id.clone(), request())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    assert_eq!(
        journal.page(10, None).await.unwrap_err().code,
        ErrorCode::Unavailable
    );
    drop(journal);
    let recovered = JobJournal::open(
        root.path().join("jobs"),
        EventHub::new("vault-one".into(), 16),
    )
    .await
    .unwrap();
    let original = recovered
        .submit("member-one".into(), request_id, request())
        .await
        .unwrap();
    assert_eq!(recovered.page(10, None).await.unwrap().len(), 1);
    recovered.claim(original.id.clone()).await.unwrap();
    recovered.0.state.lock().unwrap().fail_after_persist = true;
    assert_eq!(
        recovered
            .complete(original.id.clone(), JobCompletion::Succeeded)
            .await
            .unwrap_err()
            .mutation_outcome,
        Some(MutationOutcome::Unknown)
    );
    assert_eq!(
        recovered
            .cancel(original.id.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    drop(recovered);
    let final_state = JobJournal::open(
        root.path().join("jobs"),
        EventHub::new("vault-one".into(), 16),
    )
    .await
    .unwrap();
    assert_eq!(
        final_state.get(original.id).await.unwrap().unwrap().state,
        ProcessingJobState::Succeeded
    );
}

#[tokio::test]
async fn dropping_a_submission_waiter_does_not_abort_the_owned_persistence_operation() {
    let (_root, journal) = fixture().await;
    let held = journal.clone();
    let (locked_tx, locked_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let lock_thread = std::thread::spawn(move || {
        let _guard = held.0.state.lock().unwrap();
        let _ = locked_tx.send(());
        let _ = release_rx.recv();
    });
    locked_rx.await.unwrap();
    let task_journal = journal.clone();
    let request_id = uuid::Uuid::new_v4().to_string();
    let replay_id = request_id.clone();
    let task = tokio::spawn(async move {
        task_journal
            .submit("member-one".into(), request_id, request())
            .await
    });
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    // The fourth owner is the blocking persistence closure, waiting behind the held lock.
    while Arc::strong_count(&journal.0) < 4 {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::task::yield_now().await;
    }
    task.abort();
    release_tx.send(()).unwrap();
    lock_thread.join().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while journal.page(10, None).await.unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // A retry serializes with the already-owned write even when its original reply was abandoned.
    let recovered = journal
        .submit("member-one".into(), replay_id, request())
        .await
        .unwrap();
    assert_eq!(journal.page(10, None).await.unwrap(), vec![recovered]);
}

#[tokio::test]
#[ignore = "subprocess fixture invoked by job_recovery_after_process_termination"]
async fn job_process_fixture() {
    let root = PathBuf::from(std::env::var_os("LOOFAH_JOB_TEST_ROOT").expect("isolated test root"));
    let journal = JobJournal::open(root.join("jobs"), EventHub::new("vault-one".into(), 16))
        .await
        .unwrap();
    let queued = submit(&journal).await;
    let active = submit(&journal).await;
    journal.claim(active.id.clone()).await.unwrap();
    let mut marker = BTreeMap::new();
    marker.insert("queued", queued.id);
    marker.insert("active", active.id);
    hypr_storage::fs::atomic_write(
        &root.join("ready.json"),
        &serde_json::to_string(&marker).unwrap(),
    )
    .unwrap();
    std::future::pending::<()>().await;
}

#[tokio::test]
async fn job_recovery_after_process_termination() {
    let root = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--ignored", "--exact", "jobs::tests::job_process_fixture"])
        .env("LOOFAH_JOB_TEST_ROOT", root.path())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !root.path().join("ready.json").exists() {
        if std::time::Instant::now() > deadline || child.try_wait().unwrap().is_some() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("job subprocess did not reach its durable state");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let ids: HashMap<String, String> =
        serde_json::from_slice(&std::fs::read(root.path().join("ready.json")).unwrap()).unwrap();
    let journal = JobJournal::open(
        root.path().join("jobs"),
        EventHub::new("vault-one".into(), 16),
    )
    .await
    .unwrap();
    assert_eq!(
        journal
            .get(ids["queued"].clone())
            .await
            .unwrap()
            .unwrap()
            .state,
        ProcessingJobState::Queued
    );
    assert_eq!(
        journal
            .get(ids["active"].clone())
            .await
            .unwrap()
            .unwrap()
            .state,
        ProcessingJobState::Interrupted
    );
    assert!(
        journal
            .claim(ids["active"].clone())
            .await
            .unwrap()
            .is_none()
    );
}
