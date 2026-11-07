use std::sync::{Arc, Weak};

use hypr_loofah_protocol::ProcessingJobFailure;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use super::{
    GeneratedDocument, JobFuture, JobGenerator, JobWork,
    generation::{SummaryGenerator, TextModel},
    source::{SummaryIdentity, VaultSummarySource},
};
use crate::{VaultContext, settings::AppConfig};

#[derive(Clone, PartialEq, Eq)]
pub struct LlmSelection {
    pub provider_id: String,
    pub model_id: String,
    pub base_url: String,
}

impl LlmSelection {
    pub fn from_config(config: &AppConfig) -> Result<Self, ProcessingJobFailure> {
        let provider_id = config.current_llm_provider.as_deref().unwrap_or("").trim();
        let model_id = config.current_llm_model.as_deref().unwrap_or("").trim();
        if provider_id.is_empty()
            || model_id.is_empty()
            || provider_id.len() > 128
            || model_id.len() > 512
            || provider_id
                .chars()
                .chain(model_id.chars())
                .any(char::is_control)
        {
            return Err(ProcessingJobFailure::ProviderUnavailable);
        }
        let base_url = config
            .ai_providers
            .get(&format!("llm:{provider_id}"))
            .filter(|entry| entry.kind == "llm")
            .map(|entry| entry.base_url.trim().to_owned())
            .unwrap_or_default();
        if base_url.len() > 8192 || base_url.chars().any(char::is_control) {
            return Err(ProcessingJobFailure::ProviderUnavailable);
        }
        Ok(Self {
            provider_id: provider_id.into(),
            model_id: model_id.into(),
            base_url,
        })
    }
}

pub struct SummaryModel {
    pub model: Arc<dyn TextModel>,
    pub include_images: bool,
}

/// Hosts enforce endpoint/model policy and resolve credentials outside canonical vault settings.
pub trait SummaryModels: Send + Sync + 'static {
    fn resolve(
        &self,
        selection: LlmSelection,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<SummaryModel, ProcessingJobFailure>>;
}

#[derive(Clone)]
pub struct ProcessingAdmission {
    running: Arc<Semaphore>,
    total: Arc<Semaphore>,
}

impl ProcessingAdmission {
    pub fn new(running: usize, waiting: usize) -> Result<Self, ProcessingJobFailure> {
        if running == 0 || running > 16 || waiting > 1024 {
            return Err(ProcessingJobFailure::ResourceLimit);
        }
        Ok(Self {
            running: Arc::new(Semaphore::new(running)),
            total: Arc::new(Semaphore::new(running + waiting)),
        })
    }
}

pub struct ConfiguredSummaryGenerator {
    context: Weak<VaultContext>,
    identity: SummaryIdentity,
    models: Arc<dyn SummaryModels>,
    admission: ProcessingAdmission,
    allow_custom_templates: bool,
}

impl ConfiguredSummaryGenerator {
    pub fn new(
        context: &Arc<VaultContext>,
        identity: SummaryIdentity,
        models: Arc<dyn SummaryModels>,
        admission: ProcessingAdmission,
        allow_custom_templates: bool,
    ) -> Self {
        Self {
            context: Arc::downgrade(context),
            identity,
            models,
            admission,
            allow_custom_templates,
        }
    }
}

impl JobGenerator for ConfiguredSummaryGenerator {
    fn generate(
        &self,
        work: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>> {
        let (context, identity, models, admission) = (
            self.context.clone(),
            self.identity,
            self.models.clone(),
            self.admission.clone(),
        );
        let allow_custom_templates = self.allow_custom_templates;
        Box::pin(async move {
            let stopping = cancel.child_token();
            let _stop_on_drop = stopping.clone().drop_guard();
            // A dropped caller cancels admission, but owned preparation/model work keeps its
            // capacity and vault lease until the actual operation has drained.
            tokio::spawn(async move {
                if stopping.is_cancelled() { return Err(ProcessingJobFailure::Interrupted); }
                let _total = admission.total.try_acquire_owned().map_err(|_| ProcessingJobFailure::ResourceLimit)?;
                let _running = tokio::select! {
                    biased;
                    _ = stopping.cancelled() => return Err(ProcessingJobFailure::Interrupted),
                    permit = admission.running.acquire_owned() => permit.map_err(|_| ProcessingJobFailure::Interrupted)?,
                };
                let context = context.upgrade().ok_or(ProcessingJobFailure::SourceUnavailable)?;
                context.store.ensure_ready().map_err(|_| ProcessingJobFailure::SourceUnavailable)?;
                let config = context.settings.snapshot();
                if !allow_custom_templates && !config.auto_summary_prompt.trim().is_empty() {
                    return Err(ProcessingJobFailure::AccessDenied);
                }
                let selection = LlmSelection::from_config(&config)?;
                let resolved = models.resolve(selection, stopping.child_token()).await?;
                if stopping.is_cancelled() { return Err(ProcessingJobFailure::Interrupted); }
                let source = Arc::new(VaultSummarySource::new(&context,identity,resolved.include_images).with_custom_templates(allow_custom_templates));
                SummaryGenerator::new(source,resolved.model).generate(work,stopping).await
            }).await.map_err(|_| ProcessingJobFailure::Interrupted)?
        })
    }
}

#[cfg(test)]
mod tests;
