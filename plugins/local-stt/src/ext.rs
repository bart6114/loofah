use std::{collections::HashMap, path::PathBuf, sync::Arc};

#[cfg(feature = "whisper-cpp")]
use ractor::{ActorRef, call_t, registry};
use tauri_specta::Event;

use tauri::{Manager, Runtime};

use hypr_model_downloader::{DownloadStatus, ModelDownloadManager, ModelDownloaderRuntime};

#[cfg(feature = "whisper-cpp")]
use crate::server::internal;
use crate::{
    model::{DiarizerModel, LocalModel, LocalModelKind},
    server::{ServerInfo, ServerStatus, ServerType, supervisor},
    types::DownloadProgressPayload,
};

struct TauriModelRuntime<R: Runtime> {
    app_handle: tauri::AppHandle<R>,
}

impl<R: Runtime> ModelDownloaderRuntime<LocalModel> for TauriModelRuntime<R> {
    fn models_base(&self) -> Result<PathBuf, hypr_model_downloader::Error> {
        use tauri_plugin_settings::SettingsPluginExt;
        Ok(self
            .app_handle
            .settings()
            .global_base()
            .map(|base| base.join("models").into_std_path_buf())
            .unwrap_or_else(|_| dirs::data_dir().unwrap_or_default().join("models")))
    }

    fn emit_progress(&self, model: &LocalModel, status: hypr_model_downloader::DownloadStatus) {
        if matches!(status, DownloadStatus::Completed) && stt_completion_triggers_diarizer(model) {
            maybe_start_diarizer_download(self.app_handle.clone());
        }

        let payload = DownloadProgressPayload {
            model: model.clone(),
            status,
        };
        let _ = payload.emit(&self.app_handle);
    }
}

pub fn create_model_downloader<R: Runtime>(
    app_handle: &tauri::AppHandle<R>,
) -> ModelDownloadManager<LocalModel> {
    let runtime = Arc::new(TauriModelRuntime {
        app_handle: app_handle.clone(),
    });
    ModelDownloadManager::new(runtime)
}

