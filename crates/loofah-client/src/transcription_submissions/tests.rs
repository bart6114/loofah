use super::*;
use hypr_loofah_protocol::{AbsentTranscriptExpectation, TranscriptionPhase};

fn input() -> TranscriptionJobRequest {
    TranscriptionJobRequest {
        session_id: "note".into(),
        expected_recording_sha256: "a".repeat(64),
        expected_transcript: AbsentTranscriptExpectation::Absent,
        model: "whisper-small".into(),
        languages: vec!["en".into()],
        keywords: vec!["Loofah".into()],
    }
}
fn snapshot(
    receipt: &TranscriptionSubmissionReceipt,
    state: TranscriptionJobState,
) -> TranscriptionJobSnapshot {
    TranscriptionJobSnapshot {
        id: uuid::Uuid::new_v4().to_string(),
        session_id: receipt.input.session_id.clone(),
        state,
        phase: Some(TranscriptionPhase::Preparing),
        created_at: "2026-11-08T10:00:00Z".into(),
        updated_at: "2026-11-08T10:00:00Z".into(),
        result_available: false,
        failure: None,
        publication: None,
    }
}
fn path(root: &Path, scope: &str) -> PathBuf {
    root.join(scope_key(scope)).join("receipts.json")
}

#[tokio::test]
async fn unresolved_original_input_survives_reopen_and_blocks_every_changed_declaration() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let store = TranscriptionSubmissions::new(root.clone()).unwrap();
    let first = store
        .prepare("original-account/vault".into(), input())
        .await
        .unwrap();
    assert!(first.job.is_none());
    drop(store);
    let store = TranscriptionSubmissions::new(root.clone()).unwrap();
    assert_eq!(
        store.prepare(first.scope.clone(), input()).await.unwrap(),
        first
    );
    for changed in [
        TranscriptionJobRequest {
            model: "other-model".into(),
            ..input()
        },
        TranscriptionJobRequest {
            expected_recording_sha256: "b".repeat(64),
            ..input()
        },
        TranscriptionJobRequest {
            languages: vec!["nl".into()],
            ..input()
        },
        TranscriptionJobRequest {
            keywords: vec!["different".into()],
            ..input()
        },
    ] {
        assert_eq!(
            store
                .prepare(first.scope.clone(), changed)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
    }
    assert_eq!(
        store
            .load(first.scope.clone(), first.request_id.clone())
            .await
            .unwrap(),
        Some(first.clone())
    );
    assert_eq!(
        store
            .list(first.scope.clone(), Some("note".into()))
            .await
            .unwrap(),
        [first.clone()]
    );
    assert!(
        store
            .load("other-account/vault".into(), first.request_id.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !store
            .forget("other-account/vault".into(), first.request_id.clone())
            .await
            .unwrap()
    );
    assert!(
        store
            .forget(first.scope.clone(), first.request_id.clone())
            .await
            .unwrap()
    );
    assert_ne!(
        store
            .prepare(first.scope, input())
            .await
            .unwrap()
            .request_id,
        first.request_id
    );
}

#[tokio::test]
async fn known_active_and_uncertain_jobs_keep_the_original_uuid_and_terminal_history_is_monotone() {
    let temp = tempfile::tempdir().unwrap();
    let store = TranscriptionSubmissions::new(temp.path().join("requests")).unwrap();
    let receipt = store.prepare("scope".into(), input()).await.unwrap();
    let mut job = snapshot(&receipt, TranscriptionJobState::Running);
    store
        .observe("scope".into(), receipt.request_id.clone(), job.clone())
        .await
        .unwrap();
    let mut changed = input();
    changed.model = "another-model".into();
    assert_eq!(
        store
            .prepare("scope".into(), changed)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    job.state = TranscriptionJobState::Uncertain;
    let terminal = store
        .observe("scope".into(), receipt.request_id.clone(), job.clone())
        .await
        .unwrap();
    assert_eq!(
        store.prepare("scope".into(), input()).await.unwrap(),
        terminal
    );
    job.state = TranscriptionJobState::Running;
    job.updated_at = "later-wall-clock".into();
    assert_eq!(
        store
            .observe("scope".into(), receipt.request_id.clone(), job)
            .await
            .unwrap(),
        terminal
    );
    let foreign = snapshot(&receipt, TranscriptionJobState::Succeeded);
    assert_eq!(
        store
            .observe("scope".into(), receipt.request_id, foreign)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
}

#[tokio::test]
async fn failed_directory_sync_must_be_reconfirmed_before_a_saved_uuid_can_dispatch() {
    let temp = tempfile::tempdir().unwrap();
    let store = TranscriptionSubmissions::new(temp.path().join("requests")).unwrap();
    store
        .fail_after_write
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(store.prepare("scope".into(), input()).await.is_err());
    let saved = store
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
            .load("scope".into(), saved.request_id.clone())
            .await
            .is_err()
    );
    assert_eq!(
        store
            .load("scope".into(), saved.request_id.clone())
            .await
            .unwrap(),
        Some(saved)
    );
}

#[tokio::test]
async fn concurrent_preparations_share_one_durable_request_and_reading_never_dispatches() {
    let temp = tempfile::tempdir().unwrap();
    let store = TranscriptionSubmissions::new(temp.path().join("requests")).unwrap();
    let (first, second) = tokio::join!(
        store.prepare("scope".into(), input()),
        store.prepare("scope".into(), input())
    );
    assert_eq!(first.unwrap(), second.unwrap());
    assert_eq!(store.list("scope".into(), None).await.unwrap().len(), 1);
}

#[tokio::test]
async fn future_or_corrupt_receipts_and_capacity_limits_are_preserved_without_rewrite() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let store = TranscriptionSubmissions::new(root.clone()).unwrap();
    let receipt = store.prepare("scope".into(), input()).await.unwrap();
    let location = path(&root, "scope");
    let bytes = std::fs::read(&location).unwrap();
    for kind in ["schema", "nested", "duplicate", "scope"] {
        let mut raw: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        match kind {
            "schema" => raw["schema_version"] = 2.into(),
            "nested" => raw["receipts"][0]["input"]["replacement_authority"] = true.into(),
            "duplicate" => {
                let copied = raw["receipts"][0].clone();
                raw["receipts"].as_array_mut().unwrap().push(copied);
            }
            _ => raw["receipts"][0]["scope"] = "other".into(),
        }
        let damaged = serde_json::to_vec(&raw).unwrap();
        std::fs::write(&location, &damaged).unwrap();
        assert!(
            store
                .load("scope".into(), receipt.request_id.clone())
                .await
                .is_err()
        );
        assert!(
            store
                .forget("scope".into(), receipt.request_id.clone())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&location).unwrap(), damaged);
    }
    let receipts = (0..MAX_RECEIPTS)
        .map(|index| {
            let mut item = receipt.clone();
            item.request_id = uuid::Uuid::new_v4().to_string();
            item.input.session_id = format!("note-{index}");
            item
        })
        .collect();
    std::fs::write(
        &location,
        serde_json::to_vec(&Stored {
            schema_version: 1,
            receipts,
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        store
            .prepare("scope".into(), input())
            .await
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        store.list("scope".into(), None).await.unwrap().len(),
        MAX_RECEIPTS
    );
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_receipt_is_never_followed_or_modified() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let store = TranscriptionSubmissions::new(root.clone()).unwrap();
    let receipt = store.prepare("scope".into(), input()).await.unwrap();
    let location = path(&root, "scope");
    let external = temp.path().join("external.json");
    std::fs::write(&external, b"private user data").unwrap();
    std::fs::remove_file(&location).unwrap();
    std::os::unix::fs::symlink(&external, &location).unwrap();
    assert!(
        store
            .load("scope".into(), receipt.request_id)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(external).unwrap(), b"private user data");
}

#[test]
#[ignore = "subprocess fixture"]
fn submission_child() {
    let root =
        PathBuf::from(std::env::var_os("LOOFAH_TRANSCRIPTION_SUBMISSION_TEST_ROOT").unwrap());
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            TranscriptionSubmissions::new(root.join("requests"))
                .unwrap()
                .prepare("scope".into(), input())
                .await
                .unwrap();
            std::fs::write(root.join("prepared"), b"ready").unwrap();
        });
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[tokio::test]
async fn process_death_after_prepare_preserves_the_original_request_for_explicit_recovery() {
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
                "transcription_submissions::tests::submission_child",
                "--ignored",
                "--nocapture",
            ])
            .env("LOOFAH_TRANSCRIPTION_SUBMISSION_TEST_ROOT", root.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    );
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !root.path().join("prepared").exists() {
            assert!(child.0.try_wait().unwrap().is_none());
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    let store = TranscriptionSubmissions::new(root.path().join("requests")).unwrap();
    let original = store
        .list("scope".into(), None)
        .await
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(
        store.prepare("scope".into(), input()).await.unwrap(),
        original
    );
    assert!(original.job.is_none());
}

