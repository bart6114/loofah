use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, Ordering},
};

use hypr_loofah_core::transcription_jobs::{
    PreparedTranscription, TranscriptionEngine, TranscriptionFuture, TranscriptionInput,
};
use hypr_loofah_protocol::{
    TranscriptionAvailability, TranscriptionFailure, TranscriptionJobRequest, TranscriptionPolicy,
};
use hypr_transcribe_onnx::{
    BatchSession, LiveSession,
    diarize::Diarizer,
    models::{Model, VerifiedModel},
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

mod batch;
mod live;
#[cfg(test)]
mod tests;

const LOADING: u8 = 0;
const READY: u8 = 1;
const FAILED: u8 = 2;
const STOPPING: u8 = 3;
const STOPPED: u8 = 4;

pub(crate) fn model_fingerprint() -> String {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(b"loofah-server-onnx-models-v1\0");
    for model in [Model::Batch, Model::Streaming, Model::Diarizer] {
        digest.update(model.fingerprint());
    }
    digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

struct Models {
    batch: BatchSession,
    diarizer: Diarizer,
    streaming: Arc<VerifiedModel>,
}

struct Resources {
    phase: AtomicU8,
    shutdown: CancellationToken,
    stopping: AtomicBool,
    slot: Arc<Semaphore>,
    models: Mutex<Option<Models>>,
}

pub(crate) struct OnnxHost {
    model: String,
    resources: Arc<Resources>,
}

impl OnnxHost {
    pub(crate) fn start(config: &crate::config::TranscriptionConfig) -> Arc<Self> {
        let resources = Arc::new(Resources {
            shutdown: CancellationToken::new(),
            phase: AtomicU8::new(LOADING),
            stopping: AtomicBool::new(false),
            slot: Arc::new(Semaphore::new(1)),
            models: Mutex::new(None),
        });
        let host = Arc::new(Self {
            model: config.model.clone(),
            resources: resources.clone(),
        });
        let config = config.clone();
        let permit = resources
            .slot
            .clone()
            .try_acquire_owned()
            .expect("new model slot");
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
                || -> anyhow::Result<Models> {
                    let onnx = config.onnx.as_ref().expect("validated ONNX configuration");
                    let batch =
                        Arc::new(Model::Batch.snapshot(&config.model_path, &onnx.snapshot_root)?);
                    let streaming = Arc::new(
                        Model::Streaming
                            .snapshot(&onnx.streaming_model_path, &onnx.snapshot_root)?,
                    );
                    let diarizer = Arc::new(
                        Model::Diarizer.snapshot(&onnx.diarizer_model_path, &onnx.snapshot_root)?,
                    );
                    let batch = BatchSession::load_verified(batch)?;
                    let diarizer = Diarizer::load_verified(diarizer)?;
                    // Validate the live graphs at startup, but do not reuse a previous stream's decoder state.
                    drop(LiveSession::load_verified_pair(streaming.clone())?);
                    Ok(Models {
                        batch,
                        diarizer,
                        streaming,
                    })
                },
            ));
            match loaded {
                Ok(Ok(models)) if !resources.stopping.load(Ordering::Acquire) => {
                    *resources.models.lock().unwrap_or_else(|e| e.into_inner()) = Some(models);
                    resources.phase.store(READY, Ordering::Release);
                }
                _ => {
                    if !resources.stopping.load(Ordering::Acquire) {
                        resources.phase.store(FAILED, Ordering::Release);
                        tracing::error!(
                            "Configured ONNX models could not be verified or initialized"
                        );
                    }
                }
            }
        });
        host
    }

    fn admit(&self) -> Result<OwnedSemaphorePermit, TranscriptionFailure> {
        if self.resources.stopping.load(Ordering::Acquire)
            || self.resources.phase.load(Ordering::Acquire) != READY
        {
            return Err(TranscriptionFailure::EngineUnavailable);
        }
        let permit = self
            .resources
            .slot
            .clone()
            .try_acquire_owned()
            .map_err(|_| TranscriptionFailure::ResourceLimit)?;
        if self.resources.stopping.load(Ordering::Acquire) {
            return Err(TranscriptionFailure::EngineUnavailable);
        }
        Ok(permit)
    }

    pub(crate) fn policy(&self) -> TranscriptionPolicy {
        let phase = if self.resources.stopping.load(Ordering::Acquire) {
            STOPPING
        } else {
            self.resources.phase.load(Ordering::Acquire)
        };
        let availability = match phase {
            LOADING => TranscriptionAvailability::Loading,
            READY if self.resources.slot.available_permits() == 0 => {
                TranscriptionAvailability::InUse
            }
            READY => TranscriptionAvailability::Ready,
            STOPPING | STOPPED => TranscriptionAvailability::Stopping,
            _ => TranscriptionAvailability::Unavailable,
        };
        TranscriptionPolicy {
            model: Some(self.model.clone()),
            availability,
            batch_enabled: true,
            live_enabled: true,
        }
    }

    pub(crate) fn stop_admission(&self) {
        self.resources.stopping.store(true, Ordering::Release);
        self.resources.shutdown.cancel();
        self.resources.phase.store(STOPPING, Ordering::Release);
    }

    pub(crate) async fn shutdown(&self) {
        self.stop_admission();
        let resources = self.resources.clone();
        let permit = resources
            .slot
            .clone()
            .acquire_owned()
            .await
            .expect("owned model slot");
        let task = tokio::task::spawn_blocking(move || {
            let models = resources
                .models
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take();
            drop(models);
            resources.phase.store(STOPPED, Ordering::Release);
            drop(permit);
        });
        if task.await.is_err() {
            tracing::error!(
                "ONNX model destruction could not be confirmed; shutdown remains pending"
            );
            std::future::pending::<()>().await;
        }
    }
}

