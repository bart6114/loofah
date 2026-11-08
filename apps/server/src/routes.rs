use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{
        IntoResponse, Response, Sse,
        sse::{Event, KeepAlive},
    },
    routing::get,
};
use hypr_loofah_protocol::*;
use serde::Deserialize;
use std::{convert::Infallible, sync::Arc, time::Duration};

use crate::{
    auth::{Identity, now},
    catalog::VaultAccess,
    error::{ApiError, Result},
    service::Service,
};

mod capture;
mod jobs;
mod processing_settings;
mod session_cleanup;
mod session_summaries;
mod tag_editing;
mod transcript_overview;

pub fn router(service: Arc<Service>) -> Router {
    let protected = Router::new()
        .merge(crate::media::router())
        .merge(capture::router())
        .merge(jobs::router())
        .merge(processing_settings::router())
        .merge(session_cleanup::router())
        .merge(session_summaries::router())
        .merge(transcript_overview::router())
        .merge(tag_editing::router())
        .route("/me", get(me).post(provision))
        .route("/members", get(directory))
        .route("/vaults", get(vaults).post(create_vault))
        .route(
            "/vaults/{vault}",
            get(vault).patch(rename_vault).delete(delete_vault),
        )
        .route("/vaults/{vault}/members", get(memberships))
        .route(
            "/vaults/{vault}/members/{member}",
            axum::routing::put(set_membership).delete(remove_membership),
        )
        .route(
            "/vaults/{vault}/sessions",
            get(sessions).post(create_session),
        )
        .route("/vaults/{vault}/sessions/{session}", get(session))
        .route(
            "/vaults/{vault}/sessions/{session}/attachments",
            get(attachments),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/attachments/{attachment}",
            axum::routing::delete(remove_attachment),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/audio/metadata",
            get(audio_metadata),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/audio",
            axum::routing::delete(delete_recording),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/note/editor",
            get(note_editor).put(save_note_editor),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/metadata",
            get(metadata).patch(save_metadata),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/documents",
            get(documents),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/summary",
            get(summary).put(save_summary),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/outputs/{document}",
            get(output).put(save_output),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/transcripts",
            get(transcripts),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/tasks",
            get(tasks).put(replace_tasks),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/tasks/snapshot",
            get(tasks_snapshot),
        )
        .route("/vaults/{vault}/people", get(people))
        .route("/vaults/{vault}/tags", get(tags))
        .route(
            "/vaults/{vault}/agent/sessions/list",
            axum::routing::post(agent_list),
        )
        .route(
            "/vaults/{vault}/agent/sessions/search",
            axum::routing::post(agent_search),
        )
        .route(
            "/vaults/{vault}/agent/sessions/{session}",
            get(agent_meeting),
        )
        .route(
            "/vaults/{vault}/agent/sessions/{session}/transcript",
            get(agent_transcript),
        )
        .route(
            "/vaults/{vault}/agent/sessions/{session}/export",
            get(agent_export),
        )
        .route(
            "/vaults/{vault}/agent/sessions/{session}/segments",
            get(agent_segments),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/note",
            get(note).put(save_note),
        )
        .route("/vaults/{vault}/search", axum::routing::post(search))
        .route("/vaults/{vault}/events", get(events))
        .route_layer(middleware::from_fn_with_state(service.clone(), authorize));
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/capabilities", get(capabilities))
        .route("/api/v1/auth/config", get(auth_config))
        .nest(
            "/api/v1",
            protected.merge(crate::uploads::router(service.clone())),
        )
        .fallback(|| async { ApiError::new(ErrorCode::NotFound, "API route not found") })
        .method_not_allowed_fallback(|| async {
            let mut response = ApiError::new(
                ErrorCode::Validation,
                "Method is not supported for this operation",
            )
            .into_response();
            *response.status_mut() = StatusCode::METHOD_NOT_ALLOWED;
            response
        })
        .layer(DefaultBodyLimit::max(5 * 1024 * 1024))
        .layer(middleware::from_fn(request_context))
        .with_state(service)
}

