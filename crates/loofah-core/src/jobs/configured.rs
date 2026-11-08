use std::sync::{Arc, Weak};

use hypr_loofah_protocol::ProcessingJobFailure;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use super::{
    GeneratedDocument, JobFuture, JobGenerator, JobWork, PreparedGeneration, TitleRunner,
    generation::{SummaryGenerator, TextModel},
    source::{SummaryIdentity, VaultSummarySource, system_from_config},
    title_generation::{TitleGenerator, deterministic_title_temperature},
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
        Box::pin(async move {
            self.generate_owned(work, cancel, false, None)
                .await
                .map(|prepared| prepared.document)
        })
    }

    fn generate_with_preview(
        &self,
        work: JobWork,
        cancel: CancellationToken,
        preview: super::GenerationPreview,
        include_title: bool,
    ) -> JobFuture<'_, Result<PreparedGeneration, ProcessingJobFailure>> {
        self.generate_owned(work, cancel, include_title, Some(preview))
    }

    fn generate_prepared(
        &self,
        work: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<PreparedGeneration, ProcessingJobFailure>> {
        self.generate_owned(work, cancel, true, None)
    }
}

impl ConfiguredSummaryGenerator {
    fn generate_owned(
        &self,
        work: JobWork,
        cancel: CancellationToken,
        prepared: bool,
        preview: Option<super::GenerationPreview>,
    ) -> JobFuture<'static, Result<PreparedGeneration, ProcessingJobFailure>> {
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
                let total = admission.total.try_acquire_owned().map_err(|_| ProcessingJobFailure::ResourceLimit)?;
                let running = tokio::select! {
                    biased;
                    _ = stopping.cancelled() => return Err(ProcessingJobFailure::Interrupted),
                    permit = admission.running.acquire_owned() => permit.map_err(|_| ProcessingJobFailure::Interrupted)?,
                };
                let context = context.upgrade().ok_or(ProcessingJobFailure::SourceUnavailable)?;
                context.store.ensure_ready().map_err(|_| ProcessingJobFailure::SourceUnavailable)?;
                let config = context.settings.read_current().await.map_err(|_| ProcessingJobFailure::SourceUnavailable)?;
                if !allow_custom_templates && !config.auto_summary_prompt.trim().is_empty() {
                    return Err(ProcessingJobFailure::AccessDenied);
                }
                let selection = LlmSelection::from_config(&config)?;
                let temperature = deterministic_title_temperature(&selection.provider_id, &selection.model_id);
                let snapshot = prepared.then(|| system_from_config(config));
                let resolved = models.resolve(selection, stopping.child_token()).await?;
                if stopping.is_cancelled() { return Err(ProcessingJobFailure::Interrupted); }
                let mut source = VaultSummarySource::new(&context,identity,resolved.include_images).with_custom_templates(allow_custom_templates);
                if let Some(snapshot) = &snapshot {
                    source = source.with_system_snapshot(snapshot.clone());
                }
                let document = SummaryGenerator::new(Arc::new(source),resolved.model.clone()).generate_observed(work,stopping,preview).await?;
                let title = snapshot.map(|snapshot| {
                    Box::new(ConfiguredTitleRunner {
                        generator: TitleGenerator::new(resolved.model, snapshot.language, temperature),
                        resources: Arc::new(Resources { _context: context, _total: total, _running: running }),
                    }) as Box<dyn TitleRunner>
                });
                Ok(PreparedGeneration { document, title })
            }).await.map_err(|_| ProcessingJobFailure::Interrupted)?
        })
    }
}

struct Resources {
    _context: Arc<VaultContext>,
    _total: OwnedSemaphorePermit,
    _running: OwnedSemaphorePermit,
}

struct ConfiguredTitleRunner {
    generator: TitleGenerator,
    resources: Arc<Resources>,
}

impl TitleRunner for ConfiguredTitleRunner {
    fn generate(
        self: Box<Self>,
        enhanced_body: String,
        cancel: CancellationToken,
    ) -> JobFuture<'static, Result<Option<String>, ProcessingJobFailure>> {
        Box::pin(async move {
            let stopping = cancel.child_token();
            let _stop_on_drop = stopping.clone().drop_guard();
            tokio::spawn(async move {
                let Self {
                    generator,
                    resources: _resources,
                } = *self;
                generator.generate(enhanced_body, stopping).await
            })
            .await
            .map_err(|_| ProcessingJobFailure::Interrupted)?
        })
    }
}

#[cfg(test)]
mod tests;
