use super::*;
use crate::application_export::{SessionExportResult, files};

#[tauri::command]
#[specta::specta]
pub async fn remote_media_export_source<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    vault_id: String,
    request_id: String,
) -> Result<SessionExportResult, ApplicationError> {
    uuid::Uuid::parse_str(&request_id).map_err(|_| invalid("Invalid media request UUID"))?;
    let state = app.state::<RemoteState>();
    let media = app.state::<MediaJobState>();
    let owner = window.label().to_owned();
    let (client, cancel, pending) = state.admit(&connection_id, &owner, &request_id)?;
    client.media_source_export_capability(&cancel)?;
    let authority = media
        .authority(&state, &connection_id, &owner, &vault_id, Some(&request_id))
        .await?
        .for_request(cancel.clone());
    let _cancel_on_drop = cancel.clone().drop_guard();
    let receipts = media.receipts(&app).await?;
    let root = crate::remote::attachments::download_root(&app)?;
    let registry = state.registry.clone();
    tokio::spawn(async move {
        let _pending = pending;
        let submission = original(&receipts, &authority, &request_id).await?;
        let status = verify_status(&submission, authority.status(&request_id).await?)?;
        let submission = receipts
            .observe_admission(
                &submission,
                &MediaAdmission {
                    declaration: status.declaration.clone(),
                    revision: status.revision.clone(),
                    status,
                },
            )
            .await?;
        let filename = submission
            .declaration
            .operation
            .upload()
            .ok_or_else(|| failure(ErrorCode::NotFound, "This request has no uploaded source"))?
            .0
            .to_owned();
        let (stage, output) =
            tokio::task::spawn_blocking(move || files::stage_download(root, &filename))
                .await
                .map_err(worker_error)??;
        let (output, stage) = authority
            .download_source_with_guard(&submission, tokio::fs::File::from_std(output), stage)
            .await?;
        output.sync_all().await.map_err(copy_error)?;
        drop(output);
        let stage = tokio::task::spawn_blocking(move || {
            stage.mark_download();
            stage
        })
        .await
        .map_err(worker_error)?;
        let guard = authority.guard_source_export(&submission).await?;
        // Only the completed private copy enters this short registry/profile/login fence.
        tokio::task::spawn_blocking(move || {
            let _owners = (authority, receipts, _pending);
            let registry = registry.lock().unwrap();
            let connected = registry
                .get(&connection_id)
                .filter(|connected| {
                    connected.authenticated
                        && connected.login.is_none()
                        && connected.leases.contains_key(&owner)
                        && Arc::ptr_eq(&connected.client, &client)
                })
                .ok_or_else(missing)?;
            let _connected = connected;
            if cancel.is_cancelled() {
                return Err(failure(
                    ErrorCode::Cancelled,
                    "Original media source export cancelled",
                ));
            }
            guard.check_current()?;
            let path = files::publish(stage)?;
            drop(registry);
            Ok(SessionExportResult {
                path: path.to_string_lossy().into_owned(),
            })
        })
        .await
        .map_err(worker_error)?
    })
    .await
    .map_err(worker_error)?
}
fn worker_error(_: tokio::task::JoinError) -> ApplicationError {
    failure(
        ErrorCode::Internal,
        "Original media source export worker stopped",
    )
}
fn copy_error(cause: std::io::Error) -> ApplicationError {
    tracing::error!(%cause,"Original media source copy could not be synced");
    failure(
        ErrorCode::Internal,
        "Could not finish saving the original media source",
    )
}
