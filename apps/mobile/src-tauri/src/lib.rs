mod commands;
mod jobs;
mod persist;
mod providers;
mod recording;
mod summary;
mod sync;
mod transcript;

use serde::{Deserialize, Serialize};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Emitter, Manager};

static APP: OnceLock<tauri::AppHandle> = OnceLock::new();

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    #[serde(default = "default_transcription_model")]
    transcription_model: String,
    summary_provider: String,
    summary_language: String,
    #[serde(alias = "openrouter_model")]
    summary_model: String,
    summary_base_url: String,
}

fn default_transcription_model() -> String {
    "parakeet-v3".into()
}

#[cfg(test)]
mod settings_tests {
    use super::Settings;

    #[test]
    fn existing_settings_keep_the_multilingual_model() {
        let settings: Settings = serde_json::from_value(serde_json::json!({
            "summary_provider": "none",
            "summary_language": "en",
            "openrouter_model": ""
        }))
        .unwrap();
        assert_eq!(settings.transcription_model, "parakeet-v3");
        assert_eq!(settings.summary_provider, "none");
        let mut selected = settings;
        selected.transcription_model = "parakeet-v2".into();
        let restored: Settings =
            serde_json::from_slice(&serde_json::to_vec(&selected).unwrap()).unwrap();
        assert_eq!(restored.transcription_model, "parakeet-v2");
    }

    fn load_config(value: serde_json::Value) -> Settings {
        let vault = tempfile::tempdir().unwrap();
        let content = serde_json::to_vec(&value).unwrap();
        let path = vault.path().join("config.json");
        std::fs::write(&path, &content).unwrap();
        let settings = Settings::load(vault.path()).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), content);
        settings
    }

    #[test]
    fn legacy_apple_summaries_become_off_without_selecting_an_api_provider() {
        let settings = load_config(serde_json::json!({
            "mobile": {
                "transcription_model": "parakeet-v2",
                "summary_provider": "local",
                "summary_language": "nl",
                "openrouter_model": "old-model"
            }
        }));
        assert_eq!(settings.summary_provider, "none");
        assert!(settings.summary_model.is_empty());
        assert!(settings.summary_base_url.is_empty());
        assert_eq!(settings.summary_language, "nl");
        assert_eq!(settings.transcription_model, "parakeet-v2");
    }

    #[test]
    fn legacy_openrouter_provider_and_model_are_preserved() {
        let settings = load_config(serde_json::json!({
            "mobile": {
                "summary_provider": "openrouter",
                "summary_language": "fr-CA",
                "openrouter_model": "anthropic/claude-sonnet-4"
            }
        }));
        assert_eq!(settings.summary_provider, "openrouter");
        assert_eq!(settings.summary_model, "anthropic/claude-sonnet-4");
        assert_eq!(settings.summary_base_url, "https://openrouter.ai/api/v1");
        assert_eq!(settings.summary_language, "fr-CA");
    }

    #[test]
    fn desktop_selection_connection_and_language_override_legacy_mobile_settings() {
        let settings = load_config(serde_json::json!({
            "current_llm_provider": "anthropic",
            "current_llm_model": "claude-sonnet-4-6",
            "ai_language": "ja",
            "ai_providers": {
                "llm:anthropic": { "type": "llm", "base_url": "https://proxy.example/anthropic/v1", "api_key": "desktop-key" }
            },
            "mobile": {
                "summary_provider": "openrouter",
                "summary_language": "nl",
                "openrouter_model": "legacy/model",
                "summary_base_url": "https://legacy.example/v1"
            }
        }));
        assert_eq!(settings.summary_provider, "anthropic");
        assert_eq!(settings.summary_model, "claude-sonnet-4-6");
        assert_eq!(
            settings.summary_base_url,
            "https://proxy.example/anthropic/v1"
        );
        assert_eq!(settings.summary_language, "ja");
    }

    #[test]
    fn desktop_off_selection_clears_the_legacy_mobile_model() {
        for provider in [
            serde_json::json!(""),
            serde_json::json!("none"),
            serde_json::Value::Null,
        ] {
            let settings = load_config(serde_json::json!({
                "current_llm_provider": provider,
                "current_llm_model": "stale-model",
                "mobile": { "summary_provider": "openrouter", "openrouter_model": "legacy/model" }
            }));
            assert_eq!(settings.summary_provider, "none");
            assert!(settings.summary_model.is_empty());
            assert!(settings.summary_base_url.is_empty());
        }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            transcription_model: default_transcription_model(),
            summary_provider: "none".into(),
            summary_language: "en".into(),
            summary_model: String::new(),
            summary_base_url: String::new(),
        }
    }
}

