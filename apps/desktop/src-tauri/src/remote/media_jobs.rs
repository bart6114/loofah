use super::*;
mod source_export;
use crate::attachment_upload::{DeviceAttachmentSource, DeviceUploadIo, UploadReader};
use hypr_loofah_client::media_jobs::{
    MediaAuthority, MediaScope, MediaSubmission, MediaSubmissions,
};
use hypr_loofah_protocol::*;
pub use source_export::remote_media_export_source;
use std::sync::Weak;

#[derive(Default)]
pub struct MediaJobState {
    store: tokio::sync::OnceCell<Arc<MediaSubmissions>>,
    authorities: Mutex<HashMap<(String, String), Arc<OriginalAuthority>>>,
}
struct OriginalAuthority {
    scope: MediaScope,
    owner: String,
    client: Weak<RemoteClient>,
    bound: tokio::sync::OnceCell<Arc<MediaAuthority>>,
    lifetime: CancellationToken,
}
#[derive(Clone, Debug, serde::Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MediaQuery {
    Source { session_id: String },
    List { session_id: Option<String> },
    Status { request_id: String },
    Result { request_id: String },
    SummaryStatus { request_id: String },
}
#[derive(serde::Serialize, specta::Type)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum MediaReply {
    Source(MediaSessionSnapshot),
    List(Vec<MediaRequestStatus>),
    Status(MediaRequestStatus),
    Result(Option<TranscriptWithData>),
    SummaryStatus(MediaSummaryStatus),
}
impl MediaJobState {
    async fn receipts<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
    ) -> Result<Arc<MediaSubmissions>, ApplicationError> {
        self.store
            .get_or_try_init(|| async {
                let root = app
                    .path()
                    .app_local_data_dir()
                    .map_err(|_| unavailable())?
                    .join("media-submissions");
                MediaSubmissions::new(root)
            })
            .await
            .cloned()
    }
    async fn authority(
        &self,
        state: &RemoteState,
        connection: &str,
        owner: &str,
        vault: &str,
        request: Option<&str>,
    ) -> Result<Arc<MediaAuthority>, ApplicationError> {
        let (client, profile) = {
            let registry = state.registry.lock().unwrap();
            let connection = registry
                .get(connection)
                .filter(|c| c.leases.contains_key(owner) && c.authenticated && c.login.is_none())
                .ok_or_else(missing)?;
            let (_, profile) = connection.profile.as_ref().ok_or_else(|| {
                failure(
                    ErrorCode::Validation,
                    "Media requests require a saved desktop profile",
                )
            })?;
            (connection.client.clone(), profile.clone())
        };
        client.media_jobs_capability(&state.window_lifetime(owner))?;
        let current = profile.profile().await;
        if current.vault_id.as_deref() != Some(vault) {
            return Err(failure(
                ErrorCode::Conflict,
                "The original media vault changed",
            ));
        }
        let scope = MediaScope {
            profile_id: current.id,
            server: current.server,
            member_id: current.member_id.ok_or_else(missing)?,
            vault_id: vault.into(),
        };
        let lifetime = state.window_lifetime(owner);
        let Some(request) = request else {
            return client.bind_media(profile, scope, lifetime).await;
        };
        uuid::Uuid::parse_str(request).map_err(|_| invalid("Invalid media request UUID"))?;
        let original = {
            let mut values = self.authorities.lock().unwrap();
            let key = (scope.profile_id.clone(), request.into());
            if let Some(original) = values.get(&key) {
                if original.scope != scope
                    || original.owner != owner
                    || !Weak::ptr_eq(&original.client, &Arc::downgrade(&client))
                {
                    return Err(failure(
                        ErrorCode::Conflict,
                        "Original media connection changed; keep its receipt for review",
                    ));
                }
                original.clone()
            } else {
                if values.len() >= 4096 {
                    return Err(failure(
                        ErrorCode::ResourceLimit,
                        "Too many original media authorities; retain saved requests and restart",
                    ));
                }
                let original = Arc::new(OriginalAuthority {
                    scope: scope.clone(),
                    owner: owner.into(),
                    client: Arc::downgrade(&client),
                    bound: Default::default(),
                    lifetime,
                });
                values.insert(key, original.clone());
                original
            }
        };
        original
            .bound
            .get_or_try_init(|| client.bind_media(profile, scope, original.lifetime.clone()))
            .await
            .cloned()
    }
}
#[tauri::command]
#[specta::specta]
pub async fn remote_media_request<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    vault_id: String,
    request_id: String,
    query: MediaQuery,
) -> Result<MediaReply, ApplicationError> {
    let state = app.state::<RemoteState>();
    let media = app.state::<MediaJobState>();
    let (_, cancel, _pending) = state.admit(&connection_id, window.label(), &request_id)?;
    let authority = media
        .authority(&state, &connection_id, window.label(), &vault_id, None)
        .await?
        .for_request(cancel);
    match query {
        MediaQuery::Source { session_id } => {
            Ok(MediaReply::Source(authority.source(&session_id).await?))
        }
        MediaQuery::List { session_id } => Ok(MediaReply::List(
            authority.list(session_id.as_deref()).await?,
        )),
        MediaQuery::Status { request_id } => {
            Ok(MediaReply::Status(authority.status(&request_id).await?))
        }
        MediaQuery::Result { request_id } => {
            Ok(MediaReply::Result(authority.result(&request_id).await?))
        }
        MediaQuery::SummaryStatus { request_id } => Ok(MediaReply::SummaryStatus(
            authority.summary_status(&request_id).await?,
        )),
    }
}
#[tauri::command]
#[specta::specta]
pub async fn remote_media_submit<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    vault_id: String,
    declaration: MediaDeclaration,
    source: Option<DeviceAttachmentSource>,
) -> Result<MediaRequestStatus, ApplicationError> {
    validate_media_declaration(&declaration).map_err(invalid)?;
    let state = app.state::<RemoteState>();
    let media = app.state::<MediaJobState>();
    let (client, cancel, _pending) =
        state.admit(&connection_id, window.label(), &declaration.request_id)?;
    if matches!(
        declaration.operation,
        MediaOperation::ImportAudio {
            initial: Some(_),
            ..
        }
    ) {
        client.media_initial_metadata_capability(&cancel)?;
    }
    let authority = media
        .authority(
            &state,
            &connection_id,
            window.label(),
            &vault_id,
            Some(&declaration.request_id),
        )
        .await?
        .for_request(cancel);
    let receipts = media.receipts(&app).await?;
    let (submission, first) = receipts
        .prepare(authority.scope().clone(), declaration)
        .await?;
    if !first {
        // An ordinary renderer retry cannot repeat an unknown source upload or paid dispatch.
        let status = authority.status(&submission.declaration.request_id).await.map_err(|mut cause| {
            if cause.code == ErrorCode::NotFound {
                cause.code = ErrorCode::Unavailable;
                cause.message = "The original device declaration was saved; explicitly recover that request before selecting source again".into();
                cause.mutation_outcome = Some(MutationOutcome::Unknown);
                cause.retryable = false;
            }
            cause
        })?;
        return verify_status(&submission, status);
    }
    let reader = reader(&app, window.label(), &submission, source)?;
    let admission = authority.declare(&submission).await?;
    let submission = receipts.observe_admission(&submission, &admission).await?;
    if admission.status.phase == MediaPhase::Receiving {
        let reader = reader.ok_or_else(|| invalid("Original media source is required"))?;
        authority
            .upload_source(&submission, &admission, reader)
            .await
    } else {
        Ok(admission.status)
    }
}
#[tauri::command]
#[specta::specta]
pub async fn remote_media_recover<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    vault_id: String,
    request_id: String,
) -> Result<MediaRequestStatus, ApplicationError> {
    let state = app.state::<RemoteState>();
    let media = app.state::<MediaJobState>();
    let (_, cancel, _pending) = state.admit(&connection_id, window.label(), &request_id)?;
    let authority = media
        .authority(
            &state,
            &connection_id,
            window.label(),
            &vault_id,
            Some(&request_id),
        )
        .await?
        .for_request(cancel);
    let receipts = media.receipts(&app).await?;
    let submission = original(&receipts, &authority, &request_id).await?;
    let status = authority.recover(&submission).await?;
    let admission = MediaAdmission {
        declaration: status.declaration.clone(),
        revision: status.revision.clone(),
        status: status.clone(),
    };
    receipts.observe_admission(&submission, &admission).await?;
    Ok(status)
}
#[tauri::command]
#[specta::specta]
pub async fn remote_media_cancel<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    vault_id: String,
    request_id: String,
) -> Result<MediaRequestStatus, ApplicationError> {
    let state = app.state::<RemoteState>();
    let media = app.state::<MediaJobState>();
    {
        let registry = state.registry.lock().unwrap();
        if let Some(connection) = registry
            .get(&connection_id)
            .filter(|connection| connection.leases.contains_key(window.label()))
        {
            if let Some(active) = connection
                .requests
                .get(&(window.label().to_owned(), request_id.clone()))
            {
                active.cancel();
            }
        }
    }
    let control_id = uuid::Uuid::new_v4().to_string();
    let (_, cancel, _pending) = state.admit(&connection_id, window.label(), &control_id)?;
    let authority = media
        .authority(
            &state,
            &connection_id,
            window.label(),
            &vault_id,
            Some(&request_id),
        )
        .await?
        .for_request(cancel);
    authority.cancel(&request_id).await
}
#[tauri::command]
#[specta::specta]
pub async fn remote_media_resume_source<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    vault_id: String,
    request_id: String,
    source: DeviceAttachmentSource,
) -> Result<MediaRequestStatus, ApplicationError> {
    let state = app.state::<RemoteState>();
    let media = app.state::<MediaJobState>();
    let (_, cancel, _pending) = state.admit(&connection_id, window.label(), &request_id)?;
    let authority = media
        .authority(
            &state,
            &connection_id,
            window.label(),
            &vault_id,
            Some(&request_id),
        )
        .await?
        .for_request(cancel);
    let receipts = media.receipts(&app).await?;
    let submission = original(&receipts, &authority, &request_id).await?;
    let status = verify_status(&submission, authority.status(&request_id).await?)?;
    let admission = MediaAdmission {
        declaration: status.declaration.clone(),
        revision: status.revision.clone(),
        status,
    };
    let submission = receipts.observe_admission(&submission, &admission).await?;
    let reader = reader(&app, window.label(), &submission, Some(source))?
        .ok_or_else(|| invalid("Original source required"))?;
    authority
        .upload_source(&submission, &admission, reader)
        .await
}
#[tauri::command]
#[specta::specta]
pub async fn remote_media_submissions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    vault_id: String,
) -> Result<Vec<MediaSubmission>, ApplicationError> {
    let state = app.state::<RemoteState>();
    let media = app.state::<MediaJobState>();
    let authority = media
        .authority(&state, &connection_id, window.label(), &vault_id, None)
        .await?;
    media
        .receipts(&app)
        .await?
        .list(authority.scope().clone())
        .await
}
async fn original(
    receipts: &Arc<MediaSubmissions>,
    authority: &MediaAuthority,
    request: &str,
) -> Result<MediaSubmission, ApplicationError> {
    receipts
        .load(authority.scope().clone(), request.into())
        .await?
        .ok_or_else(|| {
            failure(
                ErrorCode::NotFound,
                "Original device media declaration is unavailable",
            )
        })
}
fn verify_status(
    submission: &MediaSubmission,
    status: MediaRequestStatus,
) -> Result<MediaRequestStatus, ApplicationError> {
    if status.declaration != submission.declaration
        || submission
            .admission_revision
            .as_ref()
            .is_some_and(|revision| revision != &status.revision)
    {
        return Err(failure(
            ErrorCode::Conflict,
            "Original media declaration or admission changed",
        ));
    }
    Ok(status)
}
fn reader<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    owner: &str,
    submission: &MediaSubmission,
    source: Option<DeviceAttachmentSource>,
) -> Result<Option<UploadReader>, ApplicationError> {
    match (submission.declaration.operation.upload(), source) {
        (None, None) => Ok(None),
        (Some((filename, source)), Some(device)) => {
            let input = UploadAttachmentRequest {
                filename: filename.into(),
                size_bytes: source.size_bytes,
                sha256: source.sha256.clone(),
            };
            Ok(Some(
                app.state::<DeviceUploadIo>()
                    .reader(owner, device, &input)?,
            ))
        }
        _ => Err(invalid(
            "Selected media source does not match its original declaration",
        )),
    }
}
fn unavailable() -> ApplicationError {
    failure(
        ErrorCode::Unavailable,
        "Media request storage is unavailable",
    )
}

fn invalid(message: &str) -> ApplicationError {
    failure(ErrorCode::Validation, message)
}
