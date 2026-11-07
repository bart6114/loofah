use std::{
    io::{self, Cursor, Read},
    sync::Arc,
    time::Duration,
};

use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{HeaderMap, header},
    middleware,
    routing::{get, post},
};
use futures_util::StreamExt;
use hypr_loofah_protocol::{
    AttachmentUploadStatus, ErrorCode, FileMetadata, MutationOutcome, UploadAttachmentRequest,
    validate_upload_attachment_request,
};
use tokio::sync::mpsc;

use crate::{
    auth::{Identity, now},
    error::{ApiError, Result},
    media::MediaAccess,
    routes::{
        ApiPath, ApiQuery, IdentityExtension, admitted_write, authorize, authorize_upload,
        request_id,
    },
    service::Service,
};

const UPLOAD_IDLE: Duration = Duration::from_secs(35);
const CHUNK_BYTES: usize = 64 * 1024;
pub(crate) const MAX_ATTACHMENT_BYTES: u64 = 32 * 1024 * 1024 * 1024;

pub(crate) fn router(service: Arc<Service>) -> Router<Arc<Service>> {
    Router::new()
        .route(
            "/vaults/{vault}/sessions/{session}/attachments",
            post(upload).route_layer(middleware::from_fn_with_state(
                service.clone(),
                authorize_upload,
            )),
        )
        .route(
            "/vaults/{vault}/sessions/{session}/attachment-uploads/{upload}",
            get(status).route_layer(middleware::from_fn_with_state(service, authorize)),
        )
}

enum UploadPart {
    Bytes(Vec<u8>),
    Complete,
}
struct UploadReader {
    receive: mpsc::Receiver<UploadPart>,
    current: Cursor<Vec<u8>>,
    complete: bool,
}
impl Read for UploadReader {
    fn read(&mut self, target: &mut [u8]) -> io::Result<usize> {
        if target.is_empty() {
            return Ok(0);
        }
        loop {
            let count = self.current.read(target)?;
            if count != 0 || self.complete {
                return Ok(count);
            }
            match self.receive.blocking_recv() {
                Some(UploadPart::Bytes(bytes)) => self.current = Cursor::new(bytes),
                Some(UploadPart::Complete) => self.complete = true,
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::UnexpectedEof,
                        "Upload body did not complete",
                    ));
                }
            }
        }
    }
}

async fn upload(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session)): ApiPath<(String, String)>,
    ApiQuery(input): ApiQuery<UploadAttachmentRequest>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<FileMetadata>> {
    let request_id = request_id(&headers)?.to_owned();
    validate(&input)?;
    if headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        != Some("application/octet-stream")
    {
        return Err(rejected(
            ErrorCode::Validation,
            "Use an octet-stream upload body",
        ));
    }
    if headers
        .get(header::CONTENT_ENCODING)
        .is_some_and(|value| value != "identity")
    {
        return Err(rejected(
            ErrorCode::Validation,
            "Encoded upload bodies are not supported",
        ));
    }
    if let Some(length) = headers.get(header::CONTENT_LENGTH)
        && length
            .to_str()
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            != Some(input.size_bytes)
    {
        return Err(rejected(
            ErrorCode::Validation,
            "Upload length does not match its declaration",
        ));
    }
    let access = service.catalog.access(&identity, &vault, true).await?;
    let context = service.vault(&vault).await?;
    let namespace = access.member_id.clone();
    let permit = service
        .attachment_uploads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Attachment transfers are busy; retry shortly"))?;
    drop(access);
    let monitor = Arc::new(MediaAccess::watch_write(
        service.clone(),
        identity.clone(),
        vault.clone(),
    ));
    let cancel = monitor.cancel.clone();
    let observer = monitor.clone();
    let authorization = service.clone();
    let host = service.clone();
    let expected = input.size_bytes;
    let checked_identity = identity.clone();
    let checked_vault = vault.clone();
    let (send, receive) = mpsc::channel(2);
    let mut work = tokio::spawn(async move {
        admitted_write(&host, async move {
            let _permit = permit;
            let _monitor = observer;
            context
                .upload_attachment_authorized(
                    namespace,
                    request_id,
                    &session,
                    input,
                    UploadReader {
                        receive,
                        current: Cursor::new(Vec::new()),
                        complete: false,
                    },
                    move || async move {
                        current_access(&authorization, &identity, &vault, true)
                            .await
                            .map_err(|cause| cause.0)
                    },
                )
                .await
                .map_err(ApiError::from)
        })
        .await
    });
    let mut stream = body.into_data_stream();
    let mut received = 0_u64;
    loop {
        let next = tokio::select! {
            biased;
            result = &mut work => return result.map_err(ApiError::internal)?.map(Json),
            _ = cancel.cancelled() => return Err(access_ended(&service, &checked_identity, &checked_vault).await),
            next = tokio::time::timeout(UPLOAD_IDLE, stream.next()) => next.map_err(|_| rejected(ErrorCode::Unavailable, "Upload body stopped making progress"))?,
        };
        let Some(bytes) = next else {
            break;
        };
        let bytes =
            bytes.map_err(|_| rejected(ErrorCode::Validation, "Upload body was interrupted"))?;
        received = received
            .checked_add(bytes.len() as u64)
            .filter(|count| *count <= expected)
            .ok_or_else(|| {
                rejected(
                    ErrorCode::Validation,
                    "Upload body exceeds its declared length",
                )
            })?;
        for part in bytes.chunks(CHUNK_BYTES) {
            let sent = tokio::select! {
                biased;
                result = &mut work => return result.map_err(ApiError::internal)?.map(Json),
                _ = cancel.cancelled() => return Err(access_ended(&service, &checked_identity, &checked_vault).await),
                sent = tokio::time::timeout(UPLOAD_IDLE, send.send(UploadPart::Bytes(part.to_vec()))) => sent.map_err(|_| rejected(ErrorCode::Unavailable, "Upload storage stopped making progress"))?,
            };
            if sent.is_err() {
                return work.await.map_err(ApiError::internal)?.map(Json);
            }
        }
    }
    if received != expected {
        return Err(rejected(
            ErrorCode::Validation,
            "Upload body did not reach its declared length",
        ));
    }
    let sent = tokio::select! {
        biased;
        result = &mut work => return result.map_err(ApiError::internal)?.map(Json),
        _ = cancel.cancelled() => return Err(access_ended(&service, &checked_identity, &checked_vault).await),
        sent = tokio::time::timeout(UPLOAD_IDLE, send.send(UploadPart::Complete)) => sent.map_err(|_| rejected(ErrorCode::Unavailable, "Upload storage stopped making progress"))?,
    };
    if sent.is_err() {
        return work.await.map_err(ApiError::internal)?.map(Json);
    }
    drop(send);
    work.await.map_err(ApiError::internal)?.map(Json)
}

