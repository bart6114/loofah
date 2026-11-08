use hypr_loofah_protocol::*;
use tauri::{AppHandle, Manager};
use tauri_plugin_local_stt::{LocalModel, LocalSttPluginExt};

use crate::{application::context, local_transcription::PRINCIPAL};

#[tauri::command]
#[specta::specta]
pub fn application_transcription_available<R: tauri::Runtime>(app: AppHandle<R>) -> bool {
    context(&app).is_ok_and(|context| context.transcription_available())
}

#[tauri::command]
#[specta::specta]
pub async fn application_transcription_policy<R: tauri::Runtime>(
    app: AppHandle<R>,
) -> Result<TranscriptionPolicy, ApplicationError> {
    let context = context(&app)?;
    let settings = context.settings.read_current().await.map_err(|_| {
        hypr_loofah_client::failure(
            ErrorCode::Unavailable,
            "Local transcription settings are unavailable",
        )
    })?;
    if settings.current_stt_provider.as_deref() != Some("fmtr") {
        return Ok(TranscriptionPolicy::default());
    }
    let Some(alias) = settings.current_stt_model.filter(|value| !value.is_empty()) else {
        return Ok(TranscriptionPolicy::default());
    };
    let parsed =
        serde_json::from_value::<LocalModel>(serde_json::Value::String(alias.clone())).ok();
    let parsed = parsed
        .and_then(|model| match model {
            LocalModel::Soniqo(model) => Some(LocalModel::Soniqo(model.batch_model())),
            LocalModel::Whisper(model) => Some(LocalModel::Whisper(model)),
            _ => None,
        })
        .filter(|model| model.is_available_on_current_platform());
    let downloaded = if let Some(model) = parsed {
        if app
            .try_state::<tauri_plugin_local_stt::SharedState>()
            .is_some()
        {
            app.local_stt()
                .is_model_downloaded(&model)
                .await
                .unwrap_or(false)
        } else {
            false
        }
    } else {
        false
    };
    Ok(TranscriptionPolicy {
        model: Some(alias),
        availability: if !context.transcription_available() {
            TranscriptionAvailability::Stopping
        } else if downloaded {
            TranscriptionAvailability::Ready
        } else {
            TranscriptionAvailability::Unavailable
        },
        batch_enabled: context.transcription_available(),
        live_enabled: false,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn application_submit_transcription<R: tauri::Runtime>(
    app: AppHandle<R>,
    input: TranscriptionJobRequest,
    request_id: String,
) -> Result<TranscriptionJobSnapshot, ApplicationError> {
    context(&app)?
        .submit_transcription_job(PRINCIPAL.into(), request_id, input)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_transcription_jobs<R: tauri::Runtime>(
    app: AppHandle<R>,
    input: ListTranscriptionJobsInput,
) -> Result<TranscriptionJobPage, ApplicationError> {
    context(&app)?
        .list_transcription_jobs(PRINCIPAL.into(), input)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_transcription_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    job_id: String,
) -> Result<TranscriptionJobSnapshot, ApplicationError> {
    context(&app)?
        .transcription_job(PRINCIPAL.into(), job_id)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_transcription_job_result<R: tauri::Runtime>(
    app: AppHandle<R>,
    job_id: String,
) -> Result<Option<TranscriptionJobResult>, ApplicationError> {
    context(&app)?
        .transcription_job_result(PRINCIPAL.into(), job_id)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_cancel_transcription_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    job_id: String,
) -> Result<TranscriptionJobSnapshot, ApplicationError> {
    context(&app)?
        .cancel_transcription_job(PRINCIPAL.into(), job_id)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_resume_transcription_result<R: tauri::Runtime>(
    app: AppHandle<R>,
    job_id: String,
) -> Result<TranscriptionJobSnapshot, ApplicationError> {
    context(&app)?
        .resume_transcription_result(PRINCIPAL.into(), job_id)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn remote_or_uninitialized_host_never_creates_a_local_transcription_service() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        assert!(!application_transcription_available(app.handle().clone()));
        assert_eq!(
            application_transcription_policy(app.handle().clone())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
        assert!(
            app.try_state::<std::sync::Arc<hypr_loofah_core::VaultContext>>()
                .is_none()
        );
    }
}
