use hypr_loofah_client::{
    ConnectionProfile, ConnectionProfiles, DesktopStartupSaveOutcome, DesktopStartupSnapshot,
    SaveDesktopStartupRequest,
};
use hypr_loofah_protocol::{ApplicationError, ErrorCode};
use serde::Serialize;
use std::sync::Arc;
use tauri::Manager;

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct DesktopConnectionProfile {
    pub id: String,
    pub name: String,
    pub server: String,
    pub member_id: Option<String>,
    pub vault_id: Option<String>,
}

impl From<ConnectionProfile> for DesktopConnectionProfile {
    fn from(profile: ConnectionProfile) -> Self {
        Self {
            id: profile.id,
            name: profile.name,
            server: profile.server,
            member_id: profile.member_id,
            vault_id: profile.vault_id,
        }
    }
}

#[derive(Clone, Debug, Serialize, specta::Type)]
pub struct DesktopBootstrapInfo {
    pub startup: DesktopStartupSnapshot,
    pub connection: Option<DesktopConnectionProfile>,
}

fn profiles<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Arc<ConnectionProfiles>, ApplicationError> {
    app.try_state::<tauri_plugin_settings::DesktopBootstrap>()
        .map(|state| state.profiles())
        .ok_or_else(|| {
            hypr_loofah_client::failure(
                ErrorCode::Unavailable,
                "Desktop startup is still in progress",
            )
        })
}

#[tauri::command]
#[specta::specta]
pub fn application_desktop_bootstrap<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<DesktopBootstrapInfo, ApplicationError> {
    let bootstrap = app
        .try_state::<tauri_plugin_settings::DesktopBootstrap>()
        .ok_or_else(|| {
            hypr_loofah_client::failure(
                ErrorCode::Unavailable,
                "Desktop startup is still in progress",
            )
        })?;
    let startup = bootstrap.startup()?;
    let connection = bootstrap
        .profiles()
        .desktop_startup_profile(&startup)?
        .map(Into::into);
    Ok(DesktopBootstrapInfo {
        startup,
        connection,
    })
}

#[tauri::command]
#[specta::specta]
pub fn application_desktop_startup<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<DesktopStartupSnapshot, ApplicationError> {
    profiles(&app)?.desktop_startup()
}

#[tauri::command]
#[specta::specta]
pub async fn application_desktop_connections<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<DesktopConnectionProfile>, ApplicationError> {
    Ok(profiles(&app)?
        .list()
        .await?
        .into_iter()
        .map(Into::into)
        .collect())
}

#[tauri::command]
#[specta::specta]
pub async fn application_create_desktop_connection<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    name: String,
    server: String,
) -> Result<DesktopConnectionProfile, ApplicationError> {
    let profile = profiles(&app)?.create(&name, &server).await?;
    Ok(profile.profile().await.into())
}

#[tauri::command]
#[specta::specta]
pub async fn application_save_desktop_startup<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input: SaveDesktopStartupRequest,
) -> Result<DesktopStartupSaveOutcome, ApplicationError> {
    profiles(&app)?.save_desktop_startup(input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_remember_desktop_destination<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    profile_id: String,
    member_id: String,
    vault_id: String,
) -> Result<(), ApplicationError> {
    hypr_loofah_protocol::validate_session_id(&member_id)
        .and_then(|_| hypr_loofah_protocol::validate_session_id(&vault_id))
        .map_err(|_| {
            hypr_loofah_client::failure(ErrorCode::Validation, "Invalid saved vault destination")
        })?;
    let (_, lease) = app.state::<crate::remote::RemoteState>().profile(
        &connection_id,
        window.label(),
        &profile_id,
    )?;
    let mut profile = lease.profile().await;
    if profile.id != profile_id {
        return Err(hypr_loofah_client::failure(
            ErrorCode::Conflict,
            "The saved server connection changed; reload it before continuing",
        ));
    }
    if profile.member_id.as_ref() == Some(&member_id)
        && profile.vault_id.as_ref() == Some(&vault_id)
    {
        return Ok(());
    }
    profile.member_id = Some(member_id);
    profile.vault_id = Some(vault_id);
    lease.save(profile).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypr_loofah_client::{DesktopStartupSaveOutcome, DesktopStartupSelection};

    #[test]
    fn default_bootstrap_needs_no_profile_login_or_local_vault_state() {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let app = tauri::test::mock_builder()
            .manage(tauri_plugin_settings::DesktopBootstrap::load(root.path()).unwrap())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let info = application_desktop_bootstrap(app.handle().clone()).unwrap();
        assert_eq!(info.startup.selection, DesktopStartupSelection::Local {});
        assert!(info.connection.is_none());
        assert!(!root.path().join("connections").exists());
        assert!(
            app.try_state::<Arc<crate::session_store::SessionStore>>()
                .is_none()
        );
    }

    #[tokio::test]
    async fn remote_bootstrap_keeps_immutable_identity_and_does_not_fall_back_when_profile_disappears()
     {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let profiles = ConnectionProfiles::new(root.path().join("connections")).unwrap();
        let lease = profiles
            .create("company", "https://company.example")
            .await
            .unwrap();
        let mut profile = lease.profile().await;
        profile.credential_accounts.push("a".repeat(64));
        lease.save(profile.clone()).await.unwrap();
        drop(lease);
        let selected = DesktopStartupSelection::Remote {
            profile_name: profile.name.clone(),
            profile_id: profile.id.clone(),
        };
        assert!(matches!(
            profiles
                .save_desktop_startup(SaveDesktopStartupRequest {
                    selection: selected.clone(),
                    expected_revision: "missing".into()
                })
                .await
                .unwrap(),
            DesktopStartupSaveOutcome::Saved(_)
        ));
        let app = tauri::test::mock_builder()
            .manage(tauri_plugin_settings::DesktopBootstrap::load(root.path()).unwrap())
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let info = application_desktop_bootstrap(app.handle().clone()).unwrap();
        assert_eq!(info.startup.selection, selected);
        let summary = serde_json::to_value(info.connection.unwrap()).unwrap();
        assert_eq!(summary["id"], profile.id);
        assert!(summary.get("credential_accounts").is_none());
        profiles
            .open("company")
            .await
            .unwrap()
            .remove()
            .await
            .unwrap();
        assert_eq!(
            application_desktop_bootstrap(app.handle().clone())
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        let replacement = profiles
            .create("company", "https://company.example")
            .await
            .unwrap();
        assert_ne!(replacement.profile().await.id, profile.id);
        assert_eq!(
            application_desktop_bootstrap(app.handle().clone())
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert!(
            app.try_state::<Arc<crate::session_store::SessionStore>>()
                .is_none()
        );
    }
}
