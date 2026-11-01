use std::sync::Arc;

use tauri::Manager;

use crate::{AppExt, embedded_cli::EmbeddedCliStatus, session_store::SessionStore};

const STAGING_BUNDLE_ID: &str = "io.loofah.staging";

#[tauri::command]
#[specta::specta]
pub async fn get_onboarding_needed<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<bool, String> {
    app.get_onboarding_needed().map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn set_onboarding_needed<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    v: bool,
) -> Result<(), String> {
    app.set_onboarding_needed(v).map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn get_dismissed_toasts<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<String>, String> {
    app.get_dismissed_toasts()
}

#[tauri::command]
#[specta::specta]
pub async fn set_dismissed_toasts<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    v: Vec<String>,
) -> Result<(), String> {
    app.set_dismissed_toasts(v)
}

fn should_show_devtool(identifier: &str) -> bool {
    cfg!(any(
        debug_assertions,
        feature = "dev",
        feature = "devtools",
        feature = "staging"
    )) || identifier == STAGING_BUNDLE_ID
}

#[tauri::command]
#[specta::specta]
pub fn show_devtool<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> bool {
    should_show_devtool(&app.config().identifier)
}

#[tauri::command]
#[specta::specta]
pub async fn complete_app_exit<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    // Last-chance flush before the process actually exits: any transcript words still sitting
    // in the debounce buffer must land on disk now, or a quit right after a burst of speech
    // could lose up to ~1s of audio's worth of words. Best-effort -- a failure here must not
    // block quitting (the user already chose to exit), just get logged.
    if let Some(store) = app.try_state::<Arc<SessionStore>>()
        && let Err(err) = store.flush_all().await
    {
        tracing::error!(error = %err, "session_store flush_all failed while completing app exit");
    }

    crate::mark_exit_flush_complete();
    app.exit(0);
}

/// Settings' "change storage location", with the session store frozen for the duration:
/// the plugin's bare `copy_vault`/`move_vault` know nothing about in-flight writes, so
/// calling them directly can copy a vault while a recording or a debounced transcript
/// flush is still landing files in it. Freezing refuses while a recording lease is held,
/// flushes the live transcript buffers, and blocks every writer until the relocation is
/// done. `keep_original` picks copy (old vault left behind) over move.
#[tauri::command]
#[specta::specta]
pub async fn relocate_vault<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    new_path: String,
    keep_original: bool,
) -> Result<(), String> {
    use tauri_plugin_settings::SettingsPluginExt;

    let store = app
        .try_state::<Arc<SessionStore>>()
        .ok_or_else(|| "session store is not initialized".to_string())?;
    let _freeze = store
        .freeze_for_vault_move()
        .await
        .map_err(|e| e.to_string())?;

    let new_path = camino::Utf8PathBuf::from(&new_path);
    if keep_original {
        app.settings()
            .copy_vault(new_path.clone())
            .await
            .map_err(|e| e.to_string())?;
        app.settings()
            .set_vault_base(new_path)
            .await
            .map_err(|e| e.to_string())
    } else {
        app.settings()
            .move_vault(new_path)
            .await
            .map_err(|e| e.to_string())
    }
}

#[tauri::command]
#[specta::specta]
pub async fn get_pinned_tabs<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<String>, String> {
    app.get_pinned_tabs()
}

#[tauri::command]
#[specta::specta]
pub async fn set_pinned_tabs<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    v: String,
) -> Result<(), String> {
    app.set_pinned_tabs(v)
}

#[tauri::command]
#[specta::specta]
pub async fn get_recently_opened_sessions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<String>, String> {
    app.get_recently_opened_sessions()
}

#[tauri::command]
#[specta::specta]
pub async fn set_recently_opened_sessions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    v: String,
) -> Result<(), String> {
    app.set_recently_opened_sessions(v)
}

#[tauri::command]
#[specta::specta]
pub async fn check_embedded_cli<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<EmbeddedCliStatus, String> {
    tauri::async_runtime::spawn_blocking(move || crate::embedded_cli::check(&app))
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn install_embedded_cli<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<EmbeddedCliStatus, String> {
    tauri::async_runtime::spawn_blocking(move || crate::embedded_cli::install(&app))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shows_devtools_for_staging_bundle() {
        assert!(should_show_devtool(STAGING_BUNDLE_ID));
    }
}
