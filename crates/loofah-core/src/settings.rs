use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

pub const CONFIG_CHANGED_EVENT: &str = "config-changed";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct AiProviderEntry {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

// Flat key space mirroring apps/desktop/src/settings/schema.ts. API keys are
// never stored here — the host resolves them from device or service credentials.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(default)]
pub struct AppConfig {
    pub autostart: bool,
    pub auto_stop_meetings: bool,
    pub floating_bar_enabled: bool,
    pub floating_bar_opacity: f64,
    pub live_caption_opacity: f64,
    pub live_caption_width: f64,
    pub live_caption_line_count: f64,
    pub live_caption_position: String,
    pub live_caption_minimized: bool,
    pub show_app_in_dock: bool,
    pub show_tray_icon: bool,
    pub theme: String,
    pub notification_detect: bool,
    pub respect_dnd: bool,
    pub cloud_sync_enabled: bool,
    pub ai_language: String,
    pub spoken_languages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meeting_languages: Option<Vec<String>>,
    pub custom_summary_instructions: String,
    pub custom_summary_instructions_token_aware: bool,
    pub auto_summary_prompt: String,
    pub auto_apply_high_confidence_tags: bool,
    pub ignored_platforms: Vec<String>,
    pub included_platforms: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_llm_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_llm_model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_stt_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_stt_model: Option<String>,
    pub transcription_timing: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone: Option<String>,
    pub ai_providers: HashMap<String, AiProviderEntry>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, Value>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            autostart: false,
            auto_stop_meetings: true,
            floating_bar_enabled: true,
            floating_bar_opacity: 0.78,
            live_caption_opacity: 0.3,
            live_caption_width: 440.0,
            live_caption_line_count: 1.0,
            live_caption_position: "topCenter".to_string(),
            live_caption_minimized: true,
            show_app_in_dock: true,
            show_tray_icon: true,
            theme: "system".to_string(),
            notification_detect: true,
            respect_dnd: false,
            cloud_sync_enabled: true,
            ai_language: "en".to_string(),
            spoken_languages: Vec::new(),
            meeting_languages: None,
            custom_summary_instructions: String::new(),
            custom_summary_instructions_token_aware: false,
            auto_summary_prompt: String::new(),
            auto_apply_high_confidence_tags: true,
            ignored_platforms: Vec::new(),
            included_platforms: Vec::new(),
            current_llm_provider: None,
            current_llm_model: None,
            current_stt_provider: None,
            current_stt_model: None,
            transcription_timing: "live".to_string(),
            timezone: None,
            ai_providers: HashMap::new(),
            extra: serde_json::Map::new(),
        }
    }
}

fn normalize_retired_stt(value: &mut Value) -> bool {
    let retired_provider = matches!(
        value.get("current_stt_provider").and_then(Value::as_str),
        Some("am" | "argmax")
    );
    let retired_model = value
        .get("current_stt_model")
        .and_then(Value::as_str)
        .is_some_and(|model| {
            model.starts_with("am-")
                || matches!(
                    model,
                    "soniqo-qwen3-small"
                        | "soniqo-qwen3-large"
                        | "aufklarer/Qwen3-ASR-0.6B-MLX-4bit"
                        | "aufklarer/Qwen3-ASR-1.7B-MLX-8bit"
                )
        });
    if retired_provider || retired_model {
        value["current_stt_provider"] = Value::String("fmtr".into());
        value["current_stt_model"] = Value::String("soniqo-parakeet-batch".into());
        return true;
    }
    false
}

fn load_config<E: std::fmt::Display>(
    path: &Path,
    persist: impl FnOnce(&str) -> Result<(), E>,
) -> AppConfig {
    if check_config_file(path).is_err() {
        return AppConfig::default();
    }
    let Ok(content) = std::fs::read_to_string(path) else {
        return AppConfig::default();
    };
    let Ok(mut value) = serde_json::from_str::<Value>(&content) else {
        return AppConfig::default();
    };
    let Ok(config) = serde_json::from_value::<AppConfig>(value.clone()) else {
        return AppConfig::default();
    };
    if !normalize_retired_stt(&mut value) {
        return config;
    }
    let config = serde_json::from_value(value.clone()).expect("normalized selection is valid");
    if let Err(error) =
        persist(&serde_json::to_string_pretty(&value).expect("JSON config serializes"))
    {
        tracing::warn!(%error, "retired_stt_selection_migration_persist_failed");
    }
    config
}

