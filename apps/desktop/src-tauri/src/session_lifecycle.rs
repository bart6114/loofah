use hypr_loofah_protocol::{
    ApplicationError, DeleteSessionOutcome, DeleteSessionRequest, RestoreSessionOutcome,
    RestoreSessionRequest, SessionLifecycleSnapshot,
};

#[tauri::command]
#[specta::specta]
pub fn application_finish_local_session_work<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    request_id: String,
) -> Result<(), ApplicationError> {
    use tauri::Manager;
    app.try_state::<hypr_loofah_core::local_session_work::LocalSessionWorkRegistry>()
        .ok_or_else(|| {
            hypr_loofah_client::failure(
                hypr_loofah_protocol::ErrorCode::Unavailable,
                "Local session work is unavailable",
            )
        })?
        .persistence_finished(&session_id, &request_id)
}

#[tauri::command]
#[specta::specta]
pub async fn application_session_lifecycle<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<SessionLifecycleSnapshot, ApplicationError> {
    crate::application::context(&app)?
        .read_session_lifecycle(&session_id)
        .await
}
#[tauri::command]
#[specta::specta]
pub async fn application_delete_session<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request_id: String,
    input: DeleteSessionRequest,
) -> Result<DeleteSessionOutcome, ApplicationError> {
    crate::application::context(&app)?
        .delete_session("desktop".into(), request_id, input)
        .await
}
#[tauri::command]
#[specta::specta]
pub async fn application_restore_session<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request_id: String,
    input: RestoreSessionRequest,
) -> Result<RestoreSessionOutcome, ApplicationError> {
    crate::application::context(&app)?
        .restore_session("desktop".into(), request_id, input)
        .await
}
