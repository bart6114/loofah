use std::path::Path;
use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tauri_plugin_notify::NotifyPluginExt;
use tauri_specta::Event;

use crate::session_store::SessionStore;
pub use hypr_loofah_core::startup::{StartupPhase, StartupState, StartupStatus};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct StartupProgress {
    pub status: StartupStatus,
}

pub fn state(app: &AppHandle, vault_path: Option<&Path>) -> StartupState {
    let app = app.clone();
    StartupState::new(vault_path).with_observer(move |status| {
        if let Err(error) = (StartupProgress { status }).emit(&app) {
            tracing::warn!(%error, "failed to emit startup progress");
        }
    })
}

#[tauri::command]
#[specta::specta]
pub fn get_startup_status<R: tauri::Runtime>(app: AppHandle<R>) -> StartupStatus {
    app.state::<StartupState>().snapshot()
}

pub fn spawn(app: AppHandle, store: Arc<SessionStore>) {
    let state = app.state::<StartupState>().inner().clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = hypr_loofah_core::startup::prepare(store.clone(), state.clone()).await {
            tracing::error!(%error, "vault startup failed");
            state.set_phase(StartupPhase::Failed {
                message: error.to_string(),
            });
            return;
        }
        crate::vault_watch::spawn(app.clone());
        crate::recording_meta::spawn(app.clone());
        store.set_startup_pending(false);
        state.set_phase(StartupPhase::Ready);
        tokio::task::spawn_blocking(move || {
            if let Err(error) = app.notify().start() {
                tracing::error!(%error, "failed to start vault watcher");
            }
        });
    });
}
