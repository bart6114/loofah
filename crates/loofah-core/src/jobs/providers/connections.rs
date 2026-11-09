use std::sync::Arc;

mod image_capabilities;

use hypr_loofah_protocol::ProcessingJobFailure;
use tokio_util::sync::CancellationToken;

use super::{HttpModel, HttpModelConfig, HttpProvider};
use crate::jobs::{
    JobFuture,
    configured::{LlmSelection, SummaryModel, SummaryModels},
};

pub struct HttpProviderDefinition {
    pub wire: HttpProvider,
    pub base_url: Option<&'static str>,
    pub requires_key: bool,
}

pub fn provider_definition(id: &str) -> Option<HttpProviderDefinition> {
    let (wire, base_url, requires_key) = match id {
        "openai" => (
            HttpProvider::OpenAi,
            Some("https://api.openai.com/v1"),
            true,
        ),
        "anthropic" => (
            HttpProvider::Anthropic,
            Some("https://api.anthropic.com/v1"),
            true,
        ),
        "google_generative_ai" => (
            HttpProvider::Google,
            Some("https://generativelanguage.googleapis.com/v1beta"),
            true,
        ),
        "openrouter" => (
            HttpProvider::ChatCompletions,
            Some("https://openrouter.ai/api/v1"),
            true,
        ),
        "mistral" => (
            HttpProvider::ChatCompletions,
            Some("https://api.mistral.ai/v1"),
            true,
        ),
        "ollama" => (
            HttpProvider::Ollama,
            Some("http://127.0.0.1:11434/v1"),
            false,
        ),
        "lmstudio" => (
            HttpProvider::ChatCompletions,
            Some("http://127.0.0.1:1234/v1"),
            false,
        ),
        "azure_openai" => (HttpProvider::AzureOpenAi, None, true),
        "azure_ai" => (HttpProvider::AzureAi, None, true),
        "cloudflare_workers_ai" | "custom" => (HttpProvider::ChatCompletions, None, true),
        _ => return None,
    };
    Some(HttpProviderDefinition {
        wire,
        base_url,
        requires_key,
    })
}

pub trait ProviderSecrets: Send + Sync + 'static {
    fn read(
        &self,
        provider_id: String,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<Option<String>, ProcessingJobFailure>>;
}

pub struct LocalHttpModels {
    secrets: Arc<dyn ProviderSecrets>,
}

impl LocalHttpModels {
    pub fn new(secrets: Arc<dyn ProviderSecrets>) -> Self {
        Self { secrets }
    }
}

impl SummaryModels for LocalHttpModels {
    fn resolve(
        &self,
        selection: LlmSelection,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<SummaryModel, ProcessingJobFailure>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            let definition = provider_definition(&selection.provider_id)
                .ok_or(ProcessingJobFailure::ProviderUnavailable)?;
            let include_images =
                image_capabilities::supports_images(&selection.provider_id, &selection.model_id);
            let base_url = if selection.base_url.is_empty() {
                definition.base_url.unwrap_or_default()
            } else {
                &selection.base_url
            };
            if base_url.is_empty() {
                return Err(ProcessingJobFailure::ProviderUnavailable);
            }
            let api_key = if matches!(definition.wire, HttpProvider::Ollama) {
                None
            } else {
                self.secrets
                    .read(selection.provider_id, cancel.child_token())
                    .await?
                    .map(|key| key.trim().to_owned())
                    .filter(|key| !key.is_empty())
            };
            if cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            if api_key.as_ref().is_some_and(|key| key.len() > 16 * 1024)
                || definition.requires_key && api_key.is_none()
            {
                return Err(ProcessingJobFailure::ProviderUnavailable);
            }
            let model = HttpModel::new(HttpModelConfig {
                provider: definition.wire,
                base_url: base_url.into(),
                model: selection.model_id,
                api_key,
                allow_insecure_http: true,
            })?;
            Ok(SummaryModel {
                model: Arc::new(model),
                include_images,
            })
        })
    }
}

#[cfg(test)]
mod tests;
