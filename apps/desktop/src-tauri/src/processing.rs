use std::sync::Arc;

use hypr_loofah_core::{
    VaultContext,
    jobs::{
        JobAuthorizer, JobFuture, JobWork, JobWorker,
        configured::{
            ConfiguredSummaryGenerator, LlmSelection, ProcessingAdmission, SummaryModel,
            SummaryModels,
        },
        providers::connections::{LocalHttpModels, ProviderSecrets},
        source::SummaryIdentity,
    },
};
use hypr_loofah_protocol::{ApplicationError, ProcessingJobFailure};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

pub const PRINCIPAL: &str = "desktop";

#[derive(Default)]
pub struct ProcessingState {
    worker: Mutex<Option<JobWorker>>,
    stop: CancellationToken,
}

impl ProcessingState {
    pub fn stopping(&self) -> CancellationToken {
        self.stop.clone()
    }

    pub async fn start<R: tauri::Runtime>(
        &self,
        app: tauri::AppHandle<R>,
        context: &Arc<VaultContext>,
    ) -> Result<(), ApplicationError> {
        let mut slot = self.worker.lock().await;
        if self.stop.is_cancelled() || slot.is_some() {
            return Ok(());
        }
        let models = LocalModels::new(app);
        let generator = ConfiguredSummaryGenerator::new(
            context,
            SummaryIdentity::LocalOwner,
            Arc::new(models),
            ProcessingAdmission::new(1, 8).expect("valid desktop admission limits"),
            true,
        );
        let worker = context
            .start_processing(Arc::new(generator), Arc::new(LocalOwner))
            .await?;
        if self.stop.is_cancelled() {
            worker.request_shutdown();
        }
        *slot = Some(worker);
        Ok(())
    }

    pub fn request_shutdown(&self) {
        self.stop.cancel();
        if let Ok(slot) = self.worker.try_lock()
            && let Some(worker) = slot.as_ref()
        {
            worker.request_shutdown();
        }
    }

    pub async fn shutdown(&self) -> Result<(), ApplicationError> {
        self.request_shutdown();
        if let Some(worker) = self.worker.lock().await.take() {
            worker.shutdown().await?;
        }
        Ok(())
    }
}

struct LocalOwner;

impl JobAuthorizer for LocalOwner {
    fn authorize<'a>(
        &'a self,
        work: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async move {
            if work.principal != PRINCIPAL {
                return Err(ProcessingJobFailure::AccessDenied);
            }
            Ok(Box::new(()) as Box<dyn Send>)
        })
    }
}

struct DeviceSecrets<R: tauri::Runtime>(tauri::AppHandle<R>);

impl<R: tauri::Runtime> ProviderSecrets for DeviceSecrets<R> {
    fn read(
        &self,
        provider_id: String,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<Option<String>, ProcessingJobFailure>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            // Native reads reuse the same per-provider coordinates and legacy migration
            // as Intelligence settings on macOS Keychain and Windows Credential Manager.
            let secret = tauri_plugin_store2::read_secret(
                self.0.clone(),
                "ai-provider-api-keys".into(),
                format!("llm:{provider_id}"),
            )
            .await
            .map_err(|_| ProcessingJobFailure::ProviderUnavailable)?;
            if cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            Ok(secret)
        })
    }
}

struct LocalModels<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    http: LocalHttpModels,
}

impl<R: tauri::Runtime> LocalModels<R> {
    fn new(app: tauri::AppHandle<R>) -> Self {
        Self {
            http: LocalHttpModels::new(Arc::new(DeviceSecrets(app.clone()))),
            app,
        }
    }
}

impl<R: tauri::Runtime> SummaryModels for LocalModels<R> {
    fn resolve(
        &self,
        selection: LlmSelection,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<SummaryModel, ProcessingJobFailure>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(ProcessingJobFailure::Interrupted);
            }
            if selection.provider_id == "chatgpt_subscription" {
                Ok(SummaryModel {
                    model: Arc::new(crate::chatgpt::model::SummaryModel::new(
                        self.app.clone(),
                        selection.model_id,
                    )),
                    include_images: true,
                })
            } else {
                self.http.resolve(selection, cancel).await
            }
        })
    }
}

#[cfg(test)]
mod tests;
