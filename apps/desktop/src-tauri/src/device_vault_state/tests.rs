use super::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

fn local(path: &std::path::Path, scope: &str) -> Arc<VaultContext> {
    Arc::new(VaultContext::new(
        scope.into(),
        Arc::new(hypr_vault_write::SessionStore::new(path.into())),
        hypr_loofah_core::startup::StartupState::new(Some(path)),
        Arc::new(hypr_loofah_core::settings::ConfigState::load_or_default(
            path,
        )),
        Arc::new(hypr_loofah_core::search::SearchService::default()),
    ))
}

async fn read(
    storage: &Arc<DeviceStore>,
    scope: &str,
    local: Option<Arc<VaultContext>>,
) -> store::Entry {
    storage
        .run(scope.into(), local, |entry| Ok(entry.clone()), |_| {})
        .await
        .unwrap()
}

async fn remember(storage: &Arc<DeviceStore>, scope: &str, id: &str) {
    let id = id.to_owned();
    storage
        .run(
            scope.into(),
            None,
            move |entry| {
                entry.recent_sessions.retain(|value| value != &id);
                entry.recent_sessions.insert(0, id);
                entry.recent_sessions.truncate(10);
                Ok(())
            },
            |_| {},
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn migration_binds_both_legacy_fields_once_to_the_exact_managed_local_scope() {
    let device = tempfile::tempdir().unwrap();
    let vault = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let pins = json!([{"type":"sessions","id":"same","pinned":true}]).to_string();
    let legacy = json!({"desktop":json!({"RecentlyOpenedSessions":json!(["same","older","same","../invalid"]).to_string(),"PinnedTabs":pins,"Unknown":"KEEP"}).to_string(),"unknown":"KEEP"}).to_string();
    std::fs::write(vault.path().join("store.json"), &legacy).unwrap();
    std::fs::write(
        other.path().join("store.json"),
        json!({"desktop":json!({"RecentlyOpenedSessions":"[\"FOREIGN\"]"}).to_string()})
            .to_string(),
    )
    .unwrap();
    let storage = DeviceStore::open(device.path().into()).await.unwrap();
    let context = local(vault.path(), "local:first");
    assert_eq!(
        read(&storage, "remote", Some(context.clone())).await,
        store::Entry::default()
    );
    assert!(!device.path().join("device-vault-state/state.json").exists());
    let entry = read(&storage, "local:first", Some(context.clone())).await;
    assert_eq!(entry.recent_sessions, vec!["same", "older"]);
    assert_eq!(entry.pinned_tabs.as_deref(), Some(pins.as_str()));
    assert_eq!(
        std::fs::read_to_string(vault.path().join("store.json")).unwrap(),
        legacy
    );
    std::fs::write(vault.path().join("store.json"), "{malformed later").unwrap();
    assert_eq!(
        read(&storage, "local:first", Some(context))
            .await
            .recent_sessions,
        vec!["same", "older"]
    );
    assert_eq!(
        read(
            &storage,
            "local:second",
            Some(local(other.path(), "local:second"))
        )
        .await,
        store::Entry::default()
    );
    let state: Value = serde_json::from_slice(
        &std::fs::read(device.path().join("device-vault-state/state.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(state["legacy_bound_scope"], "local:first");
    assert!(!state.to_string().contains("FOREIGN"));
}

#[tokio::test]
async fn concurrent_remembers_are_bounded_atomic_and_preserve_pins_and_other_scopes() {
    let device = tempfile::tempdir().unwrap();
    let storage = DeviceStore::open(device.path().into()).await.unwrap();
    remember(&storage, "second", "same").await;
    let mut work = Vec::new();
    for i in 0..24 {
        let storage = storage.clone();
        work.push(tokio::spawn(async move {
            remember(&storage, "first", &format!("note-{i}")).await;
        }));
    }
    let pins = json!([{"type":"settings","state":{"tab":"general"}}]).to_string();
    let value = pins.clone();
    storage
        .run(
            "first".into(),
            None,
            move |entry| {
                entry.pinned_tabs = Some(value);
                Ok(())
            },
            |_| {},
        )
        .await
        .unwrap();
    for job in work {
        job.await.unwrap();
    }
    let first = read(&storage, "first", None).await;
    assert_eq!(first.recent_sessions.len(), 10);
    assert_eq!(
        first
            .recent_sessions
            .iter()
            .collect::<std::collections::HashSet<_>>()
            .len(),
        10
    );
    assert_eq!(first.pinned_tabs, Some(pins));
    assert_eq!(
        read(&storage, "second", None).await.recent_sessions,
        vec!["same"]
    );
    let expected = first.clone();
    drop(storage);
    let reopened = DeviceStore::open(device.path().into()).await.unwrap();
    assert_eq!(read(&reopened, "first", None).await, expected);
}

#[tokio::test]
async fn malformed_future_and_oversized_preferences_fail_without_replacing_the_file() {
    let device = tempfile::tempdir().unwrap();
    let storage = DeviceStore::open(device.path().into()).await.unwrap();
    remember(&storage, "scope", "note").await;
    let path = device.path().join("device-vault-state/state.json");
    for bytes in [
        "{broken".to_owned(),
        json!({"schema_version":2,"legacy_bound_scope":null,"scopes":{}}).to_string(),
        "x".repeat(2 * 1024 * 1024 + 1),
    ] {
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            storage
                .run(
                    "scope".into(),
                    None,
                    |entry| {
                        entry.recent_sessions.clear();
                        Ok(())
                    },
                    |_| {}
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
    }
    assert!(store::validate_pins(&json!(vec![Value::Null; 129]).to_string()).is_err());
    assert!(store::validate_pins(&format!("[\"{}\"]", "x".repeat(256 * 1024))).is_err());
    assert!(store::validate_pins(&json!(vec![Value::Null; 128]).to_string()).is_ok());
}

#[tokio::test]
async fn dropping_a_waiter_does_not_cancel_its_atomic_write_or_notification() {
    let device = tempfile::tempdir().unwrap();
    let storage = DeviceStore::open(device.path().into()).await.unwrap();
    let changes = Arc::new(AtomicUsize::new(0));
    let updated = changes.clone();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel();
    let blocked = storage.clone();
    let holder = std::thread::spawn(move || {
        let _guard = blocked.gate.lock().unwrap();
        entered.send(()).unwrap();
        wait.recv().unwrap();
    });
    ready.await.unwrap();
    let caller = storage.clone();
    let pending = tokio::spawn(async move {
        caller
            .run(
                "scope".into(),
                None,
                |entry| {
                    entry.recent_sessions = vec!["note".into()];
                    Ok(())
                },
                move |_| {
                    updated.fetch_add(1, Ordering::SeqCst);
                },
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while Arc::strong_count(&storage) < 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    pending.abort();
    assert!(pending.await.unwrap_err().is_cancelled());
    release.send(()).unwrap();
    holder.join().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while changes.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        read(&storage, "scope", None).await.recent_sessions,
        vec!["note"]
    );
    assert_eq!(changes.load(Ordering::SeqCst), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn private_preferences_reject_symlinks_and_preserve_private_modes() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let device = tempfile::tempdir().unwrap();
    let foreign = tempfile::tempdir().unwrap();
    let storage = DeviceStore::open(device.path().into()).await.unwrap();
    remember(&storage, "scope", "note").await;
    let root = device.path().join("device-vault-state");
    let path = root.join("state.json");
    assert_eq!(
        std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::write(foreign.path().join("secret"), "CANARY").unwrap();
    std::fs::remove_file(&path).unwrap();
    symlink(foreign.path().join("secret"), &path).unwrap();
    assert!(
        storage
            .run("scope".into(), None, |entry| Ok(entry.clone()), |_| {})
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(foreign.path().join("secret")).unwrap(),
        "CANARY"
    );
    assert!(DeviceStore::open(device.path().into()).await.is_err());
}

#[tokio::test]
async fn native_device_state_needs_no_login_or_local_vault_and_only_main_writes_pins() {
    let device = tempfile::tempdir().unwrap();
    let storage = DeviceStore::open(device.path().into()).await.unwrap();
    let state = DeviceVaultState::default();
    assert!(state.0.set(storage).is_ok());
    let app = tauri::test::mock_builder()
        .manage(state)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    tauri_specta::Builder::<tauri::test::MockRuntime>::new()
        .events(tauri_specta::collect_events![DeviceVaultStateChanged])
        .mount_events(app.handle());
    let main = tauri::WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::default())
        .build()
        .unwrap();
    let other = tauri::WebviewWindowBuilder::new(&app, "note", tauri::WebviewUrl::default())
        .build()
        .unwrap();
    let scope = json!(["remote", "https://example.com/", "member", "vault"]).to_string();
    assert_eq!(
        device_recent_sessions(app.handle().clone(), scope.clone())
            .await
            .unwrap(),
        Vec::<String>::new()
    );
    assert!(
        device_pinned_tabs(app.handle().clone(), scope.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        device_remember_session(app.handle().clone(), scope.clone(), "same".into())
            .await
            .unwrap(),
        vec!["same"]
    );
    assert!(
        device_set_pinned_tabs(
            app.handle().clone(),
            other.as_ref().window().clone(),
            scope.clone(),
            "[]".into()
        )
        .await
        .is_err()
    );
    device_set_pinned_tabs(
        app.handle().clone(),
        main.as_ref().window().clone(),
        scope.clone(),
        "[]".into(),
    )
    .await
    .unwrap();
    assert_eq!(
        device_pinned_tabs(app.handle().clone(), scope)
            .await
            .unwrap()
            .as_deref(),
        Some("[]")
    );
    assert!(
        device_remember_session(app.handle().clone(), "scope".into(), "../invalid".into())
            .await
            .is_err()
    );
}