fn options(
    model: &str,
    requested: &str,
    languages: &[String],
    keywords: &[String],
    live: bool,
) -> Result<(), TranscriptionFailure> {
    const BATCH_LANGUAGES: &[&str] = &[
        "bg", "cs", "da", "de", "el", "en", "es", "et", "fi", "fr", "hr", "hu", "it", "lt", "lv",
        "mt", "nl", "pl", "pt", "ro", "ru", "sk", "sl", "sv", "uk",
    ];
    if requested != model
        || !keywords.is_empty()
        || languages.len() > 1
        || languages.iter().any(|language| {
            if live {
                language != "en"
            } else {
                !BATCH_LANGUAGES.contains(&language.as_str())
            }
        })
    {
        return Err(TranscriptionFailure::EngineUnavailable);
    }
    Ok(())
}

struct Prepared {
    resources: Arc<Resources>,
    permit: OwnedSemaphorePermit,
    request: TranscriptionJobRequest,
}
impl TranscriptionEngine for OnnxHost {
    fn prepare<'a>(
        &'a self,
        request: &'a TranscriptionJobRequest,
        cancel: CancellationToken,
    ) -> TranscriptionFuture<'a, Result<Box<dyn PreparedTranscription>, TranscriptionFailure>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(TranscriptionFailure::Interrupted);
            }
            hypr_loofah_protocol::validate_transcription_job_request(request)
                .map_err(|_| TranscriptionFailure::InvalidOutput)?;
            options(
                &self.model,
                &request.model,
                &request.languages,
                &request.keywords,
                false,
            )?;
            let permit = self.admit()?;
            Ok(Box::new(Prepared {
                resources: self.resources.clone(),
                permit,
                request: request.clone(),
            }) as Box<dyn PreparedTranscription>)
        })
    }
}
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl PreparedTranscription for Prepared {
    fn transcribe(
        self: Box<Self>,
        input: TranscriptionInput,
        cancel: CancellationToken,
    ) -> TranscriptionFuture<
        'static,
        Result<hypr_vault_read::TranscriptWithData, TranscriptionFailure>,
    > {
        Box::pin(async move {
            let cancel = cancel.child_token();
            let _cancel = CancelOnDrop(cancel.clone());
            tokio::spawn(batch::run(*self, input, cancel))
                .await
                .unwrap_or(Err(TranscriptionFailure::EngineFailed))
        })
    }
}
