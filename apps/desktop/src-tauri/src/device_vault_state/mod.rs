use std::sync::Arc;

use hypr_loofah_core::VaultContext;
use hypr_loofah_protocol::{ApplicationError, ErrorCode};
use tauri::Manager;
use tauri_specta::Event;

mod store;
use store::DeviceStore;

#[derive(Default)]
pub struct DeviceVaultState(tokio::sync::OnceCell<Arc<DeviceStore>>);

#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type, Event)]
pub struct DeviceVaultStateChanged {
    pub scope: String,
}

async fn store<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Arc<DeviceStore>, ApplicationError> {
    app.state::<DeviceVaultState>()
        .0
        .get_or_try_init(|| async {
            let parent = app.path().app_local_data_dir().map_err(|_| unavailable())?;
            DeviceStore::open(parent).await
        })
        .await
        .cloned()
}

fn legacy<R: tauri::Runtime>(app: &tauri::AppHandle<R>, scope: &str) -> Option<Arc<VaultContext>> {
    app.try_state::<Arc<VaultContext>>()
        .and_then(|context| (context.events.vault_id() == scope).then(|| context.inner().clone()))
}

async fn access<R: tauri::Runtime, T: Send + 'static>(
    app: tauri::AppHandle<R>,
    scope: String,
    operation: impl FnOnce(&mut store::Entry) -> Result<T, ApplicationError> + Send + 'static,
) -> Result<T, ApplicationError> {
    let storage = store(&app).await?;
    let local = legacy(&app, &scope);
    storage
        .run(scope, local, operation, move |scope| {
            if let Err(error) = (DeviceVaultStateChanged { scope }).emit(&app) {
                tracing::debug!(%error, "device_vault_state_notification_failed");
            }
        })
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn device_recent_sessions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
) -> Result<Vec<String>, ApplicationError> {
    access(app, scope, |entry| Ok(entry.recent_sessions.clone())).await
}

#[tauri::command]
#[specta::specta]
pub async fn device_remember_session<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
    id: String,
) -> Result<Vec<String>, ApplicationError> {
    hypr_loofah_protocol::validate_session_id(&id)
        .map_err(|_| invalid("Invalid recent session ID"))?;
    access(app, scope, move |entry| {
        entry.recent_sessions.retain(|current| current != &id);
        entry.recent_sessions.insert(0, id);
        entry.recent_sessions.truncate(10);
        Ok(entry.recent_sessions.clone())
    })
    .await
}

#[tauri::command]
#[specta::specta]
pub async fn device_pinned_tabs<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    scope: String,
) -> Result<Option<String>, ApplicationError> {
    access(app, scope, |entry| Ok(entry.pinned_tabs.clone())).await
}

#[tauri::command]
#[specta::specta]
pub async fn device_set_pinned_tabs<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    scope: String,
    value: String,
) -> Result<(), ApplicationError> {
    if window.label() != "main" {
        return Err(invalid("Pinned tabs are saved from the main window"));
    }
    store::validate_pins(&value)?;
    access(app, scope, move |entry| {
        entry.pinned_tabs = Some(value);
        Ok(())
    })
    .await
}

fn invalid(message: &str) -> ApplicationError {
    hypr_loofah_client::failure(ErrorCode::Validation, message)
}
fn unavailable() -> ApplicationError {
    hypr_loofah_client::failure(
        ErrorCode::Unavailable,
        "Device vault preferences could not be read or saved",
    )
}

#[cfg(test)]
mod tests;
