use std::{io::Read, sync::Arc, time::Duration};

use hypr_loofah_protocol::{
    API_MAJOR, ApplicationError, ErrorCode, FileMetadata, MutationOutcome, UploadAttachmentRequest,
    validate_attachment_basename, validate_upload_attachment_request,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};
use tokio_util::sync::CancellationToken;

use crate::{
    RemoteClient, RemoteOperation, RemoteReply, Result, cancelled, failure, invalid, json_body,
    unavailable,
};

#[derive(Clone, Debug, Serialize, Deserialize, specta::Type)]
pub struct AttachmentUpload {
    pub vault_id: String,
    pub session_id: String,
    pub request_id: String,
    pub input: UploadAttachmentRequest,
}

impl RemoteClient {
    pub async fn upload_attachment(
        self: &Arc<Self>,
        upload: AttachmentUpload,
        source: impl Read + Send + 'static,
        cancel: CancellationToken,
    ) -> Result<FileMetadata> {
        self.upload_attachment_inner(upload, source, cancel, Duration::from_secs(35))
            .await
    }

    pub(crate) async fn upload_attachment_inner(
        self: &Arc<Self>,
        upload: AttachmentUpload,
        source: impl Read + Send + 'static,
        cancel: CancellationToken,
        idle: Duration,
    ) -> Result<FileMetadata> {
        let request_id = upload.request_id.clone();
        let prepared = async {
            if self.stop.is_cancelled() || cancel.is_cancelled() { return Err(cancelled()); }
            if !self.capabilities.features.iter().any(|feature| feature == "attachment_upload") {
                return Err(failure(ErrorCode::IncompatibleVersion, "This server does not support attachment uploads. Upgrade the server, then reconnect."));
            }
            validate_upload_attachment_request(&upload.input).map_err(invalid)?;
            uuid::Uuid::parse_str(&request_id).map_err(|_| invalid("A UUID upload request ID is required"))?;
            let mut url = self.endpoint(&["vaults", &upload.vault_id, "sessions", &upload.session_id, "attachments"])?;
            url.query_pairs_mut()
                .append_pair("filename", &upload.input.filename)
                .append_pair("size_bytes", &upload.input.size_bytes.to_string())
                .append_pair("sha256", &upload.input.sha256);
            let permit = self.upload_writers.clone().try_acquire_owned().map_err(|_| {
                let mut cause = unavailable();
                cause.message = "Attachment uploads are busy; retry shortly".into();
                cause
            })?;
            let access = tokio::select! {
                _ = cancel.cancelled() => return Err(cancelled()),
                _ = self.stop.cancelled() => return Err(cancelled()),
                access = self.access_token() => access?,
            };
            Ok((url, permit, access))
        }.await.map_err(|mut cause| {
            cause.request_id = Some(request_id.clone());
            cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
            cause
        })?;
        let (url, permit, access) = prepared;
        let _permit = Arc::new(permit);
        let producer_permit = _permit.clone();
        let status = self
            .execute(
                RemoteOperation::AttachmentUploadStatus {
                    upload: upload.clone(),
                },
                &request_id,
                cancel.clone(),
            )
            .await
            .map_err(|mut cause| {
                cause
                    .mutation_outcome
                    .get_or_insert(MutationOutcome::Unknown);
                cause
            })?;
        match status {
            RemoteReply::AttachmentUploadStatus(
                hypr_loofah_protocol::AttachmentUploadStatus::Committed(metadata),
            ) => return Ok(metadata),
            RemoteReply::AttachmentUploadStatus(
                hypr_loofah_protocol::AttachmentUploadStatus::Missing
                | hypr_loofah_protocol::AttachmentUploadStatus::Rejected,
            ) => {}
            _ => {
                let mut cause = failure(
                    ErrorCode::Conflict,
                    "Upload could not be confirmed; retain its original request and check its status again",
                );
                cause.request_id = Some(request_id);
                cause.mutation_outcome = Some(MutationOutcome::Unknown);
                return Err(cause);
            }
        }
        let cancel = cancel.child_token();
        let _on_drop = cancel.clone().drop_guard();
        let (send, receive) = mpsc::channel(2);
        let (progress, mut progressed) = watch::channel(Instant::now());
        let body = reqwest::Body::wrap_stream(futures_util::stream::unfold(
            (receive, progress, false),
            |(mut receive, progress, ended)| async move {
                if ended {
                    return None;
                }
                let (part, ended) = match receive.recv().await {
                    Some(UploadPart::Bytes(bytes)) => (Ok(bytes), false),
                    Some(UploadPart::Complete) => return None,
                    Some(UploadPart::Failed) | None => (
                        Err(std::io::Error::other("Attachment source did not complete")),
                        true,
                    ),
                };
                progress.send_replace(Instant::now());
                Some((part, (receive, progress, ended)))
            },
        ));
        let input = upload.input.clone();
        let stopped = self.stop.clone();
        let producer_cancel = cancel.clone();
        // The reader owns admission until actual blocking I/O ends, even if this future is dropped.
        let window_work = self.begin_window_work()?;
        tokio::task::spawn_blocking(move || {
            let _window_work = window_work;
            let _permit = producer_permit;
            produce(source, input, send, producer_cancel, stopped);
        });
        // A streaming body deliberately has no Content-Length: an errored source must never
        // turn exactly the declared bytes into an apparently complete HTTP request.
        let request = self
            .http
            .post(url)
            .bearer_auth(&access)
            .header("x-loofah-api-major", API_MAJOR.to_string())
            .header("x-request-id", &request_id)
            .header("content-type", "application/octet-stream")
            .body(body);
        let send = self.send_window_request(request);
        tokio::pin!(send);
        let result = async {
            let mut progress_open = true;
            let response = loop {
                let deadline = *progressed.borrow_and_update() + idle;
                tokio::select! {
                    result = &mut send => break result?,
                    _ = tokio::time::sleep_until(deadline) => return Err(unavailable()),
                    changed = progressed.changed(), if progress_open => { progress_open = changed.is_ok(); },
                }
            };
            if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                self.expire_access(&access).await;
            }
            if response.status() != reqwest::StatusCode::OK {
                return Err(tokio::time::timeout(
                    idle,
                    json_body::<ApplicationError>(response, 131_072),
                )
                .await
                .unwrap_or_else(|_| Err(unavailable()))
                .unwrap_or_else(|_| unavailable()));
            }
            let metadata: FileMetadata = tokio::time::timeout(idle, json_body(response, 16 * 1024))
                .await
                .map_err(|_| unavailable())??;
            validate_uploaded_metadata(&metadata, &upload.input)?;
            Ok(metadata)
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = self.stop.cancelled() => Err(cancelled()),
            result = result => result,
        }
        .map_err(|mut cause| {
            cause.request_id.get_or_insert(request_id);
            cause
                .mutation_outcome
                .get_or_insert(MutationOutcome::Unknown);
            cause
        })
    }
}

