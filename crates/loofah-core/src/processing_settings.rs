use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, MutationOutcome, ProcessingPolicy, ProcessingProviderPolicy,
    ProcessingSettings, ProcessingSettingsSaveOutcome, ProcessingSettingsSnapshot,
    SaveProcessingSettingsRequest, SummaryModelSelection,
};
use sha2::{Digest, Sha256};

use crate::{
    VaultContext,
    jobs::providers::{HttpModel, HttpModelConfig, connections::provider_definition},
    settings::{AiProviderEntry, AppConfig, ConfigError},
    vault::error,
};

pub fn local_processing_policy() -> ProcessingPolicy {
    let mut providers: Vec<_> = [
        "openai",
        "anthropic",
        "google_generative_ai",
        "openrouter",
        "mistral",
        "ollama",
        "lmstudio",
        "azure_openai",
        "azure_ai",
        "cloudflare_workers_ai",
        "custom",
    ]
    .into_iter()
    .map(|id| {
        let definition = provider_definition(id).expect("known local provider");
        ProcessingProviderPolicy {
            id: id.into(),
            models: vec![],
            base_url: definition.base_url.unwrap_or_default().into(),
            allow_custom_model: true,
            allow_custom_endpoint: true,
            requires_key: definition.requires_key,
        }
    })
    .collect();
    providers.push(ProcessingProviderPolicy {
        id: "chatgpt_subscription".into(),
        models: vec![],
        base_url: String::new(),
        allow_custom_model: true,
        allow_custom_endpoint: false,
        requires_key: false,
    });
    ProcessingPolicy {
        providers,
        allow_custom_templates: true,
    }
}

impl VaultContext {
    pub fn configure_processing_policy(
        &self,
        policy: ProcessingPolicy,
    ) -> Result<(), ApplicationError> {
        if self.processing_policy.get() == Some(&policy) {
            return Ok(());
        }
        self.processing_policy.set(policy).map_err(|_| {
            error(
                ErrorCode::Conflict,
                "Processing policy is already configured",
            )
        })
    }

    pub fn processing_policy(&self) -> Result<ProcessingPolicy, ApplicationError> {
        self.store.ensure_ready().map_err(|_| unavailable())?;
        self.processing_policy
            .get()
            .cloned()
            .ok_or_else(unavailable)
    }

    pub async fn read_processing_settings(
        &self,
    ) -> Result<ProcessingSettingsSnapshot, ApplicationError> {
        self.processing_policy()?;
        let config = self
            .settings
            .read_current()
            .await
            .map_err(|cause| config_error(cause, false))?;
        Ok(snapshot(&config))
    }

    pub async fn save_processing_settings(
        &self,
        input: SaveProcessingSettingsRequest,
    ) -> Result<ProcessingSettingsSaveOutcome, ApplicationError> {
        let policy = self.processing_policy()?;
        validate_settings(&input.settings, &policy)?;
        if input.expected_revision.len() != 64
            || !input
                .expected_revision
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(error(
                ErrorCode::Validation,
                "A settings revision is required",
            ));
        }
        self.settings
            .edit(move |config| {
                let current = snapshot(config);
                if current.revision != input.expected_revision {
                    return Ok((ProcessingSettingsSaveOutcome::Conflict(current), false));
                }
                let previous = config.clone();
                config.ai_language = input.settings.ai_language;
                config.auto_summary_prompt = input.settings.summary_prompt;
                config.auto_apply_high_confidence_tags =
                    input.settings.auto_apply_high_confidence_tags;
                match input.settings.summary_model {
                    Some(selection) => {
                        let entry = config
                            .ai_providers
                            .entry(format!("llm:{}", selection.provider_id))
                            .or_insert_with(|| AiProviderEntry {
                                kind: "llm".into(),
                                base_url: String::new(),
                                extra: Default::default(),
                            });
                        entry.kind = "llm".into();
                        entry.base_url = selection.base_url;
                        config.current_llm_provider = Some(selection.provider_id);
                        config.current_llm_model = Some(selection.model_id);
                    }
                    None => {
                        config.current_llm_provider = None;
                        config.current_llm_model = None;
                    }
                }
                Ok((
                    ProcessingSettingsSaveOutcome::Saved(snapshot(config)),
                    *config != previous,
                ))
            })
            .await
            .map_err(|cause| config_error(cause, true))
    }
}

