use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use hypr_vault_write::SessionStore;
use serde_json::json;

async fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Arc<VaultContext>) {
    let vault = tempfile::tempdir().unwrap();
    let operations = tempfile::tempdir().unwrap();
    let store = Arc::new(SessionStore::new(vault.path().into()));
    let meta = serde_json::from_value(
        json!({"id":"note","title":"","created_at":"2026-11-07T10:00:00Z","tags":[]}),
    )
    .unwrap();
    store.create_note_session(&meta, "").await.unwrap();
    let context = VaultContext::new(
        "vault-a".into(),
        store,
        StartupState::new(Some(vault.path())),
        Arc::new(ConfigState::load_or_default(vault.path())),
        Arc::new(SearchService::default()),
    )
    .with_operation_storage(operations.path().into());
    (vault, operations, Arc::new(context))
}

#[tokio::test]
async fn cleanup_is_login_free_for_local_owner_and_replays_without_redeleting_restored_notes() {
    let (vault, _, context) = fixture().await;
    let key = uuid::Uuid::new_v4().to_string();
    let input = DeleteEmptySessionRequest {
        session_id: "note".into(),
    };
    assert_eq!(
        context
            .delete_empty_session("local-owner".into(), key.clone(), input.clone())
            .await
            .unwrap(),
        DeleteEmptySessionOutcome::Deleted
    );
    assert!(!vault.path().join("sessions/note").exists());
    assert!(context.store.restore_session("note").await.unwrap());
    context
        .store
        .write_note("note", "Restored content")
        .await
        .unwrap();
    assert_eq!(
        context
            .delete_empty_session("local-owner".into(), key, input)
            .await
            .unwrap(),
        DeleteEmptySessionOutcome::Deleted
    );
    assert_eq!(
        context.store.read_note("note").await.unwrap().as_deref(),
        Some("Restored content")
    );
}

#[tokio::test]
async fn cleanup_validates_rejects_revoked_access_and_reports_busy_as_definitely_not_committed() {
    let (vault, operations, context) = fixture().await;
    for id in ["../note", "CON", "note."] {
        let error = context
            .delete_empty_session(
                "owner".into(),
                uuid::Uuid::new_v4().to_string(),
                DeleteEmptySessionRequest {
                    session_id: id.into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    }
    let error = context
        .delete_empty_session_authorized(
            "owner".into(),
            uuid::Uuid::new_v4().to_string(),
            DeleteEmptySessionRequest {
                session_id: "note".into(),
            },
            || async { Err::<(), _>(error(ErrorCode::Forbidden, "Access revoked")) },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert!(!operations.path().join("session-cleanups").exists());
    context.store.note_recording_active("note");
    let error = context
        .delete_empty_session(
            "owner".into(),
            uuid::Uuid::new_v4().to_string(),
            DeleteEmptySessionRequest {
                session_id: "note".into(),
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert!(vault.path().join("sessions/note").exists());
}

#[tokio::test]
async fn cancelled_cleanup_retains_context_vault_lease_and_write_grant_through_actual_completion() {
    let (vault, operations, context) = fixture().await;
    let mut context = Arc::try_unwrap(context).ok().unwrap();
    let lease_path = vault.path().join("cleanup-test.lock");
    let lease = std::fs::File::create(&lease_path).unwrap();
    lease.try_lock().unwrap();
    context.lease = Some(lease.into());
    let context = Arc::new(context);
    let weak = Arc::downgrade(&context);
    let competitor = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lease_path)
        .unwrap();
    let blocked = context.store.lock_snapshot().await;
    let (authorized_tx, authorized_rx) = tokio::sync::oneshot::channel();
    let (released_tx, released_rx) = tokio::sync::oneshot::channel();
    struct Grant(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for Grant {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }
    let caller = context.clone();
    let task = tokio::spawn(async move {
        caller
            .delete_empty_session_authorized(
                "owner".into(),
                uuid::Uuid::new_v4().to_string(),
                DeleteEmptySessionRequest {
                    session_id: "note".into(),
                },
                || async move {
                    authorized_tx.send(()).unwrap();
                    Ok(Grant(Some(released_tx)))
                },
            )
            .await
    });
    authorized_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(context);
    assert!(weak.upgrade().is_some());
    assert!(competitor.try_lock().is_err());
    assert!(vault.path().join("sessions/note").exists());
    drop(blocked);
    tokio::time::timeout(std::time::Duration::from_secs(10), released_rx)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    competitor.try_lock().unwrap();
    assert!(!vault.path().join("sessions/note").exists());
    let path = std::fs::read_dir(operations.path().join("session-cleanups"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let receipt: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(receipt["state"]["kind"], "completed");
    assert_eq!(receipt["state"]["outcome"]["kind"], "deleted");
}