pub struct ConfigState {
    path: PathBuf,
    inner: Arc<ConfigInner>,
}

struct ConfigInner {
    config: RwLock<AppConfig>,
    write_lock: Mutex<()>,
    changes: tokio::sync::watch::Sender<u64>,
    processing_changes: tokio::sync::watch::Sender<u64>,
}

impl ConfigState {
    pub fn load_or_default(vault_base: &Path) -> Self {
        let path = hypr_storage::vault::compute_config_path(vault_base);
        let config = load_config(&path, |content| {
            hypr_storage::fs::atomic_write(&path, content)
        });

        Self::from_config(path, config)
    }

    fn from_config(path: PathBuf, config: AppConfig) -> Self {
        Self {
            path,
            inner: Arc::new(ConfigInner {
                config: RwLock::new(config),
                write_lock: Mutex::new(()),
                changes: tokio::sync::watch::channel(0).0,
                processing_changes: tokio::sync::watch::channel(0).0,
            }),
        }
    }

    pub fn snapshot(&self) -> AppConfig {
        self.inner.config.read().unwrap().clone()
    }

    pub fn subscribe(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.changes.subscribe()
    }

    pub fn subscribe_processing(&self) -> tokio::sync::watch::Receiver<u64> {
        self.inner.processing_changes.subscribe()
    }

    pub async fn read_current(&self) -> Result<AppConfig, ConfigError> {
        self.edit(|config| Ok((config.clone(), false))).await
    }

    pub(crate) async fn edit<T: Send + 'static>(
        &self,
        edit: impl FnOnce(&mut AppConfig) -> Result<(T, bool), ConfigError> + Send + 'static,
    ) -> Result<T, ConfigError> {
        let path = self.path.clone();
        let inner = self.inner.clone();
        // The task owns the lock, disk commit, cache update and notification even
        // when a webview or HTTP request disappears while persistence is running.
        tokio::task::spawn_blocking(move || inner.edit(&path, edit))
            .await
            .map_err(|error| std::io::Error::other(error.to_string()))?
    }

    pub async fn set_values(
        &self,
        values: HashMap<String, Value>,
    ) -> Result<AppConfig, ConfigError> {
        self.edit(move |current| {
            let Value::Object(mut map) = serde_json::to_value(&*current)? else {
                unreachable!("AppConfig always serializes to an object");
            };
            for (key, value) in values {
                map.insert(key, value);
            }
            let mut value = Value::Object(map);
            normalize_retired_stt(&mut value);
            *current = serde_json::from_value(value)?;
            Ok((current.clone(), true))
        })
        .await
    }

    pub fn reset(&self) -> Result<AppConfig, ConfigError> {
        let _guard = self.inner.write_lock.lock().unwrap();
        self.inner.persist(&self.path, AppConfig::default())
    }
}

impl ConfigInner {
    fn edit<T>(
        &self,
        path: &Path,
        edit: impl FnOnce(&mut AppConfig) -> Result<(T, bool), ConfigError>,
    ) -> Result<T, ConfigError> {
        let _guard = self.write_lock.lock().unwrap();
        let mut current = read_config(path)?;
        let (result, changed) = edit(&mut current)?;
        if changed {
            self.persist(path, current)?;
        } else {
            self.publish(current);
        }
        Ok(result)
    }

    fn publish(&self, next: AppConfig) {
        let mut config = self.config.write().unwrap();
        if *config != next {
            let processing_changed = crate::processing_settings::projection(&config)
                != crate::processing_settings::projection(&next);
            *config = next;
            drop(config);
            self.changes
                .send_modify(|version| *version = version.wrapping_add(1));
            if processing_changed {
                self.processing_changes
                    .send_modify(|version| *version = version.wrapping_add(1));
            }
        }
    }