pub fn snapshot(config: &AppConfig) -> ProcessingSettingsSnapshot {
    let settings = projection(config);
    let bytes = serde_json::to_vec(&settings).expect("settings serialize");
    let revision = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    ProcessingSettingsSnapshot { settings, revision }
}

pub(crate) fn projection(config: &AppConfig) -> ProcessingSettings {
    let summary_model = config
        .current_llm_provider
        .as_ref()
        .zip(config.current_llm_model.as_ref())
        .map(|(provider_id, model_id)| SummaryModelSelection {
            provider_id: provider_id.clone(),
            model_id: model_id.clone(),
            base_url: config
                .ai_providers
                .get(&format!("llm:{provider_id}"))
                .filter(|entry| entry.kind == "llm")
                .map(|entry| entry.base_url.clone())
                .unwrap_or_default(),
        });
    ProcessingSettings {
        summary_model,
        ai_language: config.ai_language.clone(),
        summary_prompt: config.auto_summary_prompt.clone(),
        auto_apply_high_confidence_tags: config.auto_apply_high_confidence_tags,
    }
}

fn validate_settings(
    settings: &ProcessingSettings,
    policy: &ProcessingPolicy,
) -> Result<(), ApplicationError> {
    if settings.ai_language.len() > 128
        || settings.ai_language.chars().any(char::is_control)
        || settings.summary_prompt.len() > 64 * 1024
    {
        return Err(error(
            ErrorCode::Validation,
            "Invalid processing language or summary prompt",
        ));
    }
    if !policy.allow_custom_templates && !settings.summary_prompt.trim().is_empty() {
        return Err(error(
            ErrorCode::Validation,
            "Custom summary prompts are not permitted by this host",
        ));
    }
    if let Some(selection) = &settings.summary_model {
        let provider = policy
            .providers
            .iter()
            .find(|provider| provider.id == selection.provider_id)
            .ok_or_else(|| {
                error(
                    ErrorCode::Validation,
                    "This provider is not permitted by this host",
                )
            })?;
        if selection.model_id.is_empty()
            || selection.model_id.len() > 512
            || selection.model_id.trim() != selection.model_id
            || selection.model_id.chars().any(char::is_control)
            || selection.base_url.len() > 8192
            || selection.base_url.trim() != selection.base_url
            || selection.base_url.chars().any(char::is_control)
        {
            return Err(error(
                ErrorCode::Validation,
                "Invalid processing model or endpoint",
            ));
        }
        if !provider.allow_custom_model && !provider.models.contains(&selection.model_id) {
            return Err(error(
                ErrorCode::Validation,
                "This model is not permitted by this host",
            ));
        }
        if !provider.allow_custom_endpoint
            && !selection.base_url.is_empty()
            && !same_endpoint(&selection.base_url, &provider.base_url)
        {
            return Err(error(
                ErrorCode::Validation,
                "This endpoint is not permitted by this host",
            ));
        }
        if provider.allow_custom_endpoint {
            let definition = provider_definition(&provider.id).ok_or_else(unavailable)?;
            HttpModel::validate_config(&HttpModelConfig {
                provider: definition.wire,
                base_url: if selection.base_url.is_empty() {
                    provider.base_url.clone()
                } else {
                    selection.base_url.clone()
                },
                model: selection.model_id.clone(),
                api_key: provider.requires_key.then(|| "validation-only".into()),
                allow_insecure_http: true,
            })
            .map_err(|_| error(ErrorCode::Validation, "Invalid processing endpoint"))?;
        }
    }
    Ok(())
}

pub fn same_endpoint(left: &str, right: &str) -> bool {
    match (
        url::Url::parse(left.trim_end_matches('/')),
        url::Url::parse(right.trim_end_matches('/')),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn unavailable() -> ApplicationError {
    error(
        ErrorCode::Unavailable,
        "Processing settings are not available for this vault",
    )
}

fn config_error(_cause: ConfigError, writing: bool) -> ApplicationError {
    tracing::warn!(writing, "processing_settings_persistence_failed");
    let mut result = error(
        ErrorCode::Unavailable,
        "Processing settings could not be read or saved",
    );
    if writing {
        result.mutation_outcome = Some(MutationOutcome::Unknown);
    }
    result
}

#[cfg(test)]
mod tests;
