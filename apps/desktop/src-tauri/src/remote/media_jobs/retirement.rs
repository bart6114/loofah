use super::*;

#[derive(Clone, Debug, serde::Serialize, specta::Type)]
pub struct MediaRetirementView {
    pub status: MediaRetirementStatus,
    pub pending: Option<MediaRetirementRequest>,
    pub verified_export: bool,
}

async fn status(
    receipts: &Arc<MediaSubmissions>,
    authority: &MediaAuthority,
    request: &str,
) -> Result<MediaRetirementView, ApplicationError> {
    let submission = original(receipts, authority, request).await?;
    let current = verify_status(&submission, authority.status(request).await?)?;
    let submission = receipts
        .observe_admission(
            &submission,
            &MediaAdmission {
                declaration: current.declaration.clone(),
                revision: current.revision.clone(),
                status: current,
            },
        )
        .await?;
    let status = authority.retirement_status(&submission).await?;
    if submission.retirement.is_some() && status.retirement.is_some() {
        receipts.observe_retirement(&submission, &status).await?;
    }
    Ok(MediaRetirementView {
        pending: submission
            .retirement
            .as_ref()
            .map(|intent| intent.request.clone()),
        verified_export: submission.has_verified_source_export(),
        status,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn remote_media_retirement_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    vault_id: String,
    request_id: String,
    control_id: String,
) -> Result<MediaRetirementView, ApplicationError> {
    let state = app.state::<RemoteState>();
    let media = app.state::<MediaJobState>();
    let (client, cancel, pending) = state.admit(&connection_id, window.label(), &control_id)?;
    client.media_retirement_capability(&cancel)?;
    let authority = media
        .authority(
            &state,
            &connection_id,
            window.label(),
            &vault_id,
            Some(&request_id),
        )
        .await?
        .for_request(cancel.clone());
    let _cancel_on_drop = cancel.clone().drop_guard();
    let receipts = media.receipts(&app).await?;
    tokio::spawn(async move {
        let _pending = pending;
        status(&receipts, &authority, &request_id).await
    })
    .await
    .map_err(worker_error)?
}

#[tauri::command]
#[specta::specta]
pub async fn remote_media_retire<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    vault_id: String,
    request_id: String,
    input: MediaRetirementRequest,
) -> Result<MediaRetirementView, ApplicationError> {
    validate_media_retirement_request(&input).map_err(invalid)?;
    let state = app.state::<RemoteState>();
    let media = app.state::<MediaJobState>();
    let (client, cancel, pending) =
        state.admit(&connection_id, window.label(), &input.retirement_id)?;
    client.media_retirement_capability(&cancel)?;
    let authority = media
        .authority(
            &state,
            &connection_id,
            window.label(),
            &vault_id,
            Some(&request_id),
        )
        .await?
        .for_request(cancel.clone());
    let _cancel_on_drop = cancel.clone().drop_guard();
    let receipts = media.receipts(&app).await?;
    tokio::spawn(async move {
        let _pending = pending;
        let submission = original(&receipts, &authority, &request_id).await?;
        authority.check_current().await?;
        let submission = receipts.prepare_retirement(&submission, input).await?;
        let status = authority.retire(&submission).await?;
        let submission = receipts.observe_retirement(&submission, &status).await?;
        Ok(MediaRetirementView {
            pending: submission
                .retirement
                .as_ref()
                .map(|intent| intent.request.clone()),
            verified_export: submission.has_verified_source_export(),
            status,
        })
    })
    .await
    .map_err(worker_error)?
}

fn worker_error(_: tokio::task::JoinError) -> ApplicationError {
    failure(
        ErrorCode::Internal,
        "Original media retirement observer stopped",
    )
}