async fn health(State(service): State<Arc<Service>>) -> StatusCode {
    if service.stop.is_cancelled() || !service.catalog.ready().await {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::NO_CONTENT
    }
}

async fn capabilities(State(service): State<Arc<Service>>) -> Json<Capabilities> {
    let mut capabilities = Capabilities {
        api_major: API_MAJOR,
        application_version: env!("LOOFAH_VERSION").into(),
        features: vec![
            "capture_ingestion".into(),
            "capture_publication".into(),
            "notes".into(),
            "session_summaries".into(),
            "transcript_overview".into(),
            "tag_editing".into(),
            "session_cleanup".into(),
            "note_editor_read".into(),
            "note_editor_write".into(),
            "audio_metadata_read".into(),
            "recording_leases".into(),
            "recording_waveforms".into(),
            "audio_playback".into(),
            "recording_delete".into(),
            "attachment_leases".into(),
            "attachment_info".into(),
            "attachment_list".into(),
            "attachment_metadata".into(),
            "attachment_remove".into(),
            "attachment_upload".into(),
            "artifact_details".into(),
            "content_read".into(),
            "document_edit".into(),
            "metadata_edit".into(),
            "tasks_edit".into(),
            "agent_read".into(),
            "search".into(),
            "vault_membership".into(),
            "change_stream".into(),
        ],
    };
    if service.config.processing.is_some() {
        capabilities.features.push("summary_body_jobs".into());
        capabilities.features.push("template_output_jobs".into());
        capabilities.features.push("generated_titles".into());
        capabilities.features.push("processing_settings".into());
    }
    Json(capabilities)
}
async fn auth_config(State(service): State<Arc<Service>>) -> Json<RemoteAuthConfig> {
    Json(service.auth.public_config())
}

async fn request_context(mut request: Request, next: Next) -> Response {
    let id = request
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| uuid::Uuid::parse_str(value).ok())
        .unwrap_or_else(uuid::Uuid::new_v4)
        .to_string();
    request.extensions_mut().insert(TrackingId(id.clone()));
    let mut response = crate::error::REQUEST_ID
        .scope(id.clone(), next.run(request))
        .await;
    response
        .headers_mut()
        .insert("x-request-id", id.parse().unwrap());
    response
        .headers_mut()
        .insert("cache-control", "no-store".parse().unwrap());
    response
}

pub(crate) async fn authorize(
    State(service): State<Arc<Service>>,
    request: Request,
    next: Next,
) -> Response {
    authorize_with(service, request, next, false).await
}

pub(crate) async fn authorize_upload(
    State(service): State<Arc<Service>>,
    request: Request,
    next: Next,
) -> Response {
    authorize_with(service, request, next, true).await
}

async fn authorize_with(
    service: Arc<Service>,
    mut request: Request,
    next: Next,
    streaming: bool,
) -> Response {
    let Ok(_request_permit) = service.requests.clone().try_acquire_owned() else {
        return ApiError::busy("Request capacity reached; retry later").into_response();
    };
    let headers = request.headers().clone();
    let result = async {
        if service.stop.is_cancelled() {
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "Server is shutting down",
            ));
        }
        if headers
            .get("x-loofah-api-major")
            .and_then(|value| value.to_str().ok())
            != Some("1")
        {
            return Err(ApiError::new(
                ErrorCode::IncompatibleVersion,
                "Negotiate API major 1 before accessing a vault",
            ));
        }
        let token = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::Unauthenticated,
                    "A company access token is required",
                )
            })?;
        service.auth.authenticate(token).await
    }
    .await;
    match result {
        Ok(identity) => {
            request.extensions_mut().insert(identity);
            if streaming {
                return next.run(request).await;
            }
            match tokio::time::timeout(Duration::from_secs(30), next.run(request)).await {
                Ok(response) => response,
                Err(_) => {
                    let mut error = ApiError::new(
                        ErrorCode::Unavailable,
                        "Request deadline exceeded; reload before retrying a write",
                    );
                    error.0.mutation_outcome = Some(MutationOutcome::Unknown);
                    error.into_response()
                }
            }
        }
        Err(mut error) => {
            error.0.request_id = request
                .extensions()
                .get::<TrackingId>()
                .map(|id| id.0.clone());
            error.into_response()
        }
    }
}