impl Settings {
    fn load(vault: &std::path::Path) -> Result<Self, String> {
        let config = hypr_storage::config::read_config(vault).map_err(|e| e.to_string())?;
        // An explicit null means Off; Option alone loses the distinction from legacy absence.
        let has_shared_selection = match std::fs::read(vault.join("config.json")) {
            Ok(content) => serde_json::from_slice::<serde_json::Value>(&content)
                .map_err(|e| e.to_string())?
                .get("current_llm_provider")
                .is_some(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
            Err(error) => return Err(error.to_string()),
        };
        let mut settings: Self = config
            .extra
            .get("mobile")
            .cloned()
            .map(serde_json::from_value)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        settings.summary_language = config.ai_language;
        if has_shared_selection {
            let provider = config.current_llm_provider.unwrap_or_default();
            if provider.trim().is_empty() || provider == "none" {
                settings.summary_provider = "none".into();
                settings.summary_model.clear();
            } else {
                settings.summary_provider = provider;
                settings.summary_model = config.current_llm_model.unwrap_or_default();
            }
        } else if settings.summary_provider == "local" {
            settings.summary_provider = "none".into();
            settings.summary_model.clear();
        }
        settings.summary_base_url = config
            .ai_providers
            .get(&format!("llm:{}", settings.summary_provider))
            .map(|p| p.base_url.clone())
            .filter(|url| !url.trim().is_empty())
            .or_else(|| {
                providers::definition(&settings.summary_provider).and_then(|p| {
                    p.get("baseUrl")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned)
                })
            })
            .unwrap_or_default();
        Ok(settings)
    }
}

pub struct MobileState {
    store: Arc<hypr_vault_write::SessionStore>,
    state_dir: PathBuf,
    jobs: Mutex<Vec<jobs::Job>>,
    settings: Mutex<Settings>,
    operation: tokio::sync::Mutex<()>,
    worker: AtomicBool,
    model_busy: AtomicBool,
    dirty: AtomicBool,
    sync: Mutex<sync::SyncState>,
    startup_errors: Mutex<Vec<String>>,
}

impl MobileState {
    fn session_dir(&self, id: &str) -> Result<PathBuf, String> {
        let relative =
            hypr_vault_read::paths::validated_session_dir(id).map_err(|e| e.to_string())?;
        let path = self.store.vault_base().join(relative);
        let meta = std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
        if meta.file_type().is_symlink() || !meta.is_dir() {
            return Err("Invalid session directory".into());
        }
        if !path.join("_meta.json").is_file() {
            return Err("Session no longer exists".into());
        }
        Ok(path)
    }

