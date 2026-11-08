mod persistence;
#[cfg(test)]
mod tests;

use hypr_loofah_core::{
    VaultContext,
    local_capture_intents::{
        LocalCaptureDeclaration, LocalCaptureIntents, LocalCapturePersistence,
        LocalTranscriptionOptions,
    },
    local_session_work::LocalSessionWorkRegistry,
};
use hypr_loofah_protocol::{
    ApplicationError, CaptureProcessingSummary, DocumentTarget, ErrorCode, TitlePolicy,
};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    task::Poll,
};
use tauri::Manager;
use tauri_plugin_local_stt::{LocalModel, LocalSttPluginExt};
use tauri_plugin_transcription::{
    CaptureParams, DeviceCaptureCompletion, DeviceCaptureStartError, LocalCaptureHost,
    OwnedLocalCaptureArgs, TranscriptionMode,
};
use tokio::sync::OnceCell;
use tokio_util::sync::CancellationToken;

struct Task {
    request: String,
    session: String,
    finished: CancellationToken,
    safe: AtomicBool,
}
#[derive(Default)]
pub struct LocalCaptureState {
    service: OnceCell<Arc<LocalCaptureIntents>>,
    tasks: Mutex<HashMap<String, Arc<Task>>>,
    closing: Mutex<bool>,
}
impl LocalCaptureState {
    pub async fn initialize(&self, context: Arc<VaultContext>) -> Result<(), ApplicationError> {
        self.service
            .get_or_try_init(|| LocalCaptureIntents::open(context))
            .await?;
        Ok(())
    }
    pub fn initialized(&self) -> bool {
        self.service.get().is_some()
    }
    pub fn request_shutdown(&self) {
        if let Ok(mut closing) = self.closing.lock() {
            *closing = true;
        }
        if let Some(service) = self.service.get() {
            service.request_shutdown();
        }
        if let Ok(tasks) = self.tasks.lock() {
            for (request, task) in tasks
                .iter()
                .filter(|(_, task)| !task.finished.is_cancelled())
            {
                let session = task.session.clone();
                let request = request.clone();
                tokio::spawn(async move {
                    tauri_plugin_transcription::stop_owned_local_capture(session, request).await;
                });
            }
        }
    }
    pub async fn drain(&self) -> Result<(), ApplicationError> {
        self.request_shutdown();
        loop {
            let pending = self
                .tasks
                .lock()
                .map_err(|_| unavailable())?
                .values()
                .any(|task| !task.finished.is_cancelled() || !task.safe.load(Ordering::Acquire));
            if !pending {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        if let Some(service) = self.service.get() {
            service.shutdown().await;
        }
        Ok(())
    }
}
struct Host<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    context: Weak<VaultContext>,
    state: Arc<LocalCaptureState>,
    work: LocalSessionWorkRegistry,
}
pub fn configure<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    context: &Arc<VaultContext>,
) -> Result<(), ApplicationError> {
    let state = app
        .try_state::<Arc<LocalCaptureState>>()
        .ok_or_else(unavailable)?
        .inner()
        .clone();
    let work = app
        .try_state::<LocalSessionWorkRegistry>()
        .ok_or_else(unavailable)?
        .inner()
        .clone();
    let host: Arc<dyn LocalCaptureHost> = Arc::new(Host {
        app: app.clone(),
        context: Arc::downgrade(context),
        state,
        work,
    });
    app.manage(host);
    Ok(())
}
impl<R: tauri::Runtime> LocalCaptureHost for Host<R> {
    fn start(
        &self,
        params: CaptureParams,
        request_id: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let context = self.context.upgrade();
        let app = self.app.clone();
        let state = self.state.clone();
        let work = self.work.clone();
        Box::pin(async move {
            let context = context.ok_or_else(|| unavailable().to_string())?;
            tokio::spawn(start(app, context, state, work, params, request_id))
                .await
                .map_err(|_| "Local capture admission requires recovery".to_owned())?
        })
    }
}
async fn start<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    context: Arc<VaultContext>,
    state: Arc<LocalCaptureState>,
    registry: LocalSessionWorkRegistry,
    mut params: CaptureParams,
    request: String,
) -> Result<(), String> {
    if *state
        .closing
        .lock()
        .map_err(|_| unavailable().to_string())?
    {
        return Err(unavailable().to_string());
    }
    let service = state
        .service
        .get()
        .ok_or_else(|| unavailable().to_string())?
        .clone();
    let config = context.settings.snapshot();
    // Capture parameters cannot turn Local mode into a cloud transcription transport.
    let model = if params.model.is_empty() {
        None
    } else {
        let model: LocalModel =
            serde_json::from_value(serde_json::Value::String(params.model.clone()))
                .map_err(|_| "Select a supported on-device transcription model".to_owned())?;
        if !model.is_available_on_current_platform() || matches!(model, LocalModel::Diarizer(_)) {
            return Err("Select a supported on-device transcription model".into());
        }
        Some(model)
    };
    params.api_key.clear();
    params.base_url.clear();
    if let Some(model) = &model {
        if let Ok(Some(server)) = app.local_stt().get_server_for_model(model).await
            && server.model.as_ref() == Some(model)
        {
            params.base_url = server.url.unwrap_or_default();
        }
    }
    if params.base_url.is_empty() {
        params.transcription_mode = Some(TranscriptionMode::Batch);
    }
    let transcription = model.map(|model| LocalTranscriptionOptions {
        model: match model {
            LocalModel::Soniqo(model) => model.batch_model().to_string(),
            LocalModel::Whisper(model) => model.to_string(),
            _ => unreachable!(),
        },
        languages: params
            .languages
            .iter()
            .map(|language| language.bcp47_code())
            .collect(),
        keywords: vec![],
    });
    let append = match context
        .store
        .open_recording(&params.session_id)
        .await
        .map_err(|_| "Recording state is unavailable".to_owned())?
    {
        Some(hypr_vault_write::media::RecordingSnapshot::Absent) => false,
        Some(hypr_vault_write::media::RecordingSnapshot::File(_)) => true,
        _ => return Err("Recording is busy or unavailable".into()),
    };
    let summary = config
        .current_llm_provider
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .zip(
            config
                .current_llm_model
                .as_deref()
                .filter(|value| !value.trim().is_empty()),
        )
        .map(|_| CaptureProcessingSummary {
            target: DocumentTarget::Summary,
            title_policy: TitlePolicy::GenerateIfEmpty,
        });
    let batch_requested =
        transcription.is_some() && params.transcription_mode == Some(TranscriptionMode::Batch);
    let intent = service
        .declare(LocalCaptureDeclaration {
            request_id: request.clone(),
            session_id: params.session_id.clone(),
            append,
            batch_requested,
            transcription,
            summary,
        })
        .await
        .map_err(|error| error.to_string())?;
    let work = match registry
        .begin(params.session_id.clone(), request.clone())
        .await
    {
        Ok(work) => work,
        Err(error) => {
            let _ = intent.not_started().await;
            return Err(error.to_string());
        }
    };
    if let Err(error) = context.store.prepare_recording(&params.session_id).await {
        let _ = work.not_started();
        let _ = intent.not_started().await;
        return Err(error.to_string());
    }
    let display = match tauri_plugin_transcription::local_capture_display_runtime(app) {
        Ok(display) => display,
        Err(_) => {
            let _ = context
                .store
                .release_recording_prepare(&params.session_id)
                .await;
            let _ = work.not_started();
            let _ = intent.not_started().await;
            return Err("Local recording display is unavailable".into());
        }
    };
    let task = Arc::new(Task {
        request: request.clone(),
        session: params.session_id.clone(),
        finished: CancellationToken::new(),
        safe: false.into(),
    });
    let registered = state
        .tasks
        .lock()
        .map(|mut tasks| {
            tasks.retain(|_, task| !task.safe.load(Ordering::Acquire));
            if tasks.contains_key(&request) {
                return false;
            }
            tasks.insert(request.clone(), task.clone());
            true
        })
        .unwrap_or(false);
    if !registered {
        let _ = context.store.release_recording_prepare(&task.session).await;
        let _ = work.not_started();
        let _ = intent.not_started().await;
        return Err("The original capture request is already active".into());
    }
    let (reporter, completion) = DeviceCaptureCompletion::channel();
    let not_dispatched = CancellationToken::new();
    let (runtime, persistence) = persistence::start(
        context.clone(),
        params.session_id.clone(),
        request.clone(),
        display,
        completion,
        not_dispatched.clone(),
    );
    let args = OwnedLocalCaptureArgs {
        request_id: request,
        params: params.into(),
        runtime,
        completion: reporter,
    };
    let result = dispatch(
        &state,
        tauri_plugin_transcription::start_owned_local_capture(args),
    )
    .await;
    if matches!(
        result,
        Err(DeviceCaptureStartError::NotDispatched | DeviceCaptureStartError::Rejected)
    ) {
        not_dispatched.cancel();
        let _ = persistence.await;
        let _ = context.store.release_recording_prepare(&task.session).await;
        let registry_result = work.not_started();
        let intent_result = intent.not_started().await;
        task.safe.store(
            registry_result.is_ok() && intent_result.is_ok(),
            Ordering::Release,
        );
        task.finished.cancel();
        return Err("Capture could not start".into());
    }
    tokio::spawn(async move {
        let completed = match persistence.await {
            Ok(completed) => completed,
            Err(_) => {
                let _ = intent
                    .persistence_finished(Err("Native capture persistence worker stopped".into()))
                    .await;
                task.finished.cancel();
                return;
            }
        };
        let joined = completed.physical.supervisor_ended
            && completed.physical.resources_released_observed_at.is_some();
        let physical = if joined && completed.physical.failure.is_none() {
            Ok(())
        } else {
            Err("Original native capture resources did not confirm completion".into())
        };
        let physical_saved = intent.physical_finished(physical).await.is_ok();
        if joined {
            let _ = work.producer_finished();
        }
        let persisted = completed.persistence.map(|value| LocalCapturePersistence {
            live_transcription_active: value.live_active,
            needs_batch_repair: value.needs_repair,
            new_transcript_id: value.transcript_id,
        });
        let canonical_saved = persisted.is_ok();
        let persistence_saved = intent.persistence_finished(persisted).await.is_ok();
        let registry_saved = if canonical_saved {
            registry
                .persistence_finished(&task.session, &task.request)
                .is_ok()
        } else {
            false
        };
        task.safe.store(
            joined && physical_saved && persistence_saved && (!canonical_saved || registry_saved),
            Ordering::Release,
        );
        task.finished.cancel();
    });
    result.map_err(|_| {
        "Capture start outcome is unconfirmed; the original request was retained".into()
    })
}
async fn dispatch(
    state: &LocalCaptureState,
    future: impl Future<Output = Result<(), DeviceCaptureStartError>>,
) -> Result<(), DeviceCaptureStartError> {
    tokio::pin!(future);
    let mut sent = false;
    std::future::poll_fn(|cx| {
        if sent {
            return future.as_mut().poll(cx);
        }
        let Ok(closing) = state.closing.lock() else {
            return Poll::Ready(Err(DeviceCaptureStartError::NotDispatched));
        };
        if *closing {
            return Poll::Ready(Err(DeviceCaptureStartError::NotDispatched));
        }
        let result = future.as_mut().poll(cx);
        sent = true;
        drop(closing);
        result
    })
    .await
}
fn unavailable() -> ApplicationError {
    hypr_loofah_client::failure(ErrorCode::Unavailable, "Local capture is not ready")
}
