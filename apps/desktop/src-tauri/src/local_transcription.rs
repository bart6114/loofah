use std::{
    io::Read,
    path::PathBuf,
    sync::{Arc, Mutex as StdMutex, Weak},
};

use hypr_listener2_core::{BatchEvent, BatchParams, BatchProvider, BatchRuntime};
use hypr_loofah_core::{VaultContext, transcription_jobs::*};
use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, TranscriptWithData, TranscriptionFailure, TranscriptionJobRequest,
};
use hypr_transcribe_whisper_local::resources::{
    Limits, ModelSpec, ResourcePhase, WhisperResources,
};
use sha2::{Digest, Sha256};
use tauri::Manager;
use tauri_plugin_local_stt::{LocalModel, LocalSttPluginExt};
use tauri_plugin_transcription::{
    BatchSessionRegistry, CaptureBackgroundGuard, CaptureMaintenanceGate,
};
use tokio::sync::{Mutex, OwnedRwLockWriteGuard};
use tokio_util::sync::CancellationToken;

pub const PRINCIPAL: &str = "00000000-0000-0000-0000-000000000000";

#[derive(Default)]
pub struct LocalTranscriptionState {
    worker: Mutex<Option<TranscriptionWorker>>,
    stop: CancellationToken,
    admissions: Arc<StdMutex<Vec<Weak<Admission>>>>,
}
impl LocalTranscriptionState {
    pub async fn start<R: tauri::Runtime>(
        &self,
        app: tauri::AppHandle<R>,
        context: &Arc<VaultContext>,
    ) -> Result<(), ApplicationError> {
        let mut slot = self.worker.lock().await;
        if self.stop.is_cancelled() || slot.is_some() {
            return Ok(());
        }
        let gate = app
            .try_state::<CaptureMaintenanceGate>()
            .ok_or_else(unavailable)?
            .inner()
            .clone();
        let batches = app
            .try_state::<Arc<BatchSessionRegistry>>()
            .ok_or_else(unavailable)?
            .inner()
            .clone();
        let worker = context
            .start_transcription(
                Arc::new(Engine {
                    app,
                    gate,
                    batches,
                    context: Arc::downgrade(context),
                    admissions: self.admissions.clone(),
                }),
                Arc::new(LocalOwner {
                    stop: self.stop.clone(),
                }),
                TranscriptionLimits::default(),
            )
            .await?;
        if self.stop.is_cancelled() {
            worker.request_shutdown();
        }
        *slot = Some(worker);
        Ok(())
    }
    pub fn initialized(&self) -> bool {
        self.worker
            .try_lock()
            .map(|worker| worker.is_some())
            .unwrap_or(true)
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
        loop {
            let active = self
                .admissions
                .lock()
                .map_err(|_| unavailable())?
                .iter()
                .any(|owner| owner.strong_count() != 0);
            if !active {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        Ok(())
    }
}
fn unavailable() -> ApplicationError {
    ApplicationError {
        code: ErrorCode::Unavailable,
        message: "Local transcription is unavailable".into(),
        retryable: false,
        request_id: None,
        current_revision: None,
        mutation_outcome: None,
    }
}

struct LocalOwner {
    stop: CancellationToken,
}
impl TranscriptionAuthorizer for LocalOwner {
    fn observe<'a>(
        &'a self,
        work: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>> {
        Box::pin(async move {
            self.authorize(work).await?;
            Ok(TranscriptionAccess {
                cancelled: CancellationToken::new(),
                interrupted: self.stop.clone(),
                owner: Box::new(()),
            })
        })
    }
    fn authorize<'a>(
        &'a self,
        work: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        Box::pin(async move {
            if work.principal != PRINCIPAL {
                return Err(TranscriptionFailure::AccessDenied);
            }
            if self.stop.is_cancelled() {
                return Err(TranscriptionFailure::Interrupted);
            }
            Ok(Box::new(()) as Box<dyn Send>)
        })
    }
}

struct Engine<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    gate: CaptureMaintenanceGate,
    batches: Arc<BatchSessionRegistry>,
    context: Weak<VaultContext>,
    admissions: Arc<StdMutex<Vec<Weak<Admission>>>>,
}
struct Admission {
    _context: Arc<VaultContext>,
    background: CaptureBackgroundGuard,
    _legacy_batches: OwnedRwLockWriteGuard<()>,
}
impl Admission {
    fn is_cancelled(&self) -> bool {
        self.background.is_cancelled()
    }
}
struct Prepared {
    model: LocalModel,
    languages: Vec<hypr_language::Language>,
    admission: Arc<Admission>,
    whisper: Option<Arc<WhisperResources>>,
}
impl<R: tauri::Runtime> TranscriptionEngine for Engine<R> {
    fn prepare<'a>(
        &'a self,
        request: &'a TranscriptionJobRequest,
        cancel: CancellationToken,
    ) -> TranscriptionFuture<'a, Result<Box<dyn PreparedTranscription>, TranscriptionFailure>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(TranscriptionFailure::Interrupted);
            }
            let (model, languages) = selection(request)?;
            let legacy_batches = self
                .batches
                .admit_session_maintenance()
                .map_err(|_| TranscriptionFailure::ResourceLimit)?;
            let proof = tauri_plugin_transcription::admit_session_maintenance(self.gate.clone())
                .await
                .map_err(|_| TranscriptionFailure::ResourceLimit)?;
            let admission = Arc::new(Admission {
                _context: self
                    .context
                    .upgrade()
                    .ok_or(TranscriptionFailure::Interrupted)?,
                background: proof
                    .into_background()
                    .ok_or(TranscriptionFailure::ResourceLimit)?,
                _legacy_batches: legacy_batches,
            });
            {
                let mut admissions = self
                    .admissions
                    .lock()
                    .map_err(|_| TranscriptionFailure::EngineUnavailable)?;
                admissions.retain(|owner| owner.strong_count() != 0);
                admissions.push(Arc::downgrade(&admission));
            }
            if !self
                .app
                .local_stt()
                .is_model_downloaded(&model)
                .await
                .map_err(|_| TranscriptionFailure::EngineUnavailable)?
            {
                return Err(TranscriptionFailure::EngineUnavailable);
            }
            let whisper = if let LocalModel::Whisper(whisper) = &model {
                let path = self.app.local_stt().models_dir().join(whisper.file_name());
                let hash_owner = admission.clone();
                let spec = tokio::task::spawn_blocking(move || {
                    let _owner = hash_owner;
                    model_spec(path)
                })
                .await
                .map_err(|_| TranscriptionFailure::EngineUnavailable)??;
                if cancel.is_cancelled() || admission.is_cancelled() {
                    return Err(TranscriptionFailure::Interrupted);
                }
                let resources = Arc::new(
                    WhisperResources::start_device_owned(
                        spec,
                        Limits::default(),
                        Box::new(admission.clone()),
                    )
                    .map_err(|_| TranscriptionFailure::EngineUnavailable)?,
                );
                let mut observer = resources.observer();
                loop {
                    if cancel.is_cancelled() || admission.is_cancelled() {
                        drain_model(&resources).await;
                        return Err(TranscriptionFailure::Interrupted);
                    }
                    match observer.snapshot().phase {
                        ResourcePhase::Ready => break,
                        ResourcePhase::Loading => {}
                        _ => {
                            drain_model(&resources).await;
                            return Err(TranscriptionFailure::EngineUnavailable);
                        }
                    }
                    tokio::select! { _ = observer.changed() => {}, _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {} }
                }
                Some(resources)
            } else {
                None
            };
            if cancel.is_cancelled() || admission.is_cancelled() {
                if let Some(model) = &whisper {
                    drain_model(model).await;
                }
                return Err(TranscriptionFailure::Interrupted);
            }
            Ok(Box::new(Prepared {
                model,
                languages,
                admission,
                whisper,
            }) as Box<dyn PreparedTranscription>)
        })
    }
}
fn selection(
    request: &TranscriptionJobRequest,
) -> Result<(LocalModel, Vec<hypr_language::Language>), TranscriptionFailure> {
    let model: LocalModel =
        serde_json::from_value(serde_json::Value::String(request.model.clone()))
            .map_err(|_| TranscriptionFailure::EngineUnavailable)?;
    let model = match model {
        LocalModel::Soniqo(model) => LocalModel::Soniqo(model.batch_model()),
        LocalModel::Whisper(model) => LocalModel::Whisper(model),
        _ => return Err(TranscriptionFailure::EngineUnavailable),
    };
    if !model.is_available_on_current_platform() {
        return Err(TranscriptionFailure::EngineUnavailable);
    }
    let languages = request
        .languages
        .iter()
        .map(|language| language.parse())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| TranscriptionFailure::EngineUnavailable)?;
    let id = match &model {
        LocalModel::Soniqo(model) => model.to_string(),
        LocalModel::Whisper(model) => model.to_string(),
        _ => unreachable!(),
    };
    if !hypr_listener2_core::is_supported_languages_batch("fmtr", Some(&id), &languages)
        .unwrap_or(false)
    {
        return Err(TranscriptionFailure::EngineUnavailable);
    }
    Ok((model, languages))
}
fn model_spec(path: PathBuf) -> Result<ModelSpec, TranscriptionFailure> {
    let mut file =
        std::fs::File::open(&path).map_err(|_| TranscriptionFailure::EngineUnavailable)?;
    let meta = file
        .metadata()
        .map_err(|_| TranscriptionFailure::EngineUnavailable)?;
    if !meta.is_file() || meta.len() == 0 {
        return Err(TranscriptionFailure::EngineUnavailable);
    }
    let mut digest = Sha256::new();
    let mut bytes = [0u8; 64 * 1024];
    loop {
        let n = file
            .read(&mut bytes)
            .map_err(|_| TranscriptionFailure::EngineUnavailable)?;
        if n == 0 {
            break;
        }
        digest.update(&bytes[..n]);
    }
    Ok(ModelSpec {
        absolute_path: path,
        size_bytes: meta.len(),
        sha256: digest
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}
async fn drain_model(resources: &WhisperResources) {
    let mut status = resources.shutdown();
    loop {
        if matches!(status.snapshot().phase, ResourcePhase::Stopped) {
            return;
        }
        if status.changed().await.is_none() {
            std::future::pending::<()>().await;
        }
    }
}
struct Runtime {
    _source: StdMutex<Box<dyn Send>>,
    admission: Arc<Admission>,
    cancel: CancellationToken,
}
impl BatchRuntime for Runtime {
    fn emit(&self, _: BatchEvent) {}
    fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled() || self.admission.is_cancelled()
    }
}
impl PreparedTranscription for Prepared {
    fn transcribe(
        self: Box<Self>,
        input: TranscriptionInput,
        cancel: CancellationToken,
    ) -> TranscriptionFuture<'static, Result<TranscriptWithData, TranscriptionFailure>> {
        Box::pin(async move {
            let Prepared {
                model,
                languages,
                admission,
                whisper,
            } = *self;
            let (path, source) = input.source.into_parts();
            let runtime = Arc::new(Runtime {
                _source: StdMutex::new(source),
                admission,
                cancel,
            });
            let (provider, id, label) = match model {
                LocalModel::Soniqo(model) => (BatchProvider::Soniqo, model.to_string(), "soniqo"),
                LocalModel::Whisper(model) => {
                    (BatchProvider::WhisperLocal, model.to_string(), "whispercpp")
                }
                _ => unreachable!(),
            };
            let params = BatchParams {
                audio: input.metadata.audio,
                session_id: input.metadata.session_id.clone(),
                provider,
                file_path: path.to_string_lossy().into_owned(),
                model: Some(id),
                base_url: String::new(),
                api_key: String::new(),
                languages,
                keywords: input.request.keywords,
                num_speakers: None,
                min_speakers: None,
                max_speakers: None,
            };
            let result = if runtime.is_cancelled() {
                Err(TranscriptionFailure::Interrupted)
            } else {
                let output = match &whisper {
                    Some(resources) => {
                        hypr_listener2_core::run_owned_whisper_batch(
                            runtime.clone(),
                            params,
                            resources.clone(),
                        )
                        .await
                    }
                    None => hypr_listener2_core::run_batch(runtime.clone(), params).await,
                };
                output.map_err(|_| {
                    if runtime.is_cancelled() {
                        TranscriptionFailure::Interrupted
                    } else {
                        TranscriptionFailure::EngineFailed
                    }
                })
            };
            if let Some(resources) = &whisper {
                drain_model(resources).await;
            }
            let output = result?;
            // Core retains a completed result before applying cancellation/revocation
            // to publication; cancellation must not erase known model output.
            let metadata = input.metadata;
            let mut new_id = || uuid::Uuid::new_v4().to_string();
            Ok(
                hypr_listener2_core::transcript::transcript_from_batch_response(
                    &output.response,
                    hypr_listener2_core::transcript::BatchTranscriptMeta {
                        session_id: &metadata.session_id,
                        user_id: &metadata.user_id,
                        provider: label,
                        created_at: metadata.created_at.clone(),
                        started_at_ms: metadata.started_at_ms(),
                        memo_md: metadata.memo_md.clone(),
                    },
                    &mut new_id,
                )
                .unwrap_or_else(|| metadata.empty_transcript()),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(model: &str, languages: &[&str]) -> TranscriptionJobRequest {
        TranscriptionJobRequest {
            session_id: uuid::Uuid::new_v4().to_string(),
            expected_recording_sha256: "a".repeat(64),
            expected_transcript: hypr_loofah_protocol::AbsentTranscriptExpectation::Absent,
            model: model.into(),
            languages: languages.iter().map(|value| (*value).into()).collect(),
            keywords: vec![],
        }
    }
    #[test]
    fn local_model_policy_cannot_resolve_paths_urls_cloud_or_diarizer() {
        for model in [
            "/tmp/model.bin",
            "https://host/model",
            "cloud",
            "whisper-1",
            "diarizer-fluid-community",
            "am-parakeet-v3",
            "soniqo-qwen3-small",
        ] {
            assert!(selection(&request(model, &["en"])).is_err(), "{model}");
        }
    }
    #[test]
    fn whisper_aliases_and_language_policy_preserve_supported_device_choices() {
        for model in [
            "whisper-large-v3",
            "QuantizedTiny",
            "QuantizedTinyEn",
            "QuantizedBase",
            "QuantizedBaseEn",
            "QuantizedSmall",
            "QuantizedSmallEn",
            "QuantizedLargeTurbo",
        ] {
            let supported =
                LocalModel::Whisper(model.parse().unwrap()).is_available_on_current_platform();
            assert_eq!(
                selection(&request(model, &["en"])).is_ok(),
                supported,
                "{model}"
            );
        }
        assert!(selection(&request("QuantizedBaseEn", &["nl"])).is_err());
        assert!(selection(&request("QuantizedBase", &["not-a-language"])).is_err());
        for model in tauri_plugin_local_stt::SoniqoModel::all() {
            let request = request(model.as_str(), &["en"]);
            let result = selection(&request);
            if model.batch_model().is_available_on_current_platform()
                && model
                    .batch_model()
                    .supports_languages(&[hypr_language::ISO639::En.into()])
            {
                assert!(
                    matches!(result, Ok((LocalModel::Soniqo(selected), _)) if selected == model.batch_model()),
                    "{}",
                    model.as_str()
                );
            }
        }
    }
    #[tokio::test]
    async fn local_principal_and_shutdown_are_authoritative() {
        let owner = LocalOwner {
            stop: CancellationToken::new(),
        };
        let mut work = TranscriptionWork {
            job_id: uuid::Uuid::new_v4().to_string(),
            principal: PRINCIPAL.into(),
            request: request("QuantizedBase", &["en"]),
        };
        let access = owner.observe(&work).await.unwrap();
        assert!(!access.interrupted.is_cancelled());
        work.principal = "desktop".into();
        assert!(matches!(
            owner.authorize(&work).await,
            Err(TranscriptionFailure::AccessDenied)
        ));
        work.principal = PRINCIPAL.into();
        owner.stop.cancel();
        assert!(access.interrupted.is_cancelled());
        assert!(matches!(
            owner.authorize(&work).await,
            Err(TranscriptionFailure::Interrupted)
        ));
    }
    #[test]
    fn model_fingerprint_is_exact_and_missing_models_stay_offline() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("model.bin");
        assert!(model_spec(path.clone()).is_err());
        std::fs::write(&path, b"abc").unwrap();
        let spec = model_spec(path).unwrap();
        assert_eq!(spec.size_bytes, 3);
        assert_eq!(
            spec.sha256,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