    fn changed(&self, app: &tauri::AppHandle) {
        self.dirty.store(true, Ordering::Release);
        let _ = app.emit("mobile-state-changed", ());
    }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn loofah_mobile_event(json: *const std::ffi::c_char) {
    if json.is_null() {
        return;
    }
    let bytes = unsafe { std::ffi::CStr::from_ptr(json) };
    if let Ok(event) = serde_json::from_slice::<serde_json::Value>(bytes.to_bytes()) {
        if let Some(kind) = event.get("type").and_then(|v| v.as_str()) {
            recording::native_event(kind);
        }
        if let Some(app) = APP.get() {
            if event.get("type").and_then(|v| v.as_str()) == Some("stop_recording") {
                if let Some(id) = event.get("session_id").and_then(|v| v.as_str()) {
                    let app = app.clone();
                    let id = id.to_string();
                    tauri::async_runtime::spawn(async move {
                        if let Err(error) = commands::stop_recording(app.clone(), Some(id)).await {
                            let state = app.state::<MobileState>();
                            state
                                .startup_errors
                                .lock()
                                .unwrap()
                                .push(format!("The recording could not be finalized: {error}"));
                            state.changed(&app);
                        }
                    });
                }
            }
            if event.get("type").and_then(|v| v.as_str()) == Some("icloud_moved") {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let state = app.state::<MobileState>();
                    let _guard = state.operation.lock().await;
                    state.sync.lock().unwrap().disconnected("The iCloud vault moved or became unavailable. Reconnect the original folder.");
                    state.changed(&app);
                });
            }
            if matches!(
                event.get("type").and_then(|v| v.as_str()),
                Some("foreground" | "icloud_changed")
            ) {
                app.state::<MobileState>()
                    .dirty
                    .store(true, Ordering::Release);
            }
            let _ = app.emit("mobile-state-changed", event);
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_mobile_native::init())
        .plugin(tauri_plugin_http::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(providers::Bridge::default())
        .invoke_handler(tauri::generate_handler![
            commands::mobile_snapshot,
            commands::mobile_session,
            commands::mobile_open_attachment,
            commands::mobile_create_session,
            commands::mobile_delete_session,
            commands::mobile_restore_session,
            commands::mobile_copy_text,
            commands::mobile_generate_title,
            transcript::mobile_transcript_export,
            commands::mobile_update_session,
            commands::mobile_start_recording,
            commands::mobile_stop_recording,
            commands::mobile_transcribe,
            commands::mobile_cancel_job,
            commands::mobile_dismiss_failed_jobs,
            commands::mobile_download_model,
            commands::mobile_select_model,
            commands::mobile_delete_model,
            commands::mobile_import_audio,
            commands::mobile_summarize,
            commands::mobile_save_settings,
            providers::mobile_provider_api_key,
            providers::mobile_summary_bridge_ready,
            providers::mobile_summary_complete,
            sync::mobile_connect_vault,
            sync::mobile_sync,
            sync::mobile_resolve_conflict,
            sync::mobile_download_audio,
        ])
        .setup(|app| {
            let root = app.path().app_local_data_dir()?;
            let vault = root.join("vault");
            let state_dir = root.join("device");
            std::fs::create_dir_all(&vault)?;
            std::fs::create_dir_all(&state_dir)?;
            let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
            let mut startup_errors = Vec::new();
            let settings = match Settings::load(&vault) {
                Ok(settings) => settings,
                Err(error) => {
                    startup_errors.push(format!(
                        "Settings could not be read; original file preserved: {error}"
                    ));
                    Settings::default()
                }
            };
            let recovered = match recording::recover(&vault, &state_dir) {
                Ok(id) => id,
                Err(error) => {
                    startup_errors.push(format!(
                        "A recording needs recovery. Its files are preserved: {error}"
                    ));
                    None
                }
            };
            let queue = match jobs::load(&state_dir) {
                Ok(queue) => queue,
                Err(error) => {
                    startup_errors.push(format!(
                        "Processing queue could not be recovered; recordings are preserved: {error}"
                    ));
                    std::fs::rename(
                        state_dir.join("jobs.json"),
                        state_dir.join(format!("jobs-recovery-{}.json", uuid::Uuid::new_v4())),
                    )?;
                    vec![]
                }
            };
            let state = MobileState {
                store: store.clone(),
                state_dir,
                jobs: Mutex::new(queue),
                settings: Mutex::new(settings),
                operation: tokio::sync::Mutex::new(()),
                worker: AtomicBool::new(false),
                model_busy: AtomicBool::new(false),
                dirty: AtomicBool::new(true),
                sync: Mutex::new(sync::SyncState::default()),
                startup_errors: Mutex::new(startup_errors),
            };
            app.manage(state);
            let handle = app.handle().clone();
            let _ = APP.set(handle.clone());
            let task_store = store.clone();
            tauri::async_runtime::spawn(async move {
                match task_store.rebuild_index().await {
                    Ok(_) => {}
                    Err(e) => {
                        handle
                            .state::<MobileState>()
                            .startup_errors
                            .lock()
                            .unwrap()
                            .push(e.to_string());
                        return;
                    }
                }
                if let Some(id) = recovered {
                    let _ = jobs::enqueue(&handle, &id, "transcribe").await;
                }
                if let Err(error) = sync::restore(&handle).await {
                    handle.state::<MobileState>().sync.lock().unwrap().error = Some(error);
                }
                let _ = handle.emit("index-changed", ());
                let mut ticks = 0u32;
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    ticks += 1;
                    if recording::FOREGROUND.load(Ordering::Acquire) {
                        jobs::kick(&handle);
                        let pending = handle
                            .state::<MobileState>()
                            .sync
                            .lock()
                            .unwrap()
                            .status()
                            .pending;
                        if ticks.is_multiple_of(15)
                            && !recording::is_active()
                            && (handle.state::<MobileState>().dirty.load(Ordering::Acquire)
                                || pending > 0
                                || ticks.is_multiple_of(150))
                        {
                            let _ = sync::reconcile(&handle).await;
                        }
                    }
                }
            });
            if let Some(rx) = store.take_index_change_receiver() {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    hypr_vault_write::index::run_index_change_dispatcher(rx, move |event| {
                        let _ = handle.emit("index-changed", event);
                    })
                    .await;
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Loofah mobile failed to start");
}