pub(crate) enum UploadPart {
    Bytes(Vec<u8>),
    Complete,
    Failed,
}

pub(crate) fn produce(
    mut source: impl Read,
    input: UploadAttachmentRequest,
    send: mpsc::Sender<UploadPart>,
    cancel: CancellationToken,
    stopped: CancellationToken,
) {
    let result = (|| {
        let mut count = 0_u64;
        let mut hash = Sha256::new();
        loop {
            if cancel.is_cancelled() || stopped.is_cancelled() || send.is_closed() {
                return Ok(());
            }
            let remaining = input
                .size_bytes
                .saturating_sub(count)
                .saturating_add(1)
                .min(64 * 1024) as usize;
            let mut buffer = vec![0_u8; remaining];
            let read = loop {
                match source.read(&mut buffer) {
                    Err(cause) if cause.kind() == std::io::ErrorKind::Interrupted => continue,
                    result => break result?,
                }
            };
            if read == 0 {
                let observed: String = hash
                    .finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                if count != input.size_bytes || observed != input.sha256 {
                    return Err(std::io::Error::other(
                        "Attachment source does not match its declaration",
                    ));
                }
                let _ = send.blocking_send(UploadPart::Complete);
                return Ok(());
            }
            count = count
                .checked_add(read as u64)
                .filter(|count| *count <= input.size_bytes)
                .ok_or_else(|| {
                    std::io::Error::other("Attachment source exceeds its declaration")
                })?;
            buffer.truncate(read);
            hash.update(&buffer);
            if send.blocking_send(UploadPart::Bytes(buffer)).is_err() {
                return Ok(());
            }
        }
    })();
    if result.is_err() {
        let _ = send.blocking_send(UploadPart::Failed);
    }
}

pub(crate) fn validate_uploaded_metadata(
    metadata: &FileMetadata,
    input: &UploadAttachmentRequest,
) -> Result<()> {
    if validate_attachment_basename(&metadata.filename).is_err()
        || metadata.size_bytes != input.size_bytes
        || metadata.sha256 != input.sha256
    {
        return Err(failure(
            ErrorCode::IncompatibleVersion,
            "Invalid uploaded attachment metadata",
        ));
    }
    crate::media::validate_file_metadata(metadata)
}
