use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ProviderCredentialSource {
    Vault,
    Operator,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProviderCredentialStatus {
    pub provider_id: String,
    pub revision: String,
    pub source: ProviderCredentialSource,
    pub endpoint_matches: bool,
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SaveProviderCredentialRequest {
    pub expected_revision: String,
    #[serde(deserialize_with = "required_optional_key")]
    pub api_key: Option<String>,
}

impl std::fmt::Debug for SaveProviderCredentialRequest {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SaveProviderCredentialRequest")
            .field("expected_revision", &self.expected_revision)
            .field("api_key", &"[redacted]")
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", content = "status", rename_all = "snake_case")]
pub enum ProviderCredentialSaveOutcome {
    Saved(ProviderCredentialStatus),
    Conflict(ProviderCredentialStatus),
}

fn required_optional_key<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Option::<String>::deserialize(deserializer)
}