pub struct LocalStt<'a, R: Runtime, M: Manager<R>> {
    manager: &'a M,
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<'a, R: Runtime, M: Manager<R>> LocalStt<'a, R, M> {
    fn ensure_stt_model(model: &LocalModel) -> Result<(), crate::Error> {
        match model {
            LocalModel::Soniqo(_) | LocalModel::Whisper(_) | LocalModel::Diarizer(_) => {
                if model.is_available_on_current_platform() {
                    Ok(())
                } else {
                    Err(crate::Error::UnsupportedPlatform)
                }
            }
        }
    }

    pub fn models_dir(&self) -> PathBuf {
        use tauri_plugin_settings::SettingsPluginExt;
        self.manager
            .settings()
            .global_base()
            .map(|base| base.join("models").join("stt").into_std_path_buf())
            .unwrap_or_else(|_| {
                dirs::data_dir()
                    .unwrap_or_default()
                    .join("models")
                    .join("stt")
            })
    }

    pub async fn soniqo_model_dir(&self, model: &LocalModel) -> Result<PathBuf, crate::Error> {
        match model {
            LocalModel::Soniqo(model) => {
                let model = *model;
                run_soniqo_blocking(
                    move || hypr_transcribe_soniqo::model_cache_dir(model),
                    crate::Error::ServerStartFailed,
                )
                .await
            }
            _ => Err(crate::Error::UnsupportedModelType),
        }
    }

    pub async fn get_supervisor(&self) -> Result<supervisor::SupervisorRef, crate::Error> {
        let state = self.manager.state::<crate::SharedState>();
        let guard = state.lock().await;
        guard
            .stt_supervisor
            .clone()
            .ok_or(crate::Error::SupervisorNotFound)
    }

    pub async fn is_model_downloaded(&self, model: &LocalModel) -> Result<bool, crate::Error> {
        Self::ensure_stt_model(model)?;

        if let LocalModel::Soniqo(model) = model {
            return Ok(soniqo_download_state(*model).await?.status == "ready");
        }

        if matches!(model, LocalModel::Diarizer(_)) {
            return Ok(diarizer_download_state().await?.status == "ready");
        }

        let downloader = {
            let state = self.manager.state::<crate::SharedState>();
            let guard = state.lock().await;
            guard.model_downloader.clone()
        };
        Ok(downloader.is_downloaded(model).await?)
    }

    #[tracing::instrument(skip_all)]
    pub async fn start_server(&self, model: LocalModel) -> Result<String, crate::Error> {
        Self::ensure_stt_model(&model)?;

        if let LocalModel::Soniqo(soniqo_model) = model {
            if soniqo_download_state(soniqo_model).await?.status != "ready" {
                return Err(crate::Error::ModelNotDownloaded);
            }

            let supervisor = self.get_supervisor().await?;
            supervisor::stop_all_stt_servers(&supervisor)
                .await
                .map_err(|e| crate::Error::ServerStopFailed(e.to_string()))?;

            return Ok(hypr_transcribe_soniqo::LOCAL_BASE_URL.to_string());
        }

        let server_type = match &model {
            LocalModel::Whisper(_) => ServerType::Internal,
            LocalModel::Soniqo(_) | LocalModel::Diarizer(_) => {
                return Err(crate::Error::UnsupportedModelType);
            }
        };

        let current_info: Option<ServerInfo> = match server_type {
            #[cfg(feature = "whisper-cpp")]
            ServerType::Internal => internal_health().await,
            #[cfg(not(feature = "whisper-cpp"))]
            ServerType::Internal => None,
        };

        if let Some(info) = current_info.as_ref()
            && info.model.as_ref() == Some(&model)
        {
            if let Some(url) = info.url.clone() {
                return Ok(url);
            }

            return Err(crate::Error::ServerStartFailed(
                "missing_health_url".to_string(),
            ));
        }

        let supervisor = self.get_supervisor().await?;

        supervisor::stop_all_stt_servers(&supervisor)
            .await
            .map_err(|e| crate::Error::ServerStopFailed(e.to_string()))?;

        match server_type {
            ServerType::Internal => {
                #[cfg(feature = "whisper-cpp")]
                {
                    let cache_dir = self.models_dir();
                    let whisper_model = match model {
                        LocalModel::Whisper(m) => m,
                        _ => return Err(crate::Error::UnsupportedModelType),
                    };
                    start_internal_server(&supervisor, cache_dir, whisper_model).await
                }
                #[cfg(not(feature = "whisper-cpp"))]
                Err(crate::Error::UnsupportedModelType)
            }
        }
    }

    #[tracing::instrument(skip_all)]
    pub async fn stop_server(&self, server_type: Option<ServerType>) -> Result<bool, crate::Error> {
        let supervisor = self.get_supervisor().await?;

        match server_type {
            Some(t) => {
                supervisor::stop_stt_server(&supervisor, t)
                    .await
                    .map_err(|e| crate::Error::ServerStopFailed(e.to_string()))?;
                Ok(true)
            }
            None => {
                supervisor::stop_all_stt_servers(&supervisor)
                    .await
                    .map_err(|e| crate::Error::ServerStopFailed(e.to_string()))?;
                Ok(true)
            }
        }
    }

    #[tracing::instrument(skip_all)]
    pub async fn get_server_for_model(
        &self,
        model: &LocalModel,
    ) -> Result<Option<ServerInfo>, crate::Error> {
        Self::ensure_stt_model(model)?;

        if let LocalModel::Soniqo(soniqo_model) = model {
            let state = soniqo_download_state(*soniqo_model).await?;
            let downloaded = state.status == "ready";
            let downloading = state.status == "downloading";

            return Ok(Some(ServerInfo {
                url: downloaded.then(|| hypr_transcribe_soniqo::LOCAL_BASE_URL.to_string()),
                status: if downloaded {
                    ServerStatus::Ready
                } else if downloading {
                    ServerStatus::Loading
                } else {
                    ServerStatus::Unreachable
                },
                model: Some(model.clone()),
            }));
        }

        let server_type = match model {
            LocalModel::Whisper(_) => ServerType::Internal,
            LocalModel::Soniqo(_) | LocalModel::Diarizer(_) => {
                return Err(crate::Error::UnsupportedModelType);
            }
        };

        let info = match server_type {
            #[cfg(feature = "whisper-cpp")]
            ServerType::Internal => internal_health().await,
            #[cfg(not(feature = "whisper-cpp"))]
            ServerType::Internal => None,
        };

        Ok(info)
    }

    #[tracing::instrument(skip_all)]
    pub async fn get_servers(&self) -> Result<HashMap<ServerType, ServerInfo>, crate::Error> {
        #[cfg(feature = "whisper-cpp")]
        let internal_info = internal_health().await.unwrap_or(ServerInfo {
            url: None,
            status: ServerStatus::Unreachable,
            model: None,
        });
        #[cfg(not(feature = "whisper-cpp"))]
        let internal_info = ServerInfo {
            url: None,
            status: ServerStatus::Unreachable,
            model: None,
        };

        Ok([(ServerType::Internal, internal_info)]
            .into_iter()
            .collect())
    }

    #[tracing::instrument(skip_all)]
    pub async fn download_model(&self, model: LocalModel) -> Result<(), crate::Error> {
        Self::ensure_stt_model(&model)?;

        if let LocalModel::Soniqo(soniqo_model) = model.clone() {
            run_soniqo_blocking(
                move || hypr_transcribe_soniqo::start_model_download(soniqo_model),
                crate::Error::ServerStartFailed,
            )
            .await?;

            spawn_bridge_progress_poller(self.manager.app_handle().clone(), model, move || {
                hypr_transcribe_soniqo::model_download_state(soniqo_model)
            });
            return Ok(());
        }

        if matches!(model, LocalModel::Diarizer(_)) {
            run_soniqo_blocking(
                hypr_transcribe_soniqo::diarize::start_model_download,
                crate::Error::ServerStartFailed,
            )
            .await?;

            spawn_bridge_progress_poller(
                self.manager.app_handle().clone(),
                model,
                hypr_transcribe_soniqo::diarize::model_download_state,
            );
            return Ok(());
        }

        let downloader = {
            let state = self.manager.state::<crate::SharedState>();
            let guard = state.lock().await;
            guard.model_downloader.clone()
        };
        downloader.download(&model).await?;
        Ok(())
    }

    #[tracing::instrument(skip_all)]
    pub async fn cancel_download(&self, model: LocalModel) -> Result<bool, crate::Error> {
        Self::ensure_stt_model(&model)?;

        if matches!(model, LocalModel::Soniqo(_) | LocalModel::Diarizer(_)) {
            return Ok(false);
        }

        let downloader = {
            let state = self.manager.state::<crate::SharedState>();
            let guard = state.lock().await;
            guard.model_downloader.clone()
        };
        Ok(downloader.cancel_download(&model).await?)
    }

    #[tracing::instrument(skip_all)]
    pub async fn is_model_downloading(&self, model: &LocalModel) -> Result<bool, crate::Error> {
        Self::ensure_stt_model(model)?;

        if let LocalModel::Soniqo(model) = model {
            return Ok(soniqo_download_state(*model).await?.status == "downloading");
        }

        if matches!(model, LocalModel::Diarizer(_)) {
            return Ok(diarizer_download_state().await?.status == "downloading");
        }

        let downloader = {
            let state = self.manager.state::<crate::SharedState>();
            let guard = state.lock().await;
            guard.model_downloader.clone()
        };
        Ok(downloader.is_downloading(model).await)
    }

    #[tracing::instrument(skip_all)]
    pub async fn delete_model(&self, model: &LocalModel) -> Result<(), crate::Error> {
        Self::ensure_stt_model(model)?;

        if let LocalModel::Soniqo(model) = model {
            let model = *model;
            return run_soniqo_blocking(
                move || hypr_transcribe_soniqo::delete_model(model),
                crate::Error::ServerStopFailed,
            )
            .await;
        }

        if matches!(model, LocalModel::Diarizer(_)) {
            return Err(crate::Error::UnsupportedModelType);
        }

        let downloader = {
            let state = self.manager.state::<crate::SharedState>();
            let guard = state.lock().await;
            guard.model_downloader.clone()
        };
        downloader.delete(model).await?;
        Ok(())
    }
}