#[derive(Clone)]
struct TrackingId(String);

pub(crate) type IdentityExtension = axum::Extension<Identity>;

async fn provision(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
) -> Result<Json<Me>> {
    Ok(Json(service.catalog.provision(&identity).await?))
}
async fn me(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
) -> Result<Json<Me>> {
    Ok(Json(service.catalog.me(&identity).await?))
}
async fn directory(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
) -> Result<Json<Vec<MemberInfo>>> {
    Ok(Json(service.catalog.directory(&identity).await?))
}
async fn vaults(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
) -> Result<Json<Vec<VaultInfo>>> {
    Ok(Json(service.catalog.list(&identity).await?))
}
async fn vault(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<VaultInfo>> {
    Ok(Json(
        service.catalog.access(&identity, &id, false).await?.info,
    ))
}
async fn create_vault(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    headers: HeaderMap,
    ApiJson(request): ApiJson<CreateVaultRequest>,
) -> Result<Json<VaultInfo>> {
    Ok(Json(
        service
            .catalog
            .create_shared(&identity, request_id(&headers)?, &request.name)
            .await?,
    ))
}
async fn rename_vault(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
    ApiJson(request): ApiJson<UpdateVaultRequest>,
) -> Result<Json<VaultMutation>> {
    Ok(Json(
        service
            .catalog
            .rename(&identity, &id, &request.name, &request.expected_revision)
            .await?,
    ))
}
#[derive(Deserialize)]
struct RevisionQuery {
    expected_revision: String,
}
async fn delete_vault(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
    ApiQuery(request): ApiQuery<RevisionQuery>,
) -> Result<Json<VaultMutation>> {
    Ok(Json(
        service
            .catalog
            .delete(&identity, &id, &request.expected_revision)
            .await?,
    ))
}
async fn memberships(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
) -> Result<Json<Vec<Membership>>> {
    Ok(Json(service.catalog.memberships(&identity, &id).await?))
}
async fn set_membership(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, member)): ApiPath<(String, String)>,
    ApiJson(request): ApiJson<SetMembershipRequest>,
) -> Result<Json<VaultMutation>> {
    Ok(Json(
        service
            .catalog
            .set_membership(
                &identity,
                &id,
                &member,
                Some(request.role),
                &request.expected_revision,
            )
            .await?,
    ))
}
async fn remove_membership(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, member)): ApiPath<(String, String)>,
    ApiQuery(request): ApiQuery<RevisionQuery>,
) -> Result<Json<VaultMutation>> {
    Ok(Json(
        service
            .catalog
            .set_membership(&identity, &id, &member, None, &request.expected_revision)
            .await?,
    ))
}

