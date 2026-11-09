use super::*;
use crate::VaultRuntime;
use serde_json::json;

async fn fixture() -> (tempfile::TempDir, VaultRuntime) {
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    std::fs::create_dir_all(vault.join("sessions/note")).unwrap();
    std::fs::write(
        vault.join("sessions/note/_meta.json"),
        serde_json::to_vec(
            &json!({"id":"note","title":"Title","created_at":"2026-11-08T10:00:00Z","tags":[]}),
        )
        .unwrap(),
    )
    .unwrap();
    std::fs::write(vault.join("sessions/note/notes.md"), "Body").unwrap();
    let runtime = VaultRuntime::open(
        "vault".into(),
        &vault,
        &root.path().join("cache"),
        &root.path().join("operations"),
    )
    .await
    .unwrap();
    (root, runtime)
}
#[tokio::test]
async fn local_trusted_principal_can_delete_restore_and_replay_without_a_login_session() {
    let (_root, runtime) = fixture().await;
    let context = runtime.context.clone();
    let revision = context
        .read_session_lifecycle("note")
        .await
        .unwrap()
        .revision;
    let request = uuid::Uuid::new_v4().to_string();
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: revision.clone(),
    };
    let deleted = context
        .delete_session("local".into(), request.clone(), input.clone())
        .await
        .unwrap();
    let DeleteSessionOutcome::Trashed { trash_token, .. } = deleted.clone() else {
        panic!()
    };
    assert!(matches!(
        context
            .restore_session(
                "local".into(),
                uuid::Uuid::new_v4().to_string(),
                RestoreSessionRequest {
                    trash_token,
                    expected_revision: revision
                }
            )
            .await
            .unwrap(),
        RestoreSessionOutcome::Restored { .. }
    ));
    assert_eq!(
        context
            .delete_session("local".into(), request, input)
            .await
            .unwrap(),
        deleted
    );
    assert!(
        context
            .store
            .vault_base()
            .join("sessions/note/_meta.json")
            .exists()
    );
    drop(context);
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn revoked_membership_never_admits_a_directory_move_or_receipt() {
    let (root, runtime) = fixture().await;
    let context = runtime.context.clone();
    let revision = context
        .read_session_lifecycle("note")
        .await
        .unwrap()
        .revision;
    let result = context
        .delete_session_authorized(
            "member".into(),
            uuid::Uuid::new_v4().to_string(),
            DeleteSessionRequest {
                session_id: "note".into(),
                expected_revision: revision,
            },
            || async { Err::<(), _>(error(ErrorCode::Forbidden, "Membership revoked")) },
        )
        .await
        .unwrap_err();
    assert_eq!(result.code, ErrorCode::Forbidden);
    assert_eq!(result.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert!(
        context
            .store
            .vault_base()
            .join("sessions/note/_meta.json")
            .exists()
    );
    assert!(!root.path().join("operations/session-lifecycle").exists());
    drop(context);
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn pending_admission_refuses_delete_immediately_without_queuing_an_exclusive_writer() {
    let (_root, runtime) = fixture().await;
    let context = runtime.context.clone();
    let revision = context
        .read_session_lifecycle("note")
        .await
        .unwrap()
        .revision;
    let work = context.admit_session_work("note").await.unwrap();
    let result = context
        .delete_session(
            "member".into(),
            uuid::Uuid::new_v4().to_string(),
            DeleteSessionRequest {
                session_id: "note".into(),
                expected_revision: revision,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(result.code, ErrorCode::Conflict);
    assert_eq!(result.mutation_outcome, Some(MutationOutcome::NotCommitted));
    drop(work);
    drop(context);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn cancelling_the_waiter_keeps_the_original_grant_and_owned_move_until_commit() {
    let (_root, runtime) = fixture().await;
    let context = runtime.context.clone();
    let revision = context
        .read_session_lifecycle("note")
        .await
        .unwrap()
        .revision;
    let path = context.store.vault_base().join("sessions/note");
    struct Grant {
        source: std::path::PathBuf,
        confirmed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    }
    impl Drop for Grant {
        fn drop(&mut self) {
            self.confirmed
                .store(!self.source.exists(), std::sync::atomic::Ordering::SeqCst);
        }
    }
    let confirmed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let (started, admitted) = tokio::sync::oneshot::channel();
    let owned = context.clone();
    let released = release.clone();
    let observed = confirmed.clone();
    let source = path.clone();
    let waiter = tokio::spawn(async move {
        owned
            .delete_session_authorized(
                "local".into(),
                uuid::Uuid::new_v4().to_string(),
                DeleteSessionRequest {
                    session_id: "note".into(),
                    expected_revision: revision,
                },
                move || async move {
                    let _ = started.send(());
                    released.notified().await;
                    Ok(Grant {
                        source,
                        confirmed: observed,
                    })
                },
            )
            .await
    });
    admitted.await.unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !confirmed.load(std::sync::atomic::Ordering::SeqCst) {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(!path.exists());
    drop(context);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn busy_replay_of_a_moved_but_unacknowledged_request_keeps_unknown_outcome() {
    let (_root, runtime) = fixture().await;
    let context = runtime.context.clone();
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: context
            .read_session_lifecycle("note")
            .await
            .unwrap()
            .revision,
    };
    let id = uuid::Uuid::new_v4().to_string();
    context
        .delete_session("local".into(), id.clone(), input.clone())
        .await
        .unwrap();
    let directory = context
        .operations
        .as_ref()
        .unwrap()
        .root
        .join("session-lifecycle/delete");
    let path = std::fs::read_dir(directory)
        .unwrap()
        .map(Result::unwrap)
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .unwrap()
        .path();
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    receipt["state"] = json!({"kind":"prepared"});
    std::fs::write(path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    let work = context.admit_session_work("note").await.unwrap();
    let cause = context
        .delete_session("local".into(), id, input.clone())
        .await
        .unwrap_err();
    assert_eq!(cause.mutation_outcome, Some(MutationOutcome::Unknown));
    let new = context
        .delete_session("local".into(), uuid::Uuid::new_v4().to_string(), input)
        .await
        .unwrap_err();
    assert_eq!(new.mutation_outcome, Some(MutationOutcome::NotCommitted));
    drop(work);
    drop(context);
    runtime.shutdown().await.unwrap();
}