async fn status(
    State(service): State<Arc<Service>>,
    axum::Extension(identity): IdentityExtension,
    ApiPath((vault, session, upload)): ApiPath<(String, String, String)>,
    ApiQuery(input): ApiQuery<UploadAttachmentRequest>,
) -> Result<Json<AttachmentUploadStatus>> {
    validate(&input)?;
    let access = service.catalog.access(&identity, &vault, false).await?;
    let context = service.vault(&vault).await?;
    let namespace = access.member_id.clone();
    let permit = service
        .attachment_uploads
        .clone()
        .try_acquire_owned()
        .map_err(|_| ApiError::busy("Attachment verification is busy; retry shortly"))?;
    let authorization = service.clone();
    drop(access);
    admitted_write(&service, async move {
        let _permit = permit;
        context
            .attachment_upload_status_authorized(
                namespace,
                upload,
                &session,
                input,
                move || async move {
                    current_access(&authorization, &identity, &vault, false)
                        .await
                        .map_err(|cause| cause.0)
                },
            )
            .await
            .map(Json)
            .map_err(ApiError::from)
    })
    .await
}

fn validate(input: &UploadAttachmentRequest) -> Result<()> {
    validate_upload_attachment_request(input)
        .map_err(|message| rejected(ErrorCode::Validation, message))?;
    if input.size_bytes > MAX_ATTACHMENT_BYTES {
        return Err(rejected(
            ErrorCode::ResourceLimit,
            "Attachment exceeds the 32 GiB server upload limit",
        ));
    }
    Ok(())
}
async fn current_access(
    service: &Service,
    identity: &Identity,
    vault: &str,
    edit: bool,
) -> Result<crate::catalog::VaultAccess> {
    if service.stop.is_cancelled() {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "Server is shutting down",
        ));
    }
    if identity.expires_at <= now() {
        return Err(ApiError::new(ErrorCode::Unauthenticated, "Sign in again"));
    }
    service.catalog.access(identity, vault, edit).await
}
async fn access_ended(service: &Service, identity: &Identity, vault: &str) -> ApiError {
    let mut cause = current_access(service, identity, vault, true)
        .await
        .err()
        .unwrap_or_else(|| {
            ApiError::new(
                ErrorCode::Cancelled,
                "Upload access ended; reconnect before retrying",
            )
        });
    cause.0.mutation_outcome = Some(MutationOutcome::NotCommitted);
    cause
}
fn rejected(code: ErrorCode, message: &str) -> ApiError {
    let mut cause = ApiError::new(code, message);
    cause.0.mutation_outcome = Some(MutationOutcome::NotCommitted);
    cause
}
