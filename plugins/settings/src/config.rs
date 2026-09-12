use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
// never stored here — they live in the OS keychain.
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
    pub auto_accept_related_tags: bool,
    pub notification_detect: bool,
    pub respect_dnd: bool,
    pub cloud_sync_enabled: bool,
    pub ai_language: String,
    pub spoken_languages: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub meeting_languages: Option<Vec<String>>,
    pub personalization_dictionary_terms: Vec<String>,
    pub custom_summary_instructions: String,
    pub custom_summary_instructions_token_aware: bool,
    pub auto_summary_prompt: String,
    pub ignored_platforms: Vec<String>,
    pub included_platforms: Vec<String>,
    pub mic_active_threshold: f64,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hooks: Option<Value>,
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
            auto_accept_related_tags: false,
            notification_detect: true,
            respect_dnd: false,
            cloud_sync_enabled: true,
            ai_language: "en".to_string(),
            spoken_languages: Vec::new(),
            meeting_languages: None,
            personalization_dictionary_terms: Vec::new(),
            custom_summary_instructions: String::new(),
            custom_summary_instructions_token_aware: false,
            auto_summary_prompt: String::new(),
            ignored_platforms: Vec::new(),
            included_platforms: Vec::new(),
            mic_active_threshold: 5.0,
            current_llm_provider: None,
            current_llm_model: None,
            current_stt_provider: None,
            current_stt_model: None,
            transcription_timing: "live".to_string(),
            timezone: None,
            ai_providers: HashMap::new(),
            hooks: None,
            extra: serde_json::Map::new(),
        }
    }
}

pub struct ConfigState {
    path: PathBuf,
    config: std::sync::RwLock<AppConfig>,
    write_lock: tokio::sync::Mutex<()>,
}

impl ConfigState {
    pub fn load_or_default(vault_base: &Path) -> Self {
        let path = hypr_storage::vault::compute_config_path(vault_base);
        let config = match std::fs::read_to_string(&path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => AppConfig::default(),
        };

        Self {
            path,
            config: std::sync::RwLock::new(config),
            write_lock: tokio::sync::Mutex::new(()),
        }
    }

    pub fn snapshot(&self) -> AppConfig {
        self.config.read().unwrap().clone()
    }

    pub async fn set_values(&self, values: HashMap<String, Value>) -> crate::Result<AppConfig> {
        let _guard = self.write_lock.lock().await;

        let current = self.snapshot();
        let Value::Object(mut map) = serde_json::to_value(&current)? else {
            unreachable!("AppConfig always serializes to an object");
        };
        for (key, value) in values {
            map.insert(key, value);
        }
        let next: AppConfig = serde_json::from_value(Value::Object(map))?;

        let content = serde_json::to_string_pretty(&next)?;
        hypr_storage::fs::atomic_write_async(&self.path, &content).await?;

        *self.config.write().unwrap() = next.clone();
        Ok(next)
    }

    pub fn reset(&self) -> crate::Result<AppConfig> {
        let next = AppConfig::default();
        let content = serde_json::to_string_pretty(&next)?;

        let mut guard = self.config.write().unwrap();
        hypr_storage::fs::atomic_write(&self.path, &content)?;
        *guard = next.clone();
        Ok(next)
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
        assert_eq!(state.snapshot().mic_active_threshold, 5.0);
        assert_eq!(state.snapshot().transcription_timing, "live");
    }

    #[tokio::test]
    async fn set_values_round_trips_through_disk() {
        let temp = tempdir().unwrap();
        let state = ConfigState::load_or_default(temp.path());

        state
            .set_values(values(&[
                ("theme", json!("dark")),
                ("transcription_timing", json!("batch")),
                ("mic_active_threshold", json!(30)),
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
        assert_eq!(reloaded.snapshot().mic_active_threshold, 30.0);
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
            r#"{"autostart": true, "future_key": {"a": 1}}"#,
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
        assert_eq!(on_disk["autostart"], json!(true));
        assert_eq!(on_disk["theme"], json!("light"));
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

    #[test]
    fn corrupt_file_yields_defaults() {
        let temp = tempdir().unwrap();
        std::fs::write(temp.path().join("config.json"), "{not json").unwrap();

        let state = ConfigState::load_or_default(temp.path());

        assert_eq!(state.snapshot(), AppConfig::default());
    }
}
