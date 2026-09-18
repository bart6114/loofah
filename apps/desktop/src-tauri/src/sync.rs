use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tauri::{AppHandle, Manager};
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_specta::Event;
use vault_sync::{
    owner::{self, Command, Owner},
    remote::{AuthorizationClient, Environment},
};
#[derive(Clone, Default, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    enabled: bool,
    phase: String,
    vault_path: String,
    browser_url: Option<String>,
    user_code: Option<String>,
    pairing_code: Option<String>,
    pairing_role: Option<String>,
    account_email: Option<String>,
    current_device_id: Option<String>,
    has_started: bool,
    up_to_date: bool,
    notice: Option<String>,
    error: Option<String>,
    last_success: Option<u64>,
    pending_work: usize,
    active_transfers: usize,
    transferred_bytes: u64,
    transfer_bytes: u64,
    used_bytes: u64,
    quota_bytes: u64,
    conflicts: Vec<SyncConflict>,
    devices: Vec<Device>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct Device {
    id: String,
    name: Option<String>,
    public_key: String,
    enrolled_at: u64,
    revoked_at: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct SyncStatusChanged {
    pub status: SyncStatus,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncEntity {
    Session { id: String },
    People,
    Tags,
    Tasks,
}
impl From<vault_sync::scope::Entity> for SyncEntity {
    fn from(value: vault_sync::scope::Entity) -> Self {
        use vault_sync::scope::Entity;
        match value {
            Entity::Session(id) => Self::Session { id },
            Entity::People => Self::People,
            Entity::Tags => Self::Tags,
            Entity::Tasks => Self::Tasks,
        }
    }
}
impl From<SyncEntity> for vault_sync::scope::Entity {
    fn from(value: SyncEntity) -> Self {
        match value {
            SyncEntity::Session { id } => Self::Session(id),
            SyncEntity::People => Self::People,
            SyncEntity::Tags => Self::Tags,
            SyncEntity::Tasks => Self::Tasks,
        }
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SyncConflict {
    pub entity: SyncEntity,
    pub local: String,
    pub cloud: String,
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SyncAction {
    Connect,
    ReopenBrowser,
    CopyCode,
    RenameDevice {
        id: String,
        name: String,
    },
    Cancel,
    Pause,
    Resume,
    ReconcileRecovery,
    Disconnect,
    SaveRecoveryKit,
    ImportRecoveryKit,
    RefreshDevices,
    PairNewMac,
    ApproveMac {
        code: String,
    },
    RevokeDevice {
        id: String,
    },
    PurgeVersion {
        revision: String,
    },
    History {
        entity: SyncEntity,
        before: Option<(u64, String)>,
    },
    Preview {
        entity: SyncEntity,
        revision: String,
    },
    Restore {
        entity: SyncEntity,
        revision: String,
    },
    Export {
        entity: SyncEntity,
        revision: String,
    },
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
pub struct Version {
    pub id: String,
    pub operation: String,
    pub device_id: String,
    pub created_at: u64,
    pub current: Option<u8>,
    pub pinned: u8,
}
#[derive(Clone, Serialize, Deserialize, specta::Type)]
pub struct VersionPreview {
    pub title: Option<String>,
    pub device_id: String,
    pub created_at: u64,
    pub text: String,
    pub files: Vec<String>,
    pub changed: Vec<String>,
    pub captured_at: Option<u64>,
    pub missing_references: Vec<String>,
    pub deleted: bool,
}
#[derive(Clone, Serialize, Deserialize, specta::Type)]
pub struct SyncContent {
    pub cancelled: bool,
    pub versions: Vec<Version>,
    pub preview: Option<VersionPreview>,
}

pub struct SyncState {
    status: Arc<RwLock<SyncStatus>>,
    directory: Option<std::path::PathBuf>,
    owns: Arc<std::sync::atomic::AtomicBool>,
    shutting_down: Arc<std::sync::atomic::AtomicBool>,
}
fn enabled<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    cfg!(feature = "staging") && app.config().identifier == "io.loofah.staging"
}
pub async fn recover(_app: &AppHandle, vault: &Path) -> Result<(), String> {
    let vault = vault.to_owned();
    tokio::task::spawn_blocking(move || owner::recover(&vault))
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

pub fn spawn(app: AppHandle, store: Arc<crate::session_store::SessionStore>) {
    let directory =
        owner::staging_root().and_then(|root| owner::directory(&root, store.vault_base()));
    let status = Arc::new(RwLock::new(SyncStatus {
        enabled: enabled(&app),
        phase: "disconnected".into(),
        vault_path: store.vault_base().display().to_string(),
        ..Default::default()
    }));
    let owns = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shutting_down = Arc::new(std::sync::atomic::AtomicBool::new(false));
    app.manage(SyncState {
        owns: owns.clone(),
        shutting_down: shutting_down.clone(),
        status: status.clone(),
        directory: directory.as_ref().ok().cloned(),
    });
    if !enabled(&app) {
        return;
    }
    tauri::async_runtime::spawn(async move {
        let directory = match directory {
            Ok(directory) => directory,
            Err(error) => {
                status.write().unwrap().error = Some(error.to_string());
                return;
            }
        };
        if let Err(error) = app
            .state::<crate::startup::StartupState>()
            .wait_until_ready()
            .await
        {
            status.write().unwrap().error = Some(error);
            return;
        }
        let mut owns_runtime = false;
        let mut previous = None;
        let mut generation = None;
        let mut changes = store.subscribe_index_changes();
        loop {
            if shutting_down.load(std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            if !owns_runtime
                && matches!(owner::request(&directory, Command::Status).await, Ok(None))
            {
                let credentials =
                    vault_sync::credentials::open(&directory, Environment::Staging, None, None)
                        .unwrap_or_else(|_| Arc::new(vault_sync::credentials::Locked));
                match Owner::acquire(
                    &directory,
                    store.clone(),
                    credentials,
                    AuthorizationClient::Desktop,
                )
                .await
                {
                    Ok(Some(mut runtime)) => {
                        owns_runtime = true;
                        owns.store(true, std::sync::atomic::Ordering::SeqCst);
                        tauri::async_runtime::spawn(async move {
                            while !runtime.is_quiescing() {
                                let _ = runtime.step().await;
                            }
                        });
                    }
                    Ok(None) => {}
                    Err(error) => {
                        status.write().unwrap().error = Some(error.to_string());
                    }
                }
            }
            let changed = tokio::select! { change = changes.recv() => { if change.is_none() { return; } true }, _ = tokio::time::sleep(Duration::from_millis(250)) => false };
            let response = match owner::request(
                &directory,
                if changed {
                    Command::Changed
                } else {
                    Command::Status
                },
            )
            .await
            {
                Ok(Some(response)) => response,
                Ok(None) => {
                    owns_runtime = false;
                    owns.store(false, std::sync::atomic::Ordering::SeqCst);
                    continue;
                }
                Err(error) => {
                    status.write().unwrap().error = Some(error.to_string());
                    continue;
                }
            };
            let current = (response.owner, response.generation);
            if !owns_runtime && generation != Some(current) {
                if let Err(error) = store.rebuild_index().await {
                    status.write().unwrap().error = Some(error.to_string());
                }
            }
            generation = Some(current);
            let Ok(next) = serde_json::to_value(response.status)
                .and_then(serde_json::from_value::<SyncStatus>)
            else {
                continue;
            };
            if previous.as_ref() != Some(&next) {
                previous = Some(next.clone());
                *status.write().unwrap() = next.clone();
                let _ = SyncStatusChanged { status: next }.emit(&app);
            }
        }
    });
}

#[tauri::command]
#[specta::specta]
pub fn sync_status<R: tauri::Runtime>(app: AppHandle<R>) -> SyncStatus {
    app.try_state::<SyncState>()
        .map(|state| state.status.read().unwrap().clone())
        .unwrap_or_default()
}

#[tauri::command]
#[specta::specta]
pub async fn sync_action<R: tauri::Runtime>(
    app: AppHandle<R>,
    action: SyncAction,
) -> Result<Option<SyncContent>, String> {
    if !enabled(&app) {
        return Err("Sync is available only in Loofah Staging.".into());
    }
    let mut value = serde_json::to_value(&action).map_err(|error| error.to_string())?;
    match &action {
        SyncAction::CopyCode => {
            let status = sync_status(app.clone());
            let code = status
                .pairing_code
                .or(status.user_code)
                .ok_or("No active code to copy.")?;
            app.clipboard()
                .write_text(code)
                .map_err(|error| error.to_string())?;
            return Ok(None);
        }
        SyncAction::ReopenBrowser => {
            let url = sync_status(app.clone())
                .browser_url
                .ok_or("Connect again to get a browser code.")?;
            app.opener()
                .open_url(url, None::<&str>)
                .map_err(|error| error.to_string())?;
            return Ok(None);
        }
        SyncAction::SaveRecoveryKit | SyncAction::ImportRecoveryKit | SyncAction::Export { .. } => {
            let path = tokio::task::block_in_place(|| match &action {
                SyncAction::SaveRecoveryKit => app
                    .dialog()
                    .file()
                    .set_file_name("Loofah Staging Recovery.loofah-key")
                    .blocking_save_file(),
                SyncAction::ImportRecoveryKit => app.dialog().file().blocking_pick_file(),
                _ => app.dialog().file().blocking_pick_folder(),
            });
            let Some(path) = path else {
                return Ok(Some(SyncContent {
                    cancelled: true,
                    versions: vec![],
                    preview: None,
                }));
            };
            let path = path.into_path().map_err(|error| error.to_string())?;
            value = match &action {
                SyncAction::SaveRecoveryKit => {
                    serde_json::json!({ "save_recovery_kit": { "path": path } })
                }
                SyncAction::ImportRecoveryKit => {
                    serde_json::json!({ "import_recovery_kit": { "path": path } })
                }
                SyncAction::Export { entity, revision } => {
                    serde_json::json!({ "export": { "entity": entity, "revision": revision, "destination": path.join(format!("loofah-version-{revision}")) } })
                }
                _ => unreachable!(),
            };
        }
        _ => {}
    }
    let action = serde_json::from_value(value).map_err(|error| error.to_string())?;
    let directory = app
        .state::<SyncState>()
        .directory
        .clone()
        .ok_or("Sync storage is unavailable.")?;
    let response = owner::request(&directory, Command::Action { action })
        .await
        .map_err(|error| error.to_string())?
        .ok_or("Sync is starting. Try again shortly.")?;
    if let Some(error) = response.error {
        return Err(error);
    }
    if response.status.phase == "authorizing" {
        if let Some(url) = &response.status.browser_url {
            let _ = app.opener().open_url(url, None::<&str>);
        }
    }
    response
        .content
        .map(|content| {
            serde_json::to_value(content)
                .and_then(serde_json::from_value)
                .map_err(|error| error.to_string())
        })
        .transpose()
}

pub async fn quiesce<R: tauri::Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    if !enabled(app) {
        return Ok(());
    }
    if let Some(state) = app.try_state::<SyncState>() {
        state
            .shutting_down
            .store(true, std::sync::atomic::Ordering::SeqCst);
        if !state.owns.load(std::sync::atomic::Ordering::SeqCst) {
            return Ok(());
        }
    }
    let Some(directory) = app
        .try_state::<SyncState>()
        .and_then(|state| state.directory.clone())
    else {
        return Ok(());
    };
    owner::request(&directory, Command::Quiesce)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}
