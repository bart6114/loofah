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
        if let Some(context) = app.try_state::<Arc<hypr_loofah_core::VaultContext>>() {
            if let Err(error) = context.recover_recording_publications().await {
                tracing::error!(%error, "recording publication recovery failed");
                state.set_phase(StartupPhase::Failed {
                    message: error.to_string(),
                });
                return;
            }
            if let Err(error) = context.recover_document_writes().await {
                tracing::error!(%error, "document recovery failed");
                state.set_phase(StartupPhase::Failed {
                    message: error.to_string(),
                });
                return;
            }
        }
        if let Err(error) = hypr_loofah_core::startup::prepare(store.clone(), state.clone()).await {
            tracing::error!(%error, "vault startup failed");
            state.set_phase(StartupPhase::Failed {
                message: error.to_string(),
            });
            return;
        }
        crate::vault_watch::spawn(app.clone());
        store.set_startup_pending(false);
        if let Some(context) = app.try_state::<Arc<hypr_loofah_core::VaultContext>>()
            && let Err(error) = app
                .state::<crate::processing::ProcessingState>()
                .start(app.clone(), &context)
                .await
        {
            tracing::error!(%error, "local processing startup failed; vault notes remain available");
        }
        if let Some(context) = app.try_state::<Arc<hypr_loofah_core::VaultContext>>()
            && let Err(error) = app
                .state::<crate::local_transcription::LocalTranscriptionState>()
                .start(app.clone(), &context)
                .await
        {
            tracing::error!(%error, "local transcription startup failed; vault notes remain available");
        }
        if let Some(context) = app.try_state::<Arc<hypr_loofah_core::VaultContext>>()
            && let Err(error) = app
                .state::<Arc<crate::local_capture::LocalCaptureState>>()
                .initialize(context.inner().clone())
                .await
        {
            tracing::error!(%error, "Local capture intent recovery failed");
            state.set_phase(StartupPhase::Failed {
                message: error.to_string(),
            });
            return;
        }
        state.set_phase(StartupPhase::Ready);
        if let Some(context) = app.try_state::<Arc<hypr_loofah_core::VaultContext>>() {
            let lease = app
                .state::<tauri_plugin_settings::LocalVaultLease>()
                .0
                .clone();
            let result = match hypr_loofah_local_owner::EndpointRoot::system() {
                Ok(endpoints) => {
                    app.state::<crate::local_owner::LocalOwnerState>()
                        .start(context.inner().clone(), lease, endpoints)
                        .await
                }
                Err(error) => Err(error.to_string()),
            };
            if let Err(error) = result {
                tracing::error!(%error, "local CLI endpoint startup failed; vault notes remain available");
            }
        }
        tokio::task::spawn_blocking(move || {
            if let Err(error) = app.notify().start() {
                tracing::error!(%error, "failed to start vault watcher");
            }
        });
    });
}