async fn run_soniqo_blocking<T>(
    task: impl FnOnce() -> hypr_transcribe_soniqo::Result<T> + Send + 'static,
    map_error: fn(String) -> crate::Error,
) -> Result<T, crate::Error>
where
    T: Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|e| map_error(e.to_string()))?
        .map_err(|e| map_error(e.to_string()))
}

async fn soniqo_download_state(
    model: hypr_transcribe_soniqo::SoniqoModel,
) -> Result<hypr_transcribe_soniqo::ModelDownloadState, crate::Error> {
    run_soniqo_blocking(
        move || hypr_transcribe_soniqo::model_download_state(model),
        crate::Error::ServerStartFailed,
    )
    .await
}

async fn diarizer_download_state()
-> Result<hypr_transcribe_soniqo::ModelDownloadState, crate::Error> {
    run_soniqo_blocking(
        hypr_transcribe_soniqo::diarize::model_download_state,
        crate::Error::ServerStartFailed,
    )
    .await
}

fn stt_completion_triggers_diarizer(model: &LocalModel) -> bool {
    model.model_kind() == LocalModelKind::Stt
}

fn diarizer_needs_download(state: &hypr_transcribe_soniqo::ModelDownloadState) -> bool {
    !matches!(state.status.as_str(), "ready" | "downloading")
}

