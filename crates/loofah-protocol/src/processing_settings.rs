use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SummaryModelSelection {
    pub provider_id: String,
    pub model_id: String,
    pub base_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct ProcessingSettings {
    pub summary_model: Option<SummaryModelSelection>,
    pub ai_language: String,
    pub summary_prompt: String,
    pub auto_apply_high_confidence_tags: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProcessingSettingsSnapshot {
    pub settings: ProcessingSettings,
    pub revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SaveProcessingSettingsRequest {
    pub settings: ProcessingSettings,
    pub expected_revision: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "snapshot", rename_all = "snake_case")]
pub enum ProcessingSettingsSaveOutcome {
    Saved(ProcessingSettingsSnapshot),
    Conflict(ProcessingSettingsSnapshot),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProcessingProviderPolicy {
    pub id: String,
    pub models: Vec<String>,
    pub base_url: String,
    pub allow_custom_model: bool,
    pub allow_custom_endpoint: bool,
    pub requires_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProcessingPolicy {
    pub providers: Vec<ProcessingProviderPolicy>,
    pub allow_custom_templates: bool,
}