#[tokio::test]
async fn completed_receipt_allows_new_request_without_reassigning_the_original_job() {
    let temp = tempfile::tempdir().unwrap();
    let store = TranscriptionSubmissions::new(temp.path().join("requests")).unwrap();
    let original = store.prepare("scope".into(), input()).await.unwrap();
    let job = snapshot(&original, TranscriptionJobState::Failed);
    store
        .observe("scope".into(), original.request_id.clone(), job.clone())
        .await
        .unwrap();
    let next = store.prepare("scope".into(), input()).await.unwrap();
    assert_ne!(next.request_id, original.request_id);
    assert_eq!(
        store
            .observe("scope".into(), next.request_id.clone(), job)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let mut wrong = snapshot(&next, TranscriptionJobState::Queued);
    wrong.session_id = "other-note".into();
    assert_eq!(
        store
            .observe("scope".into(), next.request_id.clone(), wrong)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert!(
        store
            .load("scope".into(), next.request_id)
            .await
            .unwrap()
            .unwrap()
            .job
            .is_none()
    );
}

#[tokio::test]
async fn cancelled_waiter_does_not_release_the_receipt_write_lock_or_io_capacity_early() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("requests");
    let store = TranscriptionSubmissions::new(root.clone()).unwrap();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let original = store.clone();
    let caller = tokio::spawn(async move {
        original
            .run("scope".into(), move |_| {
                entered.send(()).unwrap();
                gate.recv().unwrap();
                Ok(((), false))
            })
            .await
    });
    ready.await.unwrap();
    caller.abort();
    let _ = caller.await;
    assert_eq!(store.io.available_permits(), 1);
    let lock_file = File::options()
        .read(true)
        .write(true)
        .open(root.join(scope_key("scope")).join(".lock"))
        .unwrap();
    assert!(matches!(
        lock_file.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    release.send(()).unwrap();
    store.list("scope".into(), None).await.unwrap();
    let drained = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        store.io.clone().acquire_many_owned(2),
    )
    .await
    .unwrap()
    .unwrap();
    drop(drained);
    assert_eq!(store.io.available_permits(), 2);
}