    fn persist(&self, path: &Path, next: AppConfig) -> Result<AppConfig, ConfigError> {
        check_config_file(path)?;
        hypr_storage::fs::atomic_write(path, &serde_json::to_string_pretty(&next)?)?;
        self.publish(next.clone());
        #[cfg(unix)]
        std::fs::File::open(path.parent().expect("config has a parent"))?.sync_all()?;
        Ok(next)
    }
}

fn check_config_file(path: &Path) -> std::io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "Config must be a regular file",
        )),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn read_config(path: &Path) -> Result<AppConfig, ConfigError> {
    check_config_file(path)?;
    match std::fs::read_to_string(path) {
        Ok(content) => {
            let mut value: Value = serde_json::from_str(&content)?;
            // Validate the original fields before applying a compatibility migration.
            serde_json::from_value::<AppConfig>(value.clone())?;
            normalize_retired_stt(&mut value);
            Ok(serde_json::from_value(value)?)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(AppConfig::default()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn meeting_languages_preserve_legacy_fallback_and_independent_selection() {
        let legacy: AppConfig =
            serde_json::from_value(json!({ "ai_language": "en", "spoken_languages": ["nl"] }))
                .unwrap();
        assert_eq!(legacy.meeting_languages, None);
        assert!(
            serde_json::to_value(&legacy)
                .unwrap()
                .get("meeting_languages")
                .is_none()
        );
        let independent: AppConfig = serde_json::from_value(
            json!({ "ai_language": "en", "spoken_languages": ["fr"], "meeting_languages": ["nl"] }),
        )
        .unwrap();
        assert_eq!(independent.meeting_languages, Some(vec!["nl".to_string()]));
        let restored: AppConfig =
            serde_json::from_value(serde_json::to_value(independent).unwrap()).unwrap();
        assert_eq!(restored.meeting_languages, Some(vec!["nl".to_string()]));
    }

    fn values(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn missing_file_yields_defaults() {
        let temp = tempdir().unwrap();

        let state = ConfigState::load_or_default(temp.path());

        assert_eq!(state.snapshot(), AppConfig::default());
        assert_eq!(state.snapshot().transcription_timing, "live");
    }

    #[tokio::test]
    async fn confident_tags_default_on_and_explicit_opt_out_survives_restart() {
        let temp = tempdir().unwrap();
        let state = ConfigState::load_or_default(temp.path());
        assert!(state.snapshot().auto_apply_high_confidence_tags);

        std::fs::write(temp.path().join("config.json"), r#"{"theme":"dark"}"#).unwrap();
        let state = ConfigState::load_or_default(temp.path());
        assert!(state.snapshot().auto_apply_high_confidence_tags);
        state
            .set_values(values(&[("auto_apply_high_confidence_tags", json!(false))]))
            .await
            .unwrap();
        let reloaded = ConfigState::load_or_default(temp.path());
        assert!(!reloaded.snapshot().auto_apply_high_confidence_tags);
        reloaded
            .set_values(values(&[("theme", json!("light"))]))
            .await
            .unwrap();
        assert!(
            !ConfigState::load_or_default(temp.path())
                .snapshot()
                .auto_apply_high_confidence_tags
        );
    }

    #[tokio::test]
    async fn set_values_round_trips_through_disk() {
        let temp = tempdir().unwrap();
        let state = ConfigState::load_or_default(temp.path());

        state
            .set_values(values(&[
                ("theme", json!("dark")),
                ("transcription_timing", json!("batch")),
                ("spoken_languages", json!(["en", "ko"])),
                ("current_llm_provider", json!("openai")),
                (
                    "ai_providers",
                    json!({"llm:openai": {"type": "llm", "base_url": "https://api.openai.com/v1"}}),
                ),
            ]))
            .await
            .unwrap();

        let reloaded = ConfigState::load_or_default(temp.path());
        assert_eq!(reloaded.snapshot(), state.snapshot());
        assert_eq!(reloaded.snapshot().theme, "dark");
        assert_eq!(reloaded.snapshot().transcription_timing, "batch");
        assert_eq!(reloaded.snapshot().spoken_languages, vec!["en", "ko"]);
        assert_eq!(
            reloaded.snapshot().current_llm_provider.as_deref(),
            Some("openai")
        );
        assert_eq!(
            reloaded.snapshot().ai_providers["llm:openai"].base_url,
            "https://api.openai.com/v1"
        );
    }

    #[tokio::test]
    async fn retired_reminder_delay_is_preserved_as_unknown_config() {
        let temp = tempdir().unwrap();
        std::fs::write(
            temp.path().join("config.json"),
            r#"{"mic_active_threshold":15,"theme":"dark"}"#,
        )
        .unwrap();
        let state = ConfigState::load_or_default(temp.path());
        assert_eq!(state.snapshot().extra["mic_active_threshold"], json!(15));
        assert_eq!(state.snapshot().theme, "dark");
        state
            .set_values(values(&[("respect_dnd", json!(true))]))
            .await
            .unwrap();
        let reloaded = ConfigState::load_or_default(temp.path());
        assert_eq!(reloaded.snapshot().extra["mic_active_threshold"], json!(15));
        assert!(reloaded.snapshot().respect_dnd);
        assert!(
            !serde_json::to_value(AppConfig::default())
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("mic_active_threshold")
        );
    }

    #[tokio::test]
    async fn summary_prompt_survives_settings_writes_and_restart() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("config.json"), r#"{"auto_summary_prompt":"Decisions in {{ language }}", "selected_template_id":"retired"}"#).unwrap();
        let state = ConfigState::load_or_default(temp.path());
        assert_eq!(
            state.snapshot().auto_summary_prompt,
            "Decisions in {{ language }}"
        );
        state
            .set_values(values(&[("theme", json!("dark"))]))
            .await
            .unwrap();
        let reloaded = ConfigState::load_or_default(temp.path());
        assert_eq!(
            reloaded.snapshot().auto_summary_prompt,
            "Decisions in {{ language }}"
        );
        reloaded
            .set_values(values(&[("auto_summary_prompt", json!(""))]))
            .await
            .unwrap();
        assert_eq!(
            ConfigState::load_or_default(temp.path())
                .snapshot()
                .auto_summary_prompt,
            ""
        );
    }

    #[tokio::test]
    async fn unknown_keys_survive_read_write_round_trip() {
        let temp = tempdir().unwrap();
        std::fs::write(
            temp.path().join("config.json"),
            r#"{"autostart": true, "future_key": {"a": 1}, "personalization_dictionary_terms": ["Loofah"], "hooks": {"version": 0, "on": {"beforeListeningStarted": [{"command": "legacy-hook"}]}}}"#,
        )
        .unwrap();

        let state = ConfigState::load_or_default(temp.path());
        state
            .set_values(values(&[("theme", json!("light"))]))
            .await
            .unwrap();

        let on_disk: Value = serde_json::from_str(
            &std::fs::read_to_string(temp.path().join("config.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(on_disk["future_key"], json!({"a": 1}));
        assert_eq!(
            on_disk["personalization_dictionary_terms"],
            json!(["Loofah"])
        );
        assert_eq!(
            on_disk["hooks"]["on"]["beforeListeningStarted"][0]["command"],
            json!("legacy-hook")
        );
        assert_eq!(on_disk["autostart"], json!(true));
        assert_eq!(on_disk["theme"], json!("light"));
    }

    #[tokio::test]
    async fn legacy_task_provider_settings_preserve_config_and_task_files() {
        let temp = tempdir().unwrap();
        let legacy = json!({
            "theme": "dark",
            "current_task_provider": "apple-reminders",
            "current_todo_provider": "apple-reminders",
            "todo_providers": {"apple-reminders": {"list_id": "legacy-list"}},
            "future_key": {"nested": [1, 2, 3]}
        });
        std::fs::write(temp.path().join("config.json"), legacy.to_string()).unwrap();
        let session = temp.path().join("sessions/session-1");
        std::fs::create_dir_all(&session).unwrap();
        let tasks = br#"{
  "tasks": [{
    "id": "task-1",
    "source_type": "session_raw_note",
    "source_id": "session-1",
    "source_order": 0,
    "text": "Keep my task",
    "status": "todo",
    "body": [],
    "created_at": "2026-09-14T00:00:00Z",
    "updated_at": "2026-09-14T00:00:00Z"
  }]
}
"#;
        let tasks_path = session.join("tasks.json");
        std::fs::write(&tasks_path, tasks).unwrap();

        let state = ConfigState::load_or_default(temp.path());
        assert_eq!(state.snapshot().theme, "dark");
        assert_eq!(std::fs::read(&tasks_path).unwrap(), tasks);
        state
            .set_values(values(&[("theme", json!("light"))]))
            .await
            .unwrap();

        let reloaded = ConfigState::load_or_default(temp.path()).snapshot();
        assert_eq!(reloaded.theme, "light");
        let config = serde_json::to_value(reloaded).unwrap();
        for key in [
            "current_task_provider",
            "current_todo_provider",
            "todo_providers",
            "future_key",
        ] {
            assert_eq!(config[key], legacy[key]);
        }
        assert_eq!(std::fs::read(tasks_path).unwrap(), tasks);
    }

    #[tokio::test]
    async fn set_values_accepts_unknown_keys() {
        let temp = tempdir().unwrap();
        let state = ConfigState::load_or_default(temp.path());

        state
            .set_values(values(&[("brand_new_key", json!("hello"))]))
            .await
            .unwrap();

        assert_eq!(
            state.snapshot().extra.get("brand_new_key"),
            Some(&json!("hello"))
        );
    }

    #[tokio::test]
    async fn partial_update_touches_only_given_keys() {
        let temp = tempdir().unwrap();
        let state = ConfigState::load_or_default(temp.path());
        state
            .set_values(values(&[("ai_language", json!("nl"))]))
            .await
            .unwrap();

        state
            .set_values(values(&[("theme", json!("dark"))]))
            .await
            .unwrap();

        let snapshot = state.snapshot();
        assert_eq!(snapshot.theme, "dark");
        assert_eq!(snapshot.ai_language, "nl");
        assert_eq!(snapshot.autostart, false);
        assert_eq!(snapshot.notification_detect, true);
    }

    #[tokio::test]
    async fn type_mismatch_fails_and_leaves_state_untouched() {
        let temp = tempdir().unwrap();
        let state = ConfigState::load_or_default(temp.path());

        let result = state
            .set_values(values(&[("autostart", json!("not-a-bool"))]))
            .await;

        assert!(result.is_err());
        assert_eq!(state.snapshot(), AppConfig::default());
        assert!(!temp.path().join("config.json").exists());
    }

    #[tokio::test]
    async fn retired_selections_migrate_idempotently_without_touching_other_settings() {
        for (provider, model) in [
            ("am", "anything"),
            ("argmax", "anything"),
            ("fmtr", "am-parakeet-v2"),
            ("fmtr", "am-parakeet-v3"),
            ("fmtr", "am-whisper-large-v3"),
            ("fmtr", "soniqo-qwen3-small"),
            ("fmtr", "soniqo-qwen3-large"),
            ("fmtr", "aufklarer/Qwen3-ASR-0.6B-MLX-4bit"),
            ("fmtr", "aufklarer/Qwen3-ASR-1.7B-MLX-8bit"),
        ] {
            let temp = tempdir().unwrap();
            let path = temp.path().join("config.json");
            let mut expected = json!({
                "current_stt_provider": provider, "current_stt_model": model,
                "current_llm_provider": "legacy", "current_llm_model": "HyprLLM",
                "ai_providers": {"llm:legacy": {"type": "llm", "base_url": "", "future": 42}},
                "spoken_languages": ["fr", "nl"], "meeting_languages": ["de"],
                "transcription_timing": "live", "future": {"nested": [1, 2]}
            });
            std::fs::write(&path, expected.to_string()).unwrap();
            expected["current_stt_provider"] = json!("fmtr");
            expected["current_stt_model"] = json!("soniqo-parakeet-batch");
            let state = ConfigState::load_or_default(temp.path());
            assert_eq!(
                state.snapshot().current_stt_model.as_deref(),
                Some("soniqo-parakeet-batch")
            );
            let content = std::fs::read_to_string(&path).unwrap();
            assert_eq!(serde_json::from_str::<Value>(&content).unwrap(), expected);
            assert_eq!(
                ConfigState::load_or_default(temp.path()).snapshot(),
                state.snapshot()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), content);
            assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
            state
                .set_values(values(&[
                    ("current_stt_provider", json!(provider)),
                    ("current_stt_model", json!(model)),
                    ("theme", json!("dark")),
                ]))
                .await
                .unwrap();
            assert_eq!(
                state.snapshot().current_stt_model.as_deref(),
                Some("soniqo-parakeet-batch")
            );
        }
    }

    #[tokio::test]
    async fn failed_migration_persistence_keeps_normalized_selection() {
        let temp = tempdir().unwrap();
        let path = temp.path().join("config.json");
        let original = r#"{"current_stt_provider":"am","current_stt_model":"am-parakeet-v3"}"#;
        std::fs::write(&path, original).unwrap();
        let config = load_config(&path, |_| {
            Err(std::io::Error::other("injected write failure"))
        });
        assert_eq!(
            config.current_stt_model.as_deref(),
            Some("soniqo-parakeet-batch")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        let state = ConfigState::from_config(path.clone(), config);
        state
            .set_values(values(&[("theme", json!("dark"))]))
            .await
            .unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["current_stt_model"], "soniqo-parakeet-batch");
        assert_eq!(saved["theme"], "dark");
    }

    #[test]
    fn supported_selections_are_not_rewritten() {
        for model in [
            "soniqo-parakeet-streaming",
            "soniqo-parakeet-batch",
            "soniqo-omnilingual",
            "QuantizedTiny",
            "whisper-large-v3",
        ] {
            let temp = tempdir().unwrap();
            let path = temp.path().join("config.json");
            let original =
                json!({"current_stt_provider": "fmtr", "current_stt_model": model}).to_string();
            std::fs::write(&path, &original).unwrap();
            assert_eq!(
                ConfigState::load_or_default(temp.path())
                    .snapshot()
                    .current_stt_model
                    .as_deref(),
                Some(model)
            );
            assert_eq!(std::fs::read_to_string(path).unwrap(), original);
        }
    }

    #[tokio::test]
    async fn malformed_config_is_never_overwritten_by_migration_or_partial_save() {
        for original in [
            "{not json",
            r#"{"current_stt_provider":"am","spoken_languages":42}"#,
        ] {
            let temp = tempdir().unwrap();
            let path = temp.path().join("config.json");
            std::fs::write(&path, original).unwrap();
            let state = ConfigState::load_or_default(temp.path());
            assert!(
                state
                    .set_values(values(&[("theme", json!("dark"))]))
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read_to_string(path).unwrap(), original);
        }
    }

    #[test]
    fn corrupt_file_yields_defaults() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("config.json"), "{not json").unwrap();

        let state = ConfigState::load_or_default(temp.path());

        assert_eq!(state.snapshot(), AppConfig::default());
    }

    #[tokio::test]
    async fn partial_save_merges_the_current_file_instead_of_the_startup_cache() {
        let temp = tempdir().unwrap();
        let state = ConfigState::load_or_default(temp.path());
        std::fs::write(
            temp.path().join("config.json"),
            json!({"ai_language":"nl","future":{"external":true}}).to_string(),
        )
        .unwrap();
        state
            .set_values(values(&[("theme", json!("dark"))]))
            .await
            .unwrap();
        let reloaded = ConfigState::load_or_default(temp.path()).snapshot();
        assert_eq!(reloaded.ai_language, "nl");
        assert_eq!(reloaded.extra["future"], json!({"external":true}));
        assert_eq!(reloaded.theme, "dark");
        assert_eq!(state.snapshot(), reloaded);
    }

    #[tokio::test]
    async fn processing_generation_retains_coalesced_aba_changes_and_ignores_device_fields() {
        let temp = tempdir().unwrap();
        let state = ConfigState::load_or_default(temp.path());
        let mut changes = state.subscribe_processing();
        let initial = crate::processing_settings::snapshot(&state.snapshot());
        state
            .set_values(values(&[("theme", json!("dark"))]))
            .await
            .unwrap();
        assert!(!changes.has_changed().unwrap());
        state
            .set_values(values(&[("ai_language", json!("nl"))]))
            .await
            .unwrap();
        let intermediate =
            crate::processing_settings::snapshot(&state.read_current().await.unwrap());
        assert_ne!(intermediate, initial);
        state
            .set_values(values(&[("ai_language", json!("en"))]))
            .await
            .unwrap();
        assert_eq!(
            crate::processing_settings::snapshot(&state.snapshot()),
            initial
        );
        // A client may cache the intermediate read before the forwarding task runs.
        assert!(changes.has_changed().unwrap());
        assert_eq!(*changes.borrow_and_update(), 2);
        state
            .set_values(values(&[("theme", json!("light"))]))
            .await
            .unwrap();
        assert!(!changes.has_changed().unwrap());
    }

    #[tokio::test]
    async fn cancellation_keeps_the_lock_through_commit_cache_update_and_notification() {
        let temp = tempdir().unwrap();
        let state = Arc::new(ConfigState::load_or_default(temp.path()));
        let mut changes = state.subscribe();
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let writer = state.clone();
        let pause = barrier.clone();
        let task = tokio::spawn(async move {
            writer
                .edit(move |config| {
                    config.ai_language = "nl".into();
                    entered.send(()).unwrap();
                    pause.wait();
                    Ok(((), true))
                })
                .await
        });
        waiting.await.unwrap();
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let writer = state.clone();
        let following =
            tokio::spawn(
                async move { writer.set_values(values(&[("theme", json!("dark"))])).await },
            );
        tokio::task::spawn_blocking(move || barrier.wait())
            .await
            .unwrap();
        following.await.unwrap().unwrap();
        changes.changed().await.unwrap();
        assert_eq!(state.snapshot().ai_language, "nl");
        assert_eq!(state.snapshot().theme, "dark");
        assert_eq!(
            ConfigState::load_or_default(temp.path()).snapshot(),
            state.snapshot()
        );
    }

    #[tokio::test]
    async fn reset_and_async_writes_share_one_persistence_lock() {
        let temp = tempdir().unwrap();
        let state = Arc::new(ConfigState::load_or_default(temp.path()));
        let (entered, waiting) = tokio::sync::oneshot::channel();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let writer = state.clone();
        let pause = barrier.clone();
        let write = tokio::spawn(async move {
            writer
                .edit(move |config| {
                    entered.send(()).unwrap();
                    pause.wait();
                    config.ai_language = "nl".into();
                    Ok(((), true))
                })
                .await
        });
        waiting.await.unwrap();
        let resetting = state.clone();
        let reset = tokio::task::spawn_blocking(move || resetting.reset());
        tokio::task::spawn_blocking(move || barrier.wait())
            .await
            .unwrap();
        write.await.unwrap().unwrap();
        reset.await.unwrap().unwrap();
        assert_eq!(state.snapshot(), AppConfig::default());
        assert_eq!(
            ConfigState::load_or_default(temp.path()).snapshot(),
            AppConfig::default()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_config_is_never_read_migrated_or_replaced() {
        let temp = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let original = r#"{"current_stt_provider":"am","ai_language":"private"}"#;
        let target = outside.path().join("other-vault-config.json");
        std::fs::write(&target, original).unwrap();
        let link = temp.path().join("config.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let state = ConfigState::load_or_default(temp.path());
        assert_eq!(state.snapshot(), AppConfig::default());
        assert!(state.read_current().await.is_err());
        assert!(
            state
                .set_values(values(&[("theme", json!("dark"))]))
                .await
                .is_err()
        );
        assert!(state.reset().is_err());
        assert!(link.is_symlink());
        assert_eq!(std::fs::read_to_string(target).unwrap(), original);
    }
}
