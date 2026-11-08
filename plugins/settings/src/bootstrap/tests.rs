use super::*;
use hypr_loofah_client::{DesktopStartupSaveOutcome, SaveDesktopStartupRequest};
use serde_json::json;

fn root() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

async fn selected_remote(global: &Path) -> DesktopBootstrap {
    let store = ConnectionProfiles::new(global.join("connections")).unwrap();
    let lease = store
        .create("company", "https://company.example")
        .await
        .unwrap();
    let profile = lease.profile().await;
    drop(lease);
    let before = store.desktop_startup().unwrap();
    assert!(matches!(
        store
            .save_desktop_startup(SaveDesktopStartupRequest {
                selection: DesktopStartupSelection::Remote {
                    profile_name: profile.name,
                    profile_id: profile.id
                },
                expected_revision: before.revision,
            })
            .await
            .unwrap(),
        DesktopStartupSaveOutcome::Saved(_)
    ));
    DesktopBootstrap::load(global).unwrap()
}

#[test]
fn default_local_keeps_the_existing_vault_config_location_without_profile_provisioning() {
    let root = root();
    let vault = root.path().join("existing-vault");
    let bootstrap = DesktopBootstrap::load(root.path()).unwrap();
    assert!(bootstrap.is_local());
    let (config_base, snapshot) = bootstrap.settings_location(|| Ok(vault.clone())).unwrap();
    assert_eq!(config_base, vault);
    assert_eq!(snapshot.startup_vault_base(), Some(&vault));
    assert!(!root.path().join("connections").exists());
    assert!(!root.path().join("device-preferences").exists());
}

#[tokio::test]
async fn remote_preferences_never_resolve_or_read_a_local_vault_or_legacy_settings() {
    use crate::SettingsPluginExt;
    let root = root();
    let vault = root.path().join("unavailable-local-vault");
    std::fs::create_dir_all(&vault).unwrap();
    let canary =
        json!({"theme":"dark","custom_summary_instructions":"PRIVATE LOCAL CONFIG"}).to_string();
    let local_path = hypr_storage::vault::compute_config_path(&vault);
    std::fs::write(&local_path, &canary).unwrap();
    std::fs::write(
        hypr_storage::vault::compute_settings_path(root.path()),
        json!({"PRIVATE LOCAL SETTINGS":true}).to_string(),
    )
    .unwrap();
    let bootstrap = selected_remote(root.path()).await;
    assert!(!bootstrap.is_local());
    let (device, snapshot) = bootstrap
        .settings_location(|| panic!("remote startup resolved a local vault"))
        .unwrap();
    assert_eq!(device, root.path().join("device-preferences"));
    assert!(snapshot.startup_vault_base().is_none());
    assert_eq!(
        snapshot
            .load_with_legacy_fallback(root.path())
            .await
            .unwrap(),
        json!({})
    );
    let state = crate::ConfigState::load_or_default(&device);
    assert!(state.snapshot().custom_summary_instructions.is_empty());
    state
        .set_values(std::collections::HashMap::from([(
            "theme".into(),
            json!("light"),
        )]))
        .await
        .unwrap();
    assert_eq!(std::fs::read_to_string(&local_path).unwrap(), canary);
    let app = tauri::test::mock_builder()
        .manage(bootstrap)
        .manage(snapshot)
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    assert!(matches!(
        app.settings().vault_base(),
        Err(crate::Error::LocalVaultUnavailable)
    ));
    assert!(matches!(
        app.settings().resolve_startup_vault_base(),
        Err(crate::Error::LocalVaultUnavailable)
    ));
    let new_vault = root.path().join("must-not-create-local-vault");
    assert!(matches!(
        app.settings()
            .set_vault_base(camino::Utf8PathBuf::from_path_buf(new_vault.clone()).unwrap())
            .await,
        Err(crate::Error::LocalVaultUnavailable)
    ));
    assert!(!new_vault.exists());
    assert_eq!(std::fs::read_to_string(&local_path).unwrap(), canary);
}

#[tokio::test]
async fn malformed_startup_preference_uses_device_settings_and_preserves_the_error_and_bytes() {
    let root = root();
    let profiles = root.path().join("connections");
    std::fs::create_dir_all(&profiles).unwrap();
    let path = profiles.join("desktop-startup.json");
    let bytes = "{\"schema_version\":99,\"selection\":{\"kind\":\"future\"}}";
    std::fs::write(&path, bytes).unwrap();
    let bootstrap = DesktopBootstrap::load(root.path()).unwrap();
    assert!(bootstrap.startup().is_err());
    assert!(!bootstrap.is_local());
    let (device, snapshot) = bootstrap
        .settings_location(|| panic!("invalid selection fell back to local"))
        .unwrap();
    assert_eq!(device, root.path().join("device-preferences"));
    assert!(snapshot.startup_vault_base().is_none());
    assert_eq!(std::fs::read_to_string(path).unwrap(), bytes);
}

#[cfg(unix)]
#[tokio::test]
async fn remote_device_preferences_reject_symbolic_links_to_a_local_vault() {
    use std::os::unix::fs::symlink;
    let root = root();
    let bootstrap = selected_remote(root.path()).await;
    let vault = root.path().join("local-vault");
    std::fs::create_dir_all(&vault).unwrap();
    let canary = vault.join("config.json");
    std::fs::write(&canary, "PRIVATE LOCAL CONFIG").unwrap();
    let device = root.path().join("device-preferences");
    symlink(&vault, &device).unwrap();
    assert!(
        bootstrap
            .settings_location(|| panic!("remote fallback"))
            .is_err()
    );
    std::fs::remove_file(&device).unwrap();
    std::fs::create_dir(&device).unwrap();
    symlink(&canary, device.join("config.json")).unwrap();
    assert!(
        bootstrap
            .settings_location(|| panic!("remote fallback"))
            .is_err()
    );
    assert_eq!(
        std::fs::read_to_string(canary).unwrap(),
        "PRIVATE LOCAL CONFIG"
    );
}