#[derive(Deserialize)]
struct PageQuery {
    #[serde(default = "default_limit")]
    limit: usize,
    after: Option<String>,
}
fn default_limit() -> usize {
    100
}
async fn sessions(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
    ApiQuery(page): ApiQuery<PageQuery>,
) -> Result<Json<SessionPage>> {
    let _access = service.catalog.access(&identity, &id, false).await?;
    Ok(Json(
        service
            .vault(&id)
            .await?
            .session_page(page.limit, page.after.as_deref())?,
    ))
}
async fn attachments(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
    ApiQuery(page): ApiQuery<PageQuery>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 256 * 1024, async move {
        context
            .attachment_page(&session, page.limit, page.after.as_deref())
            .await
    })
    .await
}
async fn session(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Json<SessionDocument>> {
    let _access = service.catalog.access(&identity, &id, false).await?;
    Ok(Json(
        service.vault(&id).await?.session_document(&session).await?,
    ))
}
async fn create_session(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
    ApiJson(request): ApiJson<CreateSessionRequest>,
) -> Result<Json<SessionDocument>> {
    let access = service.catalog.access(&identity, &id, true).await?;
    let context = service.vault(&id).await?;
    let namespace = access.member_id.clone();
    let request_id = request_id(&headers)?.to_owned();
    write(&service, access, async move {
        Ok(Json(
            context
                .create_session(namespace, request_id, request)
                .await?,
        ))
    })
    .await
}
async fn note(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Json<NoteSnapshot>> {
    let _access = service.catalog.access(&identity, &id, false).await?;
    Ok(Json(service.vault(&id).await?.read_note(&session).await?))
}

async fn metadata(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 16 * 1024 * 1024, async move {
        context.read_metadata(&session).await
    })
    .await
}

async fn save_metadata(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
    ApiJson(input): ApiJson<SaveMetadataRequest>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, true).await?;
    let context = service.vault(&id).await?;
    write(&service, access, async move {
        let result = context.save_metadata(&session, input).await?;
        let status = if matches!(result, MetadataSaveOutcome::Conflict(_)) {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        };
        let bytes = tokio::task::spawn_blocking(move || encode_content(&result, 16 * 1024 * 1024))
            .await
            .map_err(ApiError::internal)??;
        Ok((status, [("content-type", "application/json")], bytes).into_response())
    })
    .await
}

async fn tasks_snapshot(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 16 * 1024 * 1024, async move {
        context.read_tasks_snapshot(&session).await
    })
    .await
}

async fn replace_tasks(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
    ApiJson(input): ApiJson<ReplaceTasksRequest>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, true).await?;
    let context = service.vault(&id).await?;
    write(&service, access, async move {
        let result = context.replace_tasks(&session, input).await?;
        let status = if matches!(result, TasksSaveOutcome::Conflict(_)) {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        };
        let bytes = tokio::task::spawn_blocking(move || encode_content(&result, 16 * 1024 * 1024))
            .await
            .map_err(ApiError::internal)??;
        Ok((status, [("content-type", "application/json")], bytes).into_response())
    })
    .await
}

async fn documents(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 32 * 1024 * 1024, async move {
        context.session_documents(&session).await
    })
    .await
}
async fn summary(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    read_document(service, identity, id, session, DocumentTarget::Summary).await
}
async fn output(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session, document_id)): ApiPath<(String, String, String)>,
) -> Result<Response> {
    read_document(
        service,
        identity,
        id,
        session,
        DocumentTarget::TemplateOutput { document_id },
    )
    .await
}
async fn read_document(
    service: Arc<Service>,
    identity: Identity,
    id: String,
    session: String,
    target: DocumentTarget,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 32 * 1024 * 1024, async move {
        context.read_document(&session, &target).await
    })
    .await
}
async fn save_summary(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
    ApiJson(input): ApiJson<SaveDocumentRequest>,
) -> Result<Response> {
    save_document(
        service,
        identity,
        id,
        session,
        DocumentTarget::Summary,
        input,
    )
    .await
}
async fn save_output(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session, document_id)): ApiPath<(String, String, String)>,
    ApiJson(input): ApiJson<SaveDocumentRequest>,
) -> Result<Response> {
    save_document(
        service,
        identity,
        id,
        session,
        DocumentTarget::TemplateOutput { document_id },
        input,
    )
    .await
}
async fn save_document(
    service: Arc<Service>,
    identity: Identity,
    id: String,
    session: String,
    target: DocumentTarget,
    input: SaveDocumentRequest,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, true).await?;
    let context = service.vault(&id).await?;
    write(&service, access, async move {
        let result = context.save_document(&session, &target, input).await?;
        let status = if matches!(result, DocumentSaveOutcome::Conflict(_)) {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        };
        let bytes = tokio::task::spawn_blocking(move || encode_content(&result, 32 * 1024 * 1024))
            .await
            .map_err(ApiError::internal)??;
        Ok((status, [("content-type", "application/json")], bytes).into_response())
    })
    .await
}
async fn audio_metadata(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    crate::media::metadata_read(&service, access, identity, id, async move {
        context.audio_metadata(&session).await
    })
    .await
}

