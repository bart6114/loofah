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
    engine: Engine,
    limits: TranscriptionLimits,
}

enum Engine {
    Whisper(Arc<CpuTranscriptionHost>),
    Onnx(Arc<crate::transcription_onnx::OnnxHost>),
}

impl TranscriptionHost {
    pub(crate) fn load(config: &TranscriptionConfig) -> Result<Arc<Self>> {
        config
            .validate()
            .map_err(|message| ApiError::new(ErrorCode::Validation, &message))?;
        if config.onnx.is_some() {
            return Ok(Arc::new(Self {
                engine: Engine::Onnx(crate::transcription_onnx::OnnxHost::start(config)),
                limits: config.limits(),
            }));
        }
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
            crate::speech_admission::SpeechAdmission::new(
                config.workload_groups,
                config.live_reserved_groups,
                config.max_queued,
            )
            .map_err(|message| ApiError::new(ErrorCode::Validation, message))?,
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
            engine: Engine::Whisper(engine),
            limits: config.limits(),
        }))
    }

    pub(crate) fn engine(&self) -> (Arc<dyn TranscriptionEngine>, TranscriptionLimits) {
        let engine: Arc<dyn TranscriptionEngine> = match &self.engine {
            Engine::Whisper(engine) => engine.clone(),
            Engine::Onnx(engine) => engine.clone(),
        };
        (engine, self.limits)
    }

    pub(crate) fn live_engine(&self) -> Arc<dyn hypr_loofah_core::capture_live::CaptureLiveEngine> {
        match &self.engine {
            Engine::Whisper(engine) => engine.clone(),
            Engine::Onnx(engine) => engine.clone(),
        }
    }

    pub(crate) fn policy(&self, model: &str) -> hypr_loofah_protocol::TranscriptionPolicy {
        match &self.engine {
            Engine::Whisper(engine) => model_policy(
                model,
                engine.observer().snapshot(),
                engine.accepting(),
                engine.capacity_exhausted(),
            ),
            Engine::Onnx(engine) => engine.policy(),
        }
    }

    pub(crate) fn stop_admission(&self) {
        match &self.engine {
            Engine::Whisper(engine) => engine.stop_admission(),
            Engine::Onnx(engine) => engine.stop_admission(),
        }
    }

    #[cfg(test)]
    pub(crate) fn observer(&self) -> hypr_transcribe_whisper_local::resources::ResourceObserver {
        match &self.engine {
            Engine::Whisper(engine) => engine.observer(),
            Engine::Onnx(_) => panic!("Whisper-only fixture observer"),
        }
    }

    pub(crate) async fn shutdown(&self) {
        let engine = match &self.engine {
            Engine::Whisper(engine) => engine,
            Engine::Onnx(engine) => return engine.shutdown().await,
        };
        let mut observer = engine.shutdown();
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

fn model_policy(
    model: &str,
    snapshot: hypr_transcribe_whisper_local::resources::ResourceSnapshot,
    accepting: bool,
    capacity_exhausted: bool,
) -> hypr_loofah_protocol::TranscriptionPolicy {
    use hypr_loofah_protocol::TranscriptionAvailability as State;
    let availability = match snapshot.phase {
        ResourcePhase::Stopping | ResourcePhase::Stopped => State::Stopping,
        _ if !accepting => State::Unavailable,
        ResourcePhase::Loading => State::Loading,
        ResourcePhase::Ready if capacity_exhausted => State::InUse,
        ResourcePhase::Ready => State::Ready,
        ResourcePhase::Failed(_) => State::Unavailable,
    };
    hypr_loofah_protocol::TranscriptionPolicy {
        model: Some(model.into()),
        availability,
        batch_enabled: true,
        live_enabled: true,
    }
}
#[cfg(test)]
mod policy_tests {
    use super::*;
    use hypr_loofah_protocol::TranscriptionAvailability as State;
    use hypr_transcribe_whisper_local::resources::{ResourceError, ResourceSnapshot};
    #[test]
    fn model_policy_keeps_reserved_capacity_visible_during_batch_work() {
        let policy = model_policy(
            "alias",
            ResourceSnapshot {
                phase: ResourcePhase::Ready,
                batch_groups: 1,
                live_groups: 0,
                live_channels: 0,
            },
            true,
            false,
        );
        assert_eq!(policy.availability, State::Ready);
        assert!(policy.live_enabled && policy.batch_enabled);
    }

    #[test]
    fn model_policy_is_advisory_sanitized_and_respects_closed_admission() {
        for (phase, batch, live, accepting, state) in [
            (ResourcePhase::Loading, 0, 0, true, State::Loading),
            (ResourcePhase::Ready, 0, 0, true, State::Ready),
            (ResourcePhase::Ready, 1, 0, true, State::InUse),
            (ResourcePhase::Ready, 0, 1, true, State::InUse),
            (ResourcePhase::Ready, 0, 0, false, State::Unavailable),
            (
                ResourcePhase::Failed(ResourceError::Failed(
                    "/private/operator/model.bin secret".into(),
                )),
                0,
                0,
                true,
                State::Unavailable,
            ),
            (ResourcePhase::Stopping, 0, 0, false, State::Stopping),
            (ResourcePhase::Stopped, 0, 0, false, State::Stopping),
        ] {
            let policy = model_policy(
                "public-operator-alias",
                ResourceSnapshot {
                    phase,
                    batch_groups: batch,
                    live_groups: live,
                    live_channels: live * 2,
                },
                accepting,
                batch + live > 0,
            );
            assert_eq!(policy.availability, state);
            assert!(policy.batch_enabled && policy.live_enabled);
            assert_eq!(policy.model.as_deref(), Some("public-operator-alias"));
            assert!(!serde_json::to_string(&policy).unwrap().contains("private"));
            assert!(!serde_json::to_string(&policy).unwrap().contains("secret"));
        }
    }
}
