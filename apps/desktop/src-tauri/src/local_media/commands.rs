use super::*;
use crate::attachment_upload::{DeviceAttachmentSource, DeviceUploadIo};
use hypr_loofah_core::manual_media_intents::ManualMediaSummaryStatus;
use tauri::{AppHandle, Manager, Runtime, Window};
fn service<R: Runtime>(app: &AppHandle<R>) -> Result<Arc<Service>, ApplicationError> {
    let context = crate::application::context(app)?;
    app.try_state::<LocalMediaState>()
        .ok_or_else(unavailable)?
        .service_for(&context)
}
#[tauri::command]
#[specta::specta]
pub fn application_local_media_available<R: Runtime>(app: AppHandle<R>) -> bool {
    service(&app).is_ok()
}
#[tauri::command]
#[specta::specta]
pub async fn application_local_media_source<R: Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<LocalMediaSourceSnapshot, ApplicationError> {
    service(&app)?.cli.media_source(&session_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn application_local_media_import_audio<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    request_id: String,
    session_id: Option<String>,
    input: UploadAttachmentRequest,
    source: DeviceAttachmentSource,
    created_at: Option<String>,
    content_type: Option<String>,
) -> Result<LocalMediaStatus, ApplicationError> {
    validate_request(&request_id)?;
    let service = service(&app)?;
    let metadata = match &session_id {
        Some(id) => Some(service.context.read_metadata(id).await?),
        None => None,
    };
    let reader = app
        .try_state::<DeviceUploadIo>()
        .ok_or_else(unavailable)?
        .reader(window.label(), source.clone(), &input)?;
    let extension =
        hypr_fs_sync_core::audio::import_source_extension(&input.filename, content_type.as_deref());
    let stem = std::path::Path::new(&input.filename)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or("imported-audio");
    let filename = format!("{stem}.{extension}");
    let temporary = app
        .path()
        .app_cache_dir()
        .map_err(|_| unavailable())?
        .join("local-media-inputs");
    let (artifact, date) = super::source::retain(
        service.context.clone(),
        reader,
        input,
        temporary,
        source,
        created_at,
    )
    .await?;
    let operation = match metadata {
        Some(snapshot) => LocalCliOperation::ImportIntoChecked {
            session_id: snapshot.meta.id,
            filename,
            transcribe: true,
            expected_metadata_revision: snapshot.revision,
            created_at: date,
        },
        None => LocalCliOperation::Import {
            filename,
            title: None,
            into: None,
            transcribe: true,
            metadata: LocalInitialSessionMetadata {
                created_at: date,
                ..Default::default()
            },
        },
    };
    service.submit(request_id, operation, Some(artifact)).await
}
#[tauri::command]
#[specta::specta]
pub async fn application_local_media_transcribe<R: Runtime>(
    app: AppHandle<R>,
    request_id: String,
    expected: LocalMediaSourceSnapshot,
) -> Result<LocalMediaStatus, ApplicationError> {
    service(&app)?
        .submit(
            request_id,
            LocalCliOperation::TranscribeChecked {
                session_id: expected.session_id.clone(),
                expected,
            },
            None,
        )
        .await
}
#[tauri::command]
#[specta::specta]
pub async fn application_local_media_import_subtitles<R: Runtime>(
    app: AppHandle<R>,
    window: Window<R>,
    request_id: String,
    session_id: String,
    input: UploadAttachmentRequest,
    source: DeviceAttachmentSource,
) -> Result<LocalMediaStatus, ApplicationError> {
    validate_request(&request_id)?;
    validate_session_id(&session_id).map_err(|_| invalid("Invalid media session"))?;
    if input.size_bytes > 32 * 1024 * 1024 {
        return Err(invalid("Subtitle source exceeds its import budget"));
    }
    let service = service(&app)?;
    let reader = app
        .try_state::<DeviceUploadIo>()
        .ok_or_else(unavailable)?
        .reader(window.label(), source.clone(), &input)?;
    let filename = input.filename.clone();
    let temporary = app
        .path()
        .app_cache_dir()
        .map_err(|_| unavailable())?
        .join("local-media-inputs");
    let (artifact, _) = super::source::retain(
        service.context.clone(),
        reader,
        input,
        temporary,
        source,
        None,
    )
    .await?;
    service
        .submit(
            request_id,
            LocalCliOperation::ImportSubtitles {
                session_id,
                filename,
            },
            Some(artifact),
        )
        .await
}
#[tauri::command]
#[specta::specta]
pub async fn application_local_media_list<R: Runtime>(
    app: AppHandle<R>,
    session_id: Option<String>,
) -> Result<Vec<LocalMediaStatus>, ApplicationError> {
    service(&app)?.list(session_id.as_deref()).await
}
#[tauri::command]
#[specta::specta]
pub async fn application_local_media_status<R: Runtime>(
    app: AppHandle<R>,
    request_id: String,
) -> Result<LocalMediaStatus, ApplicationError> {
    service(&app)?.status(&request_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn application_local_media_cancel<R: Runtime>(
    app: AppHandle<R>,
    request_id: String,
) -> Result<LocalMediaStatus, ApplicationError> {
    service(&app)?.cancel(&request_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn application_local_media_recover<R: Runtime>(
    app: AppHandle<R>,
    request_id: String,
) -> Result<LocalMediaStatus, ApplicationError> {
    service(&app)?.recover(request_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn application_local_media_summary_status<R: Runtime>(
    app: AppHandle<R>,
    request_id: String,
) -> Result<ManualMediaSummaryStatus, ApplicationError> {
    service(&app)?
        .context
        .manual_media_summary_status(&request_id)
        .await
}