async fn delete_recording(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
    ApiQuery(input): ApiQuery<DeleteRecordingRequest>,
    headers: HeaderMap,
) -> Result<Json<bool>> {
    request_id(&headers)?;
    let access = service.catalog.access(&identity, &id, true).await?;
    let context = service.vault(&id).await?;
    let permit = service
        .media_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Recording readers are busy; retry shortly"))?;
    let leases = service.media_leases.clone();
    let authorization = service.clone();
    let vault = id.clone();
    drop(access);
    admitted_write(&service, async move {
        let _permit = permit;
        let deleted = context
            .delete_recording_authorized(&session, input, move || async move {
                if authorization.stop.is_cancelled() {
                    return Err(ApiError::new(ErrorCode::Unavailable, "Server is shutting down").0);
                }
                if identity.expires_at <= now() {
                    return Err(ApiError::new(ErrorCode::Unauthenticated, "Sign in again").0);
                }
                authorization
                    .catalog
                    .access(&identity, &vault, true)
                    .await
                    .map_err(|cause| cause.0)
            })
            .await?;
        leases.close_recording_session(&id, &session);
        Ok(Json(deleted))
    })
    .await
}

async fn remove_attachment(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session, attachment)): ApiPath<(String, String, String)>,
    ApiQuery(input): ApiQuery<RemoveAttachmentRequest>,
    headers: HeaderMap,
) -> Result<Json<bool>> {
    let request_id = request_id(&headers)?.to_owned();
    let access = service.catalog.access(&identity, &id, true).await?;
    let context = service.vault(&id).await?;
    let namespace = access.member_id.clone();
    let permit = service
        .media_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Media readers are busy; retry shortly"))?;
    let leases = service.media_leases.clone();
    let authorization = service.clone();
    let vault = id.clone();
    drop(access);
    admitted_write(&service, async move {
        let _permit = permit;
        let expected_sha256 = input.expected_sha256.clone();
        let removed = context
            .remove_attachment_authorized(
                namespace,
                request_id,
                &session,
                &attachment,
                input,
                move || async move {
                    if authorization.stop.is_cancelled() {
                        return Err(ApiError::new(
                            ErrorCode::Unavailable,
                            "Server is shutting down",
                        )
                        .0);
                    }
                    if identity.expires_at <= now() {
                        return Err(ApiError::new(ErrorCode::Unauthenticated, "Sign in again").0);
                    }
                    authorization
                        .catalog
                        .access(&identity, &vault, true)
                        .await
                        .map_err(|cause| cause.0)
                },
            )
            .await?;
        leases.close_attachment_source(&id, &session, &attachment, &expected_sha256);
        Ok(Json(removed))
    })
    .await
}

async fn transcripts(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 32 * 1024 * 1024, async move {
        context.session_transcripts(&session).await
    })
    .await
}
async fn tasks(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 8 * 1024 * 1024, async move {
        context.session_tasks(&session).await
    })
    .await
}
async fn people(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 1024 * 1024, async move {
        context.people().await
    })
    .await
}
async fn tags(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 1024 * 1024, async move {
        context.tags().await
    })
    .await
}

