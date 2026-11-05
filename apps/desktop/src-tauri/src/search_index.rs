use std::sync::Arc;

use tauri::Manager;
use tauri_plugin_settings::SettingsPluginExt;

use crate::session_store::SessionStore;

pub fn spawn(app: tauri::AppHandle, store: Arc<SessionStore>) {
    let startup = app.state::<crate::startup::StartupState>().inner().clone();
    let service = app
        .state::<tauri_plugin_tantivy::IndexState>()
        .service
        .clone();
    let paths = app.settings().global_base().and_then(|global| {
        app.settings()
            .vault_base()
            .map(|vault| (global.into_std_path_buf(), vault.into_std_path_buf()))
    });
    let (global, vault) = match paths {
        Ok(paths) => paths,
        Err(error) => {
            tracing::error!(%error, "search cache paths unavailable");
            return;
        }
    };
    let changes = store.subscribe_index_changes();
    tauri::async_runtime::spawn(async move {
        service
            .run(
                &global,
                &vault,
                &startup,
                changes,
                Default::default(),
                || {},
            )
            .await;
    });
}
