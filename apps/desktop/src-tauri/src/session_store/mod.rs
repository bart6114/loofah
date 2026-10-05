//! Desktop face of the vault write path: re-exports `hypr-vault-write` (the extracted
//! `SessionStore` and friends), declares the Tauri IPC glue (`commands.rs`), and wires
//! the store's index-change stream to the `index-changed` Tauri event.

use std::sync::Arc;

pub use hypr_vault_write::*;

pub mod commands;

/// Wire the managed store's change stream to the `index-changed` Tauri event
/// (emitted app-wide, i.e. to every webview). Same startup shape as
/// `vault_watch::spawn`; call once from `lib.rs`'s setup after the store is managed.
pub fn spawn_dispatcher(app: tauri::AppHandle) {
    use tauri::Manager;

    let Some(store) = app
        .try_state::<Arc<SessionStore>>()
        .map(|state| state.inner().clone())
    else {
        tracing::error!(
            "index events: session store is not managed; index-changed emission is disabled"
        );
        return;
    };

    let Some(rx) = store.take_index_change_receiver() else {
        tracing::error!("index events: dispatcher already spawned");
        return;
    };

    tauri::async_runtime::spawn(async move {
        index::run_index_change_dispatcher(rx, move |event| {
            use tauri_specta::Event;
            if let Err(error) = event.emit(&app) {
                tracing::warn!(%error, "index events: failed to emit index-changed");
            }
        })
        .await;
    });
}