async fn agent_list(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
    ApiJson(input): ApiJson<ListMeetingsInput>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 8 * 1024 * 1024, async move {
        context.agent_list(input).await
    })
    .await
}
async fn agent_search(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
    ApiJson(input): ApiJson<SearchMeetingsInput>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 8 * 1024 * 1024, async move {
        context.agent_search(input).await
    })
    .await
}
async fn agent_meeting(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 32 * 1024 * 1024, async move {
        context.agent_meeting(&session).await
    })
    .await
}
async fn agent_transcript(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 32 * 1024 * 1024, async move {
        context.agent_transcript(&session).await
    })
    .await
}
async fn agent_export(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 32 * 1024 * 1024, async move {
        context.agent_export(&session).await
    })
    .await
}
async fn agent_segments(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 32 * 1024 * 1024, async move {
        context.agent_segments(&session).await
    })
    .await
}
pub(crate) async fn content_read<T: serde::Serialize + Send + 'static>(
    service: &Service,
    access: VaultAccess,
    limit: usize,
    operation: impl Future<Output = std::result::Result<T, ApplicationError>> + Send + 'static,
) -> Result<Response> {
    if service.stop.is_cancelled() {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "Server is shutting down",
        ));
    }
    let permit = service
        .content_reads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Content read capacity reached; retry later"))?;
    tokio::spawn(async move {
        let value = operation.await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let _access = access;
            let bytes = encode_content(&value, limit)?;
            Ok(([("content-type", "application/json")], bytes).into_response())
        })
        .await
        .map_err(ApiError::internal)?
    })
    .await
    .map_err(ApiError::internal)?
}

fn encode_content(value: &impl serde::Serialize, limit: usize) -> Result<Vec<u8>> {
    struct Buffer {
        bytes: Vec<u8>,
        limit: usize,
        full: bool,
    }
    impl std::io::Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.bytes.len().saturating_add(bytes.len()) > self.limit {
                self.full = true;
                return Err(std::io::Error::other("content response size limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Buffer {
        bytes: Vec::new(),
        limit,
        full: false,
    };
    if let Err(cause) = serde_json::to_writer(&mut buffer, value) {
        return Err(if buffer.full {
            ApiError::new(
                ErrorCode::ResourceLimit,
                "Content exceeds the current remote read limit",
            )
        } else {
            ApiError::internal(cause)
        });
    }
    Ok(buffer.bytes)
}

async fn note_editor(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    content_read(&service, access, 32 * 1024 * 1024, async move {
        context.read_note_editor(&session).await
    })
    .await
}

async fn save_note_editor(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
    ApiJson(input): ApiJson<SaveNoteEditorRequest>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, true).await?;
    let context = service.vault(&id).await?;
    write(&service, access, async move {
        let result = context.save_note_editor(&session, input).await?;
        let status = if matches!(result, NoteEditorSaveOutcome::Conflict(_)) {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        };
        let bytes = tokio::task::spawn_blocking(move || encode_content(&result, 32 * 1024 * 1024))
            .await
            .map_err(ApiError::internal)??;
        Ok((status, [("content-type", "application/json")], bytes).into_response())
    })
    .await
}

async fn save_note(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((id, session)): ApiPath<(String, String)>,
    ApiJson(request): ApiJson<SaveNoteRequest>,
) -> Result<Response> {
    let access = service.catalog.access(&identity, &id, true).await?;
    let context = service.vault(&id).await?;
    write(&service, access, async move {
        let result = context.save_note(&session, request).await?;
        let status = if matches!(result, NoteSaveOutcome::Conflict(_)) {
            StatusCode::CONFLICT
        } else {
            StatusCode::OK
        };
        Ok((status, Json(result)).into_response())
    })
    .await
}
async fn search(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
    ApiJson(request): ApiJson<hypr_search_cache::SearchRequest>,
) -> Result<Json<hypr_search_cache::SearchResult>> {
    let _access = service.catalog.access(&identity, &id, false).await?;
    Ok(Json(
        service.vault(&id).await?.search_sessions(request).await?,
    ))
}

async fn events(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath(id): ApiPath<String>,
    headers: HeaderMap,
) -> Result<Sse<impl futures_util::Stream<Item = std::result::Result<Event, Infallible>>>> {
    let access = service.catalog.access(&identity, &id, false).await?;
    let context = service.vault(&id).await?;
    let mut access_changes = service.catalog.subscribe();
    let cursor = headers
        .get("last-event-id")
        .and_then(|value| value.to_str().ok());
    if cursor.is_some_and(|cursor| cursor.len() > 128) {
        return Err(ApiError::new(ErrorCode::Validation, "Invalid event cursor"));
    }
    let mut subscription = context.events.subscribe_for(&access.member_id, cursor);
    drop(access);
    let permit = service
        .streams
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Subscription capacity reached"))?;
    let stream = async_stream::stream! {
        let _permit = permit;
        let expiry = tokio::time::sleep(Duration::from_secs(identity.expires_at.saturating_sub(now())));
        tokio::pin!(expiry);
        loop {
            tokio::select! {
                _ = service.stop.cancelled() => break,
                _ = &mut expiry => break,
                change = access_changes.changed() => {
                    if change.is_err() || service.catalog.access(&identity, &id, false).await.is_err() { break; }
                },
                event = subscription.recv() => {
                    let Some(event) = event else { break; };
                    let Ok(access) = service.catalog.access(&identity, &id, false).await else { break; };
                    let Ok(json) = serde_json::to_string(&event) else { break; };
                    drop(access);
                    yield Ok(Event::default().id(event.cursor).event("change").data(json));
                },
            }
        }
    };
    Ok(Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

pub(crate) fn request_id(headers: &HeaderMap) -> Result<&str> {
    let id = headers
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|id| uuid::Uuid::parse_str(id).is_ok());
    id.ok_or_else(|| {
        ApiError::new(
            ErrorCode::Validation,
            "A client-generated UUID request ID is required",
        )
    })
}

async fn write<T: Send + 'static>(
    service: &Service,
    access: VaultAccess,
    operation: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    admitted_write(service, async move {
        let _access = access;
        operation.await
    })
    .await
}

pub(crate) async fn admitted_write<T: Send + 'static>(
    service: &Service,
    operation: impl Future<Output = Result<T>> + Send + 'static,
) -> Result<T> {
    if service.stop.is_cancelled() {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "Server is shutting down",
        ));
    }
    let permit = service
        .writes
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Write capacity reached; retry later"))?;
    tokio::spawn(async move {
        let _permit = permit;
        operation.await
    })
    .await
    .map_err(ApiError::internal)?
}

