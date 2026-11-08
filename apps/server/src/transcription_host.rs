use std::sync::Arc;

use hypr_loofah_core::transcription_jobs::{TranscriptionEngine, TranscriptionLimits};
use hypr_loofah_protocol::ErrorCode;
use hypr_transcribe_whisper_local::resources::{Limits, ModelSpec, ResourcePhase};

use crate::{
    config::TranscriptionConfig,
    error::{ApiError, Result},
    transcription_cpu::CpuTranscriptionHost,
};

pub(crate) struct TranscriptionHost {
    engine: Arc<CpuTranscriptionHost>,
    limits: TranscriptionLimits,
}

impl TranscriptionHost {
    pub(crate) fn load(config: &TranscriptionConfig) -> Result<Arc<Self>> {
        config
            .validate()
            .map_err(|message| ApiError::new(ErrorCode::Validation, &message))?;
        let engine = CpuTranscriptionHost::start(
            config.model.clone(),
            ModelSpec {
                absolute_path: config.model_path.clone(),
                size_bytes: config.model_size_bytes,
                sha256: config.model_sha256.clone(),
            },
            Limits {
                workload_groups: config.workload_groups,
                ..Default::default()
            },
        )
        .map_err(|_| {
            ApiError::new(
                ErrorCode::Validation,
                "Invalid CPU transcription configuration",
            )
        })?;
        let mut state = engine.observer();
        tokio::spawn(async move {
            loop {
                match state.snapshot().phase {
                    ResourcePhase::Ready => {
                        tracing::info!("CPU transcription model is ready");
                        return;
                    }
                    ResourcePhase::Failed(_) => {
                        tracing::error!(
                            "Configured CPU transcription model could not be loaded or verified"
                        );
                        return;
                    }
                    ResourcePhase::Stopping | ResourcePhase::Stopped => return,
                    ResourcePhase::Loading => {}
                }
                if state.changed().await.is_none() {
                    return;
                }
            }
        });
        Ok(Arc::new(Self {
            engine,
            limits: config.limits(),
        }))
    }

    pub(crate) fn engine(&self) -> (Arc<dyn TranscriptionEngine>, TranscriptionLimits) {
        (self.engine.clone(), self.limits)
    }

    pub(crate) fn stop_admission(&self) {
        self.engine.stop_admission();
    }

    #[cfg(test)]
    pub(crate) fn observer(&self) -> hypr_transcribe_whisper_local::resources::ResourceObserver {
        self.engine.observer()
    }

    pub(crate) async fn shutdown(&self) {
        let mut observer = self.engine.shutdown();
        loop {
            if observer.snapshot().phase == ResourcePhase::Stopped {
                return;
            }
            if matches!(observer.snapshot().phase, ResourcePhase::Failed(_)) {
                tracing::error!(
                    "CPU model destruction failed; owned service shutdown remains pending"
                );
                std::future::pending::<()>().await;
            }
            if observer.changed().await.is_none() {
                tracing::error!(
                    "CPU model destruction could not be confirmed; owned service shutdown remains pending"
                );
                std::future::pending::<()>().await;
            }
        }
    }
}
