use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use hypr_loofah_core::{
    VaultRuntime,
    jobs::{
        JobAuthorizer, JobFuture, JobWork,
        configured::{
            ConfiguredSummaryGenerator, LlmSelection, ProcessingAdmission, SummaryModel,
            SummaryModels,
        },
        providers::{HttpModel, HttpModelConfig, connections::provider_definition},
        source::SummaryIdentity,
    },
};
use hypr_loofah_protocol::{ErrorCode, ProcessingJobFailure};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

use crate::{
    catalog::Catalog,
    error::{ApiError, Result},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessingConfig {
    pub providers: Vec<ProviderConfig>,
    #[serde(default = "default_running")]
    pub concurrent_jobs: usize,
    #[serde(default = "default_waiting")]
    pub waiting_jobs: usize,
    #[serde(default)]
    pub allow_custom_templates: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub id: String,
    pub base_url: Option<String>,
    pub models: Vec<String>,
    pub api_key_env: Option<String>,
    #[serde(default)]
    pub allow_insecure_http: bool,
    #[serde(default = "default_images")]
    pub include_images: bool,
}
fn default_running() -> usize {
    2
}
fn default_waiting() -> usize {
    32
}
fn default_images() -> bool {
    true
}

impl ProcessingConfig {
    pub fn validate(&self) -> std::result::Result<(), String> {
        if ProcessingAdmission::new(self.concurrent_jobs, self.waiting_jobs).is_err()
            || self.providers.is_empty()
            || self.providers.len() > 16
        {
            return Err(
                "Configure 1-16 processing providers, 1-16 workers and at most 1024 waiting jobs"
                    .into(),
            );
        }
        let mut ids = BTreeSet::new();
        for provider in &self.providers {
            let definition =
                provider_definition(&provider.id).ok_or("Unsupported processing provider")?;
            if !ids.insert(&provider.id)
                || provider.models.is_empty()
                || provider.models.len() > 256
                || provider.models.iter().any(|model| {
                    model.trim().is_empty()
                        || model.trim() != model
                        || model.len() > 512
                        || model.chars().any(char::is_control)
                })
                || provider.models.iter().collect::<BTreeSet<_>>().len() != provider.models.len()
                || provider.api_key_env.as_ref().is_some_and(|name| {
                    name.is_empty()
                        || name.len() > 128
                        || !name.bytes().enumerate().all(|(index, c)| {
                            c == b'_' || c.is_ascii_uppercase() || index > 0 && c.is_ascii_digit()
                        })
                })
                || definition.requires_key && provider.api_key_env.is_none()
            {
                return Err(
                    "Invalid processing provider, model allowlist or credential reference".into(),
                );
            }
            let base_url = provider
                .base_url
                .as_deref()
                .or(definition.base_url)
                .ok_or("Configure the processing provider endpoint")?;
            if base_url.len() > 8192 {
                return Err("Invalid processing provider endpoint".into());
            }
            for model in &provider.models {
                let probe = HttpModelConfig {
                    provider: definition.wire,
                    base_url: base_url.into(),
                    model: model.clone(),
                    api_key: definition.requires_key.then(|| "validation-only".into()),
                    allow_insecure_http: provider.allow_insecure_http,
                };
                HttpModel::validate_config(&probe)
                    .map_err(|_| "Invalid processing provider endpoint or model")?;
            }
        }
        Ok(())
    }
}

struct Binding {
    config: ProviderConfig,
    base_url: String,
    api_key: Option<String>,
}

pub(crate) struct ProcessingHost {
    models: Arc<OperatorModels>,
    admission: ProcessingAdmission,
    allow_custom_templates: bool,
}
struct OperatorModels(BTreeMap<String, Binding>);

impl ProcessingHost {
    pub fn load(config: &ProcessingConfig) -> Result<Arc<Self>> {
        Self::load_with(config, |name| std::env::var(name).ok())
    }

    fn load_with(
        config: &ProcessingConfig,
        secret: impl Fn(&str) -> Option<String>,
    ) -> Result<Arc<Self>> {
        config
            .validate()
            .map_err(|message| ApiError::new(ErrorCode::Validation, &message))?;
        let mut bindings = BTreeMap::new();
        for provider in &config.providers {
            let definition = provider_definition(&provider.id).unwrap();
            let api_key = match &provider.api_key_env {
                Some(name) => Some(
                    secret(name)
                        .map(|key| key.trim().to_owned())
                        .filter(|key| !key.is_empty() && key.len() <= 16 * 1024)
                        .ok_or_else(|| {
                            ApiError::new(
                                ErrorCode::Validation,
                                "Configured provider credential is unavailable",
                            )
                        })?,
                ),
                None => None,
            };
            let base_url = provider
                .base_url
                .clone()
                .or_else(|| definition.base_url.map(str::to_owned))
                .unwrap();
            HttpModel::validate_config(&HttpModelConfig {
                provider: definition.wire,
                base_url: base_url.clone(),
                model: provider.models[0].clone(),
                api_key: api_key.clone(),
                allow_insecure_http: provider.allow_insecure_http,
            })
            .map_err(|_| {
                ApiError::new(
                    ErrorCode::Validation,
                    "Configured provider credential is invalid",
                )
            })?;
            bindings.insert(
                provider.id.clone(),
                Binding {
                    config: provider.clone(),
                    base_url,
                    api_key,
                },
            );
        }
        Ok(Arc::new(Self {
            models: Arc::new(OperatorModels(bindings)),
            admission: ProcessingAdmission::new(config.concurrent_jobs, config.waiting_jobs)
                .map_err(|_| ApiError::new(ErrorCode::Validation, "Invalid processing capacity"))?,
            allow_custom_templates: config.allow_custom_templates,
        }))
    }

    pub async fn attach(&self, runtime: &mut VaultRuntime, catalog: Catalog) -> Result<()> {
        runtime
            .context
            .configure_processing_policy(hypr_loofah_protocol::ProcessingPolicy {
                providers: self
                    .models
                    .0
                    .values()
                    .map(|binding| hypr_loofah_protocol::ProcessingProviderPolicy {
                        id: binding.config.id.clone(),
                        models: binding.config.models.clone(),
                        base_url: binding.base_url.clone(),
                        allow_custom_model: false,
                        allow_custom_endpoint: false,
                        requires_key: provider_definition(&binding.config.id)
                            .expect("validated provider")
                            .requires_key,
                    })
                    .collect(),
                allow_custom_templates: self.allow_custom_templates,
            })?;
        let generator = Arc::new(ConfiguredSummaryGenerator::new(
            &runtime.context,
            SummaryIdentity::JobMember,
            self.models.clone(),
            self.admission.clone(),
            self.allow_custom_templates,
        ));
        let authorizer = Arc::new(MemberAuthorizer {
            catalog,
            vault_id: runtime.context.events.vault_id(),
        });
        runtime
            .enable_processing(generator, authorizer)
            .await
            .map_err(ApiError::from)
    }
}

impl SummaryModels for OperatorModels {
    fn resolve(
        &self,
        selection: LlmSelection,
        cancel: CancellationToken,
    ) -> JobFuture<'_, std::result::Result<SummaryModel, ProcessingJobFailure>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            let binding = self
                .0
                .get(&selection.provider_id)
                .ok_or(ProcessingJobFailure::ProviderUnavailable)?;
            if !binding.config.models.contains(&selection.model_id) {
                return Err(ProcessingJobFailure::ProviderUnavailable);
            }
            if !selection.base_url.is_empty()
                && !hypr_loofah_core::processing_settings::same_endpoint(
                    &selection.base_url,
                    &binding.base_url,
                )
            {
                return Err(ProcessingJobFailure::AccessDenied);
            }
            let definition = provider_definition(&binding.config.id)
                .ok_or(ProcessingJobFailure::ProviderUnavailable)?;
            let model = HttpModel::new(HttpModelConfig {
                provider: definition.wire,
                base_url: binding.base_url.clone(),
                model: selection.model_id,
                api_key: binding.api_key.clone(),
                allow_insecure_http: binding.config.allow_insecure_http,
            })?;
            Ok(SummaryModel {
                model: Arc::new(model),
                include_images: binding.config.include_images,
            })
        })
    }
}

struct MemberAuthorizer {
    catalog: Catalog,
    vault_id: String,
}
impl JobAuthorizer for MemberAuthorizer {
    fn authorize<'a>(
        &'a self,
        work: &'a JobWork,
    ) -> JobFuture<'a, std::result::Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async move {
            self.catalog
                .access_member(&work.principal, &self.vault_id, true)
                .await
                .map(|grant| Box::new(grant) as Box<dyn Send>)
                .map_err(|error| {
                    if error.0.code == ErrorCode::Unavailable {
                        ProcessingJobFailure::Interrupted
                    } else {
                        ProcessingJobFailure::AccessDenied
                    }
                })
        })
    }
}

#[cfg(test)]
mod tests;
