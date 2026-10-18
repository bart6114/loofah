use super::*;

struct NoSecrets;
impl CredentialStore for NoSecrets {
    fn load(&self, _: &str) -> hypr_vault_access::Result<Option<Credentials>> {
        Ok(None)
    }
    fn save(&self, _: &str, _: &Credentials) -> hypr_vault_access::Result<()> {
        Ok(())
    }
    fn remove(&self, _: &str) -> hypr_vault_access::Result<()> {
        Ok(())
    }
}

fn app(root: &std::path::Path) -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .manage(Arc::new(
            ConnectionState::open(root.into(), Arc::new(NoSecrets)).unwrap(),
        ))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap()
}

#[tokio::test]
async fn local_mode_keeps_store_commands_local_and_native_processing_leases_are_released() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    assert!(
        invoke::<_, Value>(app.handle(), "session_get", json!({"session_id":"s1"}))
            .await
            .is_none()
    );
    let status = action(app.handle(), "status", json!({})).await.unwrap();
    assert_eq!(status["active"], false);
    assert_eq!(status["pending"], json!([]));
    assert_eq!(status["transfers"], json!([]));
    let lease = action(app.handle(), "begin_processing", json!({}))
        .await
        .unwrap();
    assert_eq!(
        action(app.handle(), "status", json!({})).await.unwrap()["processing"],
        true
    );
    action(app.handle(), "end_processing", json!({"id":lease}))
        .await
        .unwrap();
    assert_eq!(
        action(app.handle(), "status", json!({})).await.unwrap()["processing"],
        false
    );
    assert_eq!(
        action(app.handle(), "unknown", json!({}))
            .await
            .unwrap_err()
            .code,
        "invalid_request"
    );
}

#[tokio::test]
async fn standalone_note_windows_block_destination_changes_before_any_configuration_is_changed() {
    let root = tempfile::tempdir().unwrap();
    let app = app(root.path());
    require_main_only(app.handle()).unwrap();
    let note = tauri::WebviewWindowBuilder::new(
        &app,
        "note-s1",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .build()
    .unwrap();
    for operation in ["select", "logout"] {
        let error = action(app.handle(), operation, json!({"vault_id":null}))
            .await
            .unwrap_err();
        assert_eq!(error.code, "conflict");
        assert!(error.message.contains("standalone note windows"));
    }
    assert!(app.state::<Arc<ConnectionState>>().profile().is_none());
    note.close().unwrap();
}

#[tokio::test]
async fn native_remote_command_failure_emits_revocation_instead_of_falling_back_to_local_storage() {
    use tauri::Listener;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("connection.json"),serde_json::to_vec(&json!({"base_url":"https://company.example","server_id":"server","user_id":"me","vault_id":"team"})).unwrap()).unwrap();
    let app = app(root.path());
    let revoked = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let event = revoked.clone();
    app.listen_any("company-access-revoked", move |_| {
        event.store(true, std::sync::atomic::Ordering::Release);
    });
    let result = invoke::<_, Value>(app.handle(), "session_get", json!({"session_id":"s1"}))
        .await
        .unwrap();
    assert!(result.unwrap_err().contains("unauthenticated"));
    assert!(revoked.load(std::sync::atomic::Ordering::Acquire));
    assert!(!root.path().join("sessions").exists());
}