// Diarization is always-on when its model is present: any downloaded STT model
// pulls the diarizer in, but a missing/failed diarizer never blocks STT.
pub(crate) fn maybe_start_diarizer_download<R: Runtime>(app_handle: tauri::AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        match diarizer_download_state().await {
            Ok(state) if diarizer_needs_download(&state) => {}
            Ok(_) => return,
            Err(error) => {
                tracing::warn!("diarizer_state_check_failed: {error}");
                return;
            }
        }

        if let Err(error) = run_soniqo_blocking(
            hypr_transcribe_soniqo::diarize::start_model_download,
            crate::Error::ServerStartFailed,
        )
        .await
        {
            tracing::warn!("diarizer_download_start_failed: {error}");
            return;
        }

        spawn_bridge_progress_poller(
            app_handle,
            LocalModel::Diarizer(DiarizerModel::FluidCommunity),
            hypr_transcribe_soniqo::diarize::model_download_state,
        );
    });
}

pub(crate) fn backfill_diarizer_download<R: Runtime>(app_handle: tauri::AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        for model in LocalModel::all() {
            if model.model_kind() != LocalModelKind::Stt
                || !model.is_available_on_current_platform()
            {
                continue;
            }

            if app_handle
                .local_stt()
                .is_model_downloaded(&model)
                .await
                .unwrap_or(false)
            {
                maybe_start_diarizer_download(app_handle.clone());
                return;
            }
        }
    });
}

fn spawn_bridge_progress_poller<R: Runtime, F>(
    app_handle: tauri::AppHandle<R>,
    model: LocalModel,
    fetch_state: F,
) where
    F: Fn() -> hypr_transcribe_soniqo::Result<hypr_transcribe_soniqo::ModelDownloadState>
        + Clone
        + Send
        + Sync
        + 'static,
{
    tokio::spawn(async move {
        for _ in 0..7200 {
            let fetch_once = fetch_state.clone();
            let status = tokio::task::spawn_blocking(move || fetch_once()).await;

            let download_status = match status {
                Ok(Ok(state)) => match state.status.as_str() {
                    "ready" => DownloadStatus::Completed,
                    "error" => DownloadStatus::Failed(
                        state
                            .error
                            .unwrap_or_else(|| "Model download failed".to_string()),
                    ),
                    _ => DownloadStatus::Downloading(state.progress_percent.unwrap_or(0)),
                },
                Ok(Err(error)) => DownloadStatus::Failed(error.to_string()),
                Err(error) => DownloadStatus::Failed(error.to_string()),
            };

            let should_stop = matches!(
                download_status,
                DownloadStatus::Completed | DownloadStatus::Failed(_)
            );

            if matches!(download_status, DownloadStatus::Completed)
                && stt_completion_triggers_diarizer(&model)
            {
                maybe_start_diarizer_download(app_handle.clone());
            }

            let _ = DownloadProgressPayload {
                model: model.clone(),
                status: download_status,
            }
            .emit(&app_handle);

            if should_stop {
                return;
            }

            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        }

        let _ = DownloadProgressPayload {
            model,
            status: DownloadStatus::Failed("Model download timed out".to_string()),
        }
        .emit(&app_handle);
    });
}

