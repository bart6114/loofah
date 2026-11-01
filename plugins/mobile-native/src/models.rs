use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NativeWord {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NativeModelStatus {
    pub ready: bool,
    pub downloading: bool,
    pub model_id: String,
    pub revision: String,
    pub downloaded_bytes: u64,
    pub total_bytes: u64,
    pub phase: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct NativeImportResult {
    pub path: String,
    pub duration_seconds: f64,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, specta::Type)]
pub struct NativeVaultSelection {
    pub path: Option<String>,
}
