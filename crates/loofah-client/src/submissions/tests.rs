use super::*;
use hypr_loofah_protocol::ProcessingPhase;

fn input() -> SummaryJobRequest {
    SummaryJobRequest {
        session_id: "note".into(),
        target: DocumentTarget::Summary,
        expected_revision: "baseline".into(),
    }
}
fn snapshot(receipt: &JobSubmissionReceipt, state: ProcessingJobState) -> ProcessingJobSnapshot {
    ProcessingJobSnapshot {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: receipt.input.session_id.clone(),
        target: receipt.input.target.clone(),
        state,
        phase: Some(ProcessingPhase::Preparing),
        failure: None,
        created_at: "2026-11-07T10:00:00Z".into(),
        updated_at: "2026-11-07T10:00:00Z".into(),
    }
}
fn path(root: &Path, scope: &str) -> PathBuf {
    root.join(scope_key(scope)).join("receipts.json")
}

#[test]
#[ignore = "subprocess fixture"]
fn submission_child() {
    let root = PathBuf::from(std::env::var_os("LOOFAH_SUBMISSION_TEST_ROOT").unwrap());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let store = JobSubmissions::new(root.join("requests")).unwrap();
            store.prepare("scope".into(), input()).await.unwrap();
            std::fs::write(root.join("prepared"), b"ready").unwrap();
        });
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[tokio::test]
async fn killing_the_device_process_after_prepare_keeps_the_same_resumable_uuid() {
    struct Child(std::process::Child);
    impl Drop for Child {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = tempfile::tempdir().unwrap();
    let mut child = Child(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "submissions::tests::submission_child",
                "--ignored",
                "--nocapture",
            ])
            .env("LOOFAH_SUBMISSION_TEST_ROOT", root.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while !root.path().join("prepared").exists() {
            assert!(child.0.try_wait().unwrap().is_none());
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(child.0.try_wait().unwrap().is_none());
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let store = JobSubmissions::new(root.path().join("requests")).unwrap();
    let persisted = store.list("scope".into(), None).await.unwrap();
    assert_eq!(persisted.len(), 1);
    assert!(persisted[0].job.is_none());
    assert_eq!(
        store
            .prepare("scope".into(), input())
            .await
            .unwrap()
            .request_id,
        persisted[0].request_id
    );
}

#[tokio::test]
async fn a_failed_directory_sync_is_reconfirmed_before_reusing_an_unacknowledged_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let store = JobSubmissions::new(temp.path().join("requests")).unwrap();
    store
        .fail_after_write
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(store.prepare("scope".into(), input()).await.is_err());
    let persisted = store
        .list("scope".into(), None)
        .await
        .unwrap()
        .pop()
        .unwrap();
    store
        .fail_after_write
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(
        store
            .load("scope".into(), persisted.request_id.clone())
            .await
            .is_err()
    );
    assert_eq!(
        store.list("scope".into(), None).await.unwrap()[0].request_id,
        persisted.request_id
    );
    let confirmed = store
        .load("scope".into(), persisted.request_id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(confirmed, persisted);
    assert!(confirmed.job.is_none());
}

#[tokio::test]
async fn unacknowledged_inputs_survive_restart_and_repeated_preparation_reuses_the_original_uuid() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let first = JobSubmissions::new(root.clone())
        .unwrap()
        .prepare("account/vault".into(), input())
        .await
        .unwrap();
    assert!(first.job.is_none());
    let reopened = JobSubmissions::new(root.clone()).unwrap();
    assert_eq!(
        reopened
            .list("account/vault".into(), Some("note".into()))
            .await
            .unwrap(),
        [first.clone()]
    );
    assert_eq!(
        reopened
            .prepare("account/vault".into(), input())
            .await
            .unwrap(),
        first
    );
    let original = std::fs::read(path(&root, "account/vault")).unwrap();
    let mut changed = input();
    changed.expected_revision = "other-baseline".into();
    assert_eq!(
        reopened
            .prepare("account/vault".into(), changed)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        std::fs::read(path(&root, "account/vault")).unwrap(),
        original
    );
}

#[tokio::test]
async fn template_receipts_reopen_with_exact_target_revision_and_uuid_without_cross_target_acknowledgement()
 {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let store = JobSubmissions::new(root.clone()).unwrap();
    let summary = store.prepare("scope".into(), input()).await.unwrap();
    let mut request = input();
    request.target = DocumentTarget::TemplateOutput {
        document_id: "existing-output".into(),
    };
    let template = store
        .prepare("scope".into(), request.clone())
        .await
        .unwrap();
    let mut other = request.clone();
    other.target = DocumentTarget::TemplateOutput {
        document_id: "other-output".into(),
    };
    let other = store.prepare("scope".into(), other).await.unwrap();
    assert_ne!(summary.request_id, template.request_id);
    assert_ne!(other.request_id, template.request_id);
    drop(store);

    let reopened = JobSubmissions::new(root.clone()).unwrap();
    for original in [&summary, &template, &other] {
        assert_eq!(
            reopened
                .load("scope".into(), original.request_id.clone())
                .await
                .unwrap()
                .as_ref(),
            Some(original)
        );
    }
    assert_eq!(
        reopened
            .prepare("scope".into(), request.clone())
            .await
            .unwrap(),
        template
    );
    let bytes = std::fs::read(path(&root, "scope")).unwrap();
    let mut mismatched = snapshot(&template, ProcessingJobState::Running);
    mismatched.target = DocumentTarget::Summary;
    assert_eq!(
        reopened
            .observe("scope".into(), template.request_id.clone(), mismatched)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    request.expected_revision = "different-revision".into();
    assert_eq!(
        reopened
            .prepare("scope".into(), request)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(std::fs::read(path(&root, "scope")).unwrap(), bytes);
    let running = snapshot(&template, ProcessingJobState::Running);
    assert_eq!(
        reopened
            .observe("scope".into(), template.request_id.clone(), running.clone())
            .await
            .unwrap()
            .job,
        Some(running)
    );
}

#[tokio::test]
async fn duplicate_active_template_targets_are_retained_for_recovery_without_resetting_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let store = JobSubmissions::new(root.clone()).unwrap();
    let mut request = input();
    request.target = DocumentTarget::TemplateOutput {
        document_id: "existing-output".into(),
    };
    let first = store.prepare("scope".into(), request).await.unwrap();
    let mut duplicate = first.clone();
    duplicate.request_id = uuid::Uuid::new_v4().to_string();
    let bytes = serde_json::to_vec(&Stored {
        schema_version: 1,
        receipts: vec![first.clone(), duplicate],
    })
    .unwrap();
    let file = path(&root, "scope");
    std::fs::write(&file, &bytes).unwrap();
    assert_eq!(
        store.list("scope".into(), None).await.unwrap_err().code,
        ErrorCode::Internal
    );
    assert_eq!(
        store
            .prepare("scope".into(), first.input)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Internal
    );
    assert_eq!(std::fs::read(file).unwrap(), bytes);
}

#[tokio::test]
async fn invalid_template_targets_cannot_change_existing_device_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let store = JobSubmissions::new(root.clone()).unwrap();
    store.prepare("scope".into(), input()).await.unwrap();
    let bytes = std::fs::read(path(&root, "scope")).unwrap();
    for document_id in [
        "".into(),
        "../escape".into(),
        "CON".into(),
        "a/b".into(),
        "a\\b".into(),
        "a\0b".into(),
        "x".repeat(513),
    ] {
        let mut request = input();
        request.target = DocumentTarget::TemplateOutput { document_id };
        assert_eq!(
            store
                .prepare("scope".into(), request)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        assert_eq!(std::fs::read(path(&root, "scope")).unwrap(), bytes);
    }
}

#[tokio::test]
async fn independent_store_instances_serialize_windows_and_processes_with_one_active_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let first = JobSubmissions::new(temp.path().join("requests")).unwrap();
    let second = JobSubmissions::new(temp.path().join("requests")).unwrap();
    let (a, b) = tokio::join!(
        first.prepare("scope".into(), input()),
        second.prepare("scope".into(), input())
    );
    assert_eq!(a.unwrap().request_id, b.unwrap().request_id);
    assert_eq!(first.list("scope".into(), None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn scopes_and_sessions_cannot_read_acknowledge_or_remove_another_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let store = JobSubmissions::new(temp.path().join("requests")).unwrap();
    let first = store
        .prepare("member-one/vault".into(), input())
        .await
        .unwrap();
    let other = store
        .prepare("member-two/vault".into(), input())
        .await
        .unwrap();
    assert_ne!(first.request_id, other.request_id);
    assert!(
        store
            .list("member-one/vault".into(), Some("other-note".into()))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        store
            .observe(
                "member-two/vault".into(),
                first.request_id.clone(),
                snapshot(&first, ProcessingJobState::Running)
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert!(
        !store
            .forget("member-two/vault".into(), first.request_id.clone())
            .await
            .unwrap()
    );
    assert_eq!(
        store.list("member-one/vault".into(), None).await.unwrap(),
        [first]
    );
}

#[tokio::test]
async fn mismatched_job_bindings_are_rejected_and_delayed_status_cannot_revive_a_completed_job() {
    let temp = tempfile::tempdir().unwrap();
    let store = JobSubmissions::new(temp.path().join("requests")).unwrap();
    let receipt = store.prepare("scope".into(), input()).await.unwrap();
    let running = snapshot(&receipt, ProcessingJobState::Running);
    let mut wrong = running.clone();
    wrong.session_id = "other".into();
    assert_eq!(
        store
            .observe("scope".into(), receipt.request_id.clone(), wrong)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    store
        .observe("scope".into(), receipt.request_id.clone(), running.clone())
        .await
        .unwrap();
    assert_eq!(
        store
            .observe(
                "scope".into(),
                receipt.request_id.clone(),
                snapshot(&receipt, ProcessingJobState::Running)
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let mut cancelling = running.clone();
    cancelling.state = ProcessingJobState::Cancelling;
    store
        .observe(
            "scope".into(),
            receipt.request_id.clone(),
            cancelling.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .observe("scope".into(), receipt.request_id.clone(), running.clone())
            .await
            .unwrap()
            .job,
        Some(cancelling)
    );
    let mut completed = running.clone();
    completed.state = ProcessingJobState::Succeeded;
    // A regressing wall clock does not make the terminal response older.
    completed.updated_at = "2026-11-06T10:00:00Z".into();
    store
        .observe(
            "scope".into(),
            receipt.request_id.clone(),
            completed.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        store
            .observe("scope".into(), receipt.request_id.clone(), running)
            .await
            .unwrap()
            .job,
        Some(completed)
    );
    let next = store.prepare("scope".into(), input()).await.unwrap();
    assert_ne!(next.request_id, receipt.request_id);
}

#[tokio::test]
async fn corrupt_foreign_and_future_manifests_are_retained_without_resetting_request_ids() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let store = JobSubmissions::new(root.clone()).unwrap();
    store.prepare("scope".into(), input()).await.unwrap();
    let file = path(&root, "scope");
    let original = std::fs::read(&file).unwrap();
    let mut foreign: serde_json::Value = serde_json::from_slice(&original).unwrap();
    foreign["receipts"][0]["scope"] = "foreign".into();
    for content in [
        b"not json".to_vec(),
        serde_json::to_vec(&foreign).unwrap(),
        br#"{"schema_version":999,"receipts":[]}"#.to_vec(),
    ] {
        std::fs::write(&file, &content).unwrap();
        assert!(store.prepare("scope".into(), input()).await.is_err());
        assert!(store.list("scope".into(), None).await.is_err());
        assert_eq!(std::fs::read(&file).unwrap(), content);
    }
}

#[tokio::test]
async fn one_full_scope_never_discards_previous_requests_and_explicit_forget_does_not_imply_cancellation()
 {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let store = JobSubmissions::new(root.clone()).unwrap();
    let mut stored = Stored {
        schema_version: 1,
        receipts: Vec::new(),
    };
    for n in 0..MAX_RECEIPTS {
        let mut request = input();
        request.session_id = format!("note-{n}");
        stored.receipts.push(JobSubmissionReceipt {
            scope: "scope".into(),
            request_id: uuid::Uuid::new_v4().to_string(),
            input: request,
            created_at_ms: n as u64,
            job: None,
        });
    }
    let file = path(&root, "scope");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let bytes = serde_json::to_vec(&stored).unwrap();
    std::fs::write(&file, &bytes).unwrap();
    assert_eq!(
        store
            .prepare("scope".into(), input())
            .await
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    assert!(
        store
            .forget("scope".into(), stored.receipts[0].request_id.clone())
            .await
            .unwrap()
    );
    assert_eq!(
        store.list("scope".into(), None).await.unwrap().len(),
        MAX_RECEIPTS - 1
    );
    assert!(store.prepare("scope".into(), input()).await.is_ok());
}

#[tokio::test]
#[cfg(unix)]
async fn symlinks_cannot_redirect_scope_receipts_or_locks_and_private_modes_are_preserved() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let store = JobSubmissions::new(root.clone()).unwrap();
    store.prepare("scope".into(), input()).await.unwrap();
    let file = path(&root, "scope");
    assert_eq!(root.metadata().unwrap().permissions().mode() & 0o777, 0o700);
    assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    let original = std::fs::read(&file).unwrap();
    for target in [file.clone(), file.parent().unwrap().join(".lock")] {
        let moved = target.with_extension("held");
        std::fs::rename(&target, &moved).unwrap();
        symlink(&moved, &target).unwrap();
        assert!(store.list("scope".into(), None).await.is_err());
        std::fs::remove_file(&target).unwrap();
        std::fs::rename(moved, target).unwrap();
    }
    let directory = file.parent().unwrap();
    let moved = temp.path().join("foreign");
    std::fs::rename(directory, &moved).unwrap();
    symlink(&moved, directory).unwrap();
    assert!(store.list("scope".into(), None).await.is_err());
    assert_eq!(
        std::fs::read(moved.join("receipts.json")).unwrap(),
        original
    );
}
