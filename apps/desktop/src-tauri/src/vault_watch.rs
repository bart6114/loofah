use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tauri_plugin_notify::FileChanged;
use tauri_specta::Event;

use crate::session_store::SessionStore;

pub fn spawn(app: AppHandle) {
    let Some(store) = app
        .try_state::<Arc<SessionStore>>()
        .map(|state| state.inner().clone())
    else {
        tracing::error!(
            "vault watch: session store is not managed; external-edit ingestion is disabled"
        );
        return;
    };

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    FileChanged::listen(&app, move |event| {
        let _ = tx.send(event.payload.path);
    });

    tauri::async_runtime::spawn(async move {
        hypr_loofah_core::vault_watch::run(store, rx).await;
    });
}