pub trait LocalSttPluginExt<R: Runtime> {
    fn local_stt(&self) -> LocalStt<'_, R, Self>
    where
        Self: Manager<R> + Sized;
}

impl<R: Runtime, T: Manager<R>> LocalSttPluginExt<R> for T {
    fn local_stt(&self) -> LocalStt<'_, R, Self>
    where
        Self: Sized,
    {
        LocalStt {
            manager: self,
            _runtime: std::marker::PhantomData,
        }
    }
}

#[cfg(feature = "whisper-cpp")]
async fn start_internal_server(
    supervisor: &supervisor::SupervisorRef,
    cache_dir: PathBuf,
    model: hypr_whisper_local_model::WhisperModel,
) -> Result<String, crate::Error> {
    supervisor::start_internal_stt(
        supervisor,
        internal::InternalSTTArgs {
            model_cache_dir: cache_dir,
            model_type: model,
        },
    )
    .await
    .map_err(|e| crate::Error::ServerStartFailed(e.to_string()))?;

    internal_health()
        .await
        .and_then(|info| info.url)
        .ok_or_else(|| crate::Error::ServerStartFailed("empty_health".to_string()))
}

#[cfg(feature = "whisper-cpp")]
async fn internal_health() -> Option<ServerInfo> {
    match registry::where_is(internal::InternalSTTActor::name()) {
        Some(cell) => {
            let actor: ActorRef<internal::InternalSTTMessage> = cell.into();
            call_t!(actor, internal::InternalSTTMessage::GetHealth, 10 * 1000).ok()
        }
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SoniqoModel;

    fn state_with_status(status: &str) -> hypr_transcribe_soniqo::ModelDownloadState {
        hypr_transcribe_soniqo::ModelDownloadState {
            status: status.to_string(),
            current_file: None,
            progress_percent: None,
            local_path: String::new(),
            error: None,
        }
    }

    // No SharedState is managed on the mock app: reaching the supervisor or
    // downloader machinery would panic, so an early error proves the diarizer
    // can never start or block an STT server.
    #[tokio::test]
    async fn diarizer_never_starts_or_blocks_stt_server() {
        let app = tauri::test::mock_app();

        let result = app
            .local_stt()
            .start_server(LocalModel::Diarizer(DiarizerModel::FluidCommunity))
            .await;

        assert!(matches!(
            result,
            Err(crate::Error::UnsupportedModelType) | Err(crate::Error::UnsupportedPlatform)
        ));
    }

    #[test]
    fn only_stt_completions_trigger_diarizer_download() {
        assert!(stt_completion_triggers_diarizer(&LocalModel::Whisper(
            hypr_whisper_local_model::WhisperModel::QuantizedTiny
        )));
        assert!(stt_completion_triggers_diarizer(&LocalModel::Soniqo(
            SoniqoModel::ParakeetStreaming
        )));
        assert!(!stt_completion_triggers_diarizer(&LocalModel::Diarizer(
            DiarizerModel::FluidCommunity
        )));
    }

    #[test]
    fn diarizer_download_skipped_when_ready_or_downloading() {
        assert!(diarizer_needs_download(&state_with_status("idle")));
        assert!(diarizer_needs_download(&state_with_status("error")));
        assert!(!diarizer_needs_download(&state_with_status("downloading")));
        assert!(!diarizer_needs_download(&state_with_status("ready")));
    }
}