struct ApiJson<T>(T);
impl<S: Send + Sync, T: serde::de::DeserializeOwned> axum::extract::FromRequest<S> for ApiJson<T> {
    type Rejection = ApiError;
    async fn from_request(request: Request, state: &S) -> Result<Self> {
        <Json<T> as axum::extract::FromRequest<S>>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|_| {
                ApiError::new(
                    ErrorCode::Validation,
                    "Request JSON is invalid or exceeds the size limit",
                )
            })
    }
}

pub(crate) struct ApiQuery<T>(pub(crate) T);
impl<S: Send + Sync, T: serde::de::DeserializeOwned> axum::extract::FromRequestParts<S>
    for ApiQuery<T>
{
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, state: &S) -> Result<Self> {
        <Query<T> as axum::extract::FromRequestParts<S>>::from_request_parts(parts, state)
            .await
            .map(|Query(value)| Self(value))
            .map_err(|_| ApiError::new(ErrorCode::Validation, "Request query is invalid"))
    }
}

pub(crate) struct ApiPath<T>(pub(crate) T);
impl<S: Send + Sync, T: serde::de::DeserializeOwned + Send> axum::extract::FromRequestParts<S>
    for ApiPath<T>
{
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut axum::http::request::Parts, state: &S) -> Result<Self> {
        <Path<T> as axum::extract::FromRequestParts<S>>::from_request_parts(parts, state)
            .await
            .map(|Path(value)| Self(value))
            .map_err(|_| ApiError::new(ErrorCode::Validation, "Request path is invalid"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_encoding_bounds_bytes_without_returning_a_truncated_document() {
        assert_eq!(encode_content(&"é", 4).unwrap(), b"\"\xc3\xa9\"");
        let error = encode_content(&"é", 3).unwrap_err();
        assert_eq!(error.0.code, ErrorCode::ResourceLimit);
        assert!(!error.0.retryable);
    }
}
