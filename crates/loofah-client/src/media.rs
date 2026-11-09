mod download;

use std::{sync::Arc, time::Duration};

use hypr_loofah_protocol::{
    API_MAJOR, AttachmentLease, ErrorCode, FileMetadata, RECORDING_CHUNK_LIMIT, RecordingLease,
    RecordingWaveform, validate_attachment_basename,
};
use reqwest::{Method, Response, StatusCode};
use tokio_util::sync::CancellationToken;

use crate::{RemoteClient, Result, cancelled, failure, invalid, json_body, unavailable};

#[derive(Clone, Copy)]
enum MediaKind {
    Recording,
    Attachment,
}
impl MediaKind {
    fn endpoint(self) -> &'static str {
        match self {
            Self::Recording => "audio",
            Self::Attachment => "attachments",
        }
    }
    fn capability(self) -> &'static str {
        match self {
            Self::Recording => "recording_leases",
            Self::Attachment => "attachment_leases",
        }
    }
}

#[derive(Clone, Copy)]
enum MediaRef<'a> {
    Recording(&'a RecordingLease),
    Attachment(&'a AttachmentLease),
}
impl<'a> MediaRef<'a> {
    fn kind(self) -> MediaKind {
        match self {
            Self::Recording(_) => MediaKind::Recording,
            Self::Attachment(_) => MediaKind::Attachment,
        }
    }
    fn parts(self) -> (&'a str, &'a str, &'a FileMetadata) {
        match self {
            Self::Recording(lease) => (&lease.vault_id, &lease.id, &lease.metadata),
            Self::Attachment(lease) => (&lease.vault_id, &lease.id, &lease.metadata),
        }
    }
    fn validate(self) -> Result<()> {
        let (_, _, metadata) = self.parts();
        let idle = match self {
            Self::Recording(lease) => {
                let mime = match metadata.filename.as_str() {
                    "audio.mp3" => "audio/mpeg",
                    "audio.wav" => "audio/wav",
                    "audio.ogg" => "audio/ogg",
                    _ => return Err(invalid_response()),
                };
                if metadata.content_type != mime {
                    return Err(invalid_response());
                }
                lease.idle_timeout_secs
            }
            Self::Attachment(lease) => {
                if metadata.filename != lease.attachment_id
                    || validate_attachment_basename(&lease.attachment_id).is_err()
                {
                    return Err(invalid_response());
                }
                lease.idle_timeout_secs
            }
        };
        if idle == 0 {
            return Err(invalid_response());
        }
        validate_file_metadata(metadata)
    }
}

pub(crate) fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(crate) fn validate_file_metadata(metadata: &FileMetadata) -> Result<()> {
    if !valid_sha256(&metadata.sha256) {
        return Err(invalid_response());
    }
    validate_content_type(&metadata.content_type)
}

pub(crate) fn validate_content_type(value: &str) -> Result<()> {
    if value.len() > 256
        || !value.is_ascii()
        || !value.contains('/')
        || reqwest::header::HeaderValue::from_str(value).is_err()
    {
        return Err(invalid_response());
    }
    Ok(())
}

impl RemoteClient {
    pub async fn head_recording(
        self: &Arc<Self>,
        lease: &RecordingLease,
        cancel: CancellationToken,
    ) -> Result<()> {
        self.head_media(MediaRef::Recording(lease), cancel).await
    }
    pub async fn head_attachment(
        self: &Arc<Self>,
        lease: &AttachmentLease,
        cancel: CancellationToken,
    ) -> Result<()> {
        self.head_media(MediaRef::Attachment(lease), cancel).await
    }

    async fn head_media(
        self: &Arc<Self>,
        lease: MediaRef<'_>,
        cancel: CancellationToken,
    ) -> Result<()> {
        let _window_work = self.begin_window_work()?;
        self.check_media_request(lease.kind(), &cancel)?;
        lease.validate()?;
        let (vault, id, metadata) = lease.parts();
        let id = normalize_id(id)?;
        let etag = format!("\"sha256:{}\"", metadata.sha256);
        let operation = async {
            let response = self
                .media_request(
                    Method::HEAD,
                    &["vaults", vault, lease.kind().endpoint(), "leases", &id],
                    &[("if-match", &etag)],
                )
                .await?;
            if response.status() != StatusCode::OK {
                return Err(media_head_error(response.status()));
            }
            if response
                .headers()
                .get("etag")
                .and_then(|value| value.to_str().ok())
                != Some(etag.as_str())
                || response
                    .headers()
                    .get("content-length")
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| value.parse::<u64>().ok())
                    != Some(metadata.size_bytes)
                || response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    != Some(metadata.content_type.as_str())
            {
                return Err(invalid_response());
            }
            Ok(())
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = self.stop.cancelled() => Err(cancelled()),
            result = operation => result,
        }
    }
    pub async fn recording_waveform(
        self: &Arc<Self>,
        lease: &RecordingLease,
        cancel: CancellationToken,
    ) -> Result<RecordingWaveform> {
        let _window_work = self.begin_window_work()?;
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        self.media_capability(MediaKind::Recording)?;
        if !self
            .capabilities
            .features
            .iter()
            .any(|value| value == "recording_waveforms")
        {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support recording waveforms. Upgrade the server, then reconnect.",
            ));
        }
        MediaRef::Recording(lease).validate()?;
        let id = normalize_id(&lease.id)?;
        let etag = format!("\"sha256:{}\"", lease.metadata.sha256);
        let operation = async {
            let response = self
                .media_request(
                    Method::GET,
                    &["vaults", &lease.vault_id, "audio", "leases", &id, "peaks"],
                    &[("if-match", &etag)],
                )
                .await?;
            if response.status() != StatusCode::OK {
                return Err(media_response_error(response).await);
            }
            if response
                .headers()
                .get("etag")
                .and_then(|value| value.to_str().ok())
                != Some(etag.as_str())
            {
                return Err(invalid_response());
            }
            let waveform: RecordingWaveform = json_body(response, 256 * 1024).await?;
            let peaks = &waveform.peaks;
            if waveform.recording_sha256 != lease.metadata.sha256
                || !peaks.duration_sec.is_finite()
                || peaks.duration_sec <= 0.0
                || peaks.channels.is_empty()
                || peaks.channels.len() > 8
                || peaks.channels.iter().any(|channel| {
                    channel.is_empty()
                        || channel.len() > 1500
                        || channel
                            .iter()
                            .any(|value| !value.is_finite() || *value < 0.0)
                })
            {
                return Err(invalid_response());
            }
            Ok(waveform)
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = self.stop.cancelled() => Err(cancelled()),
            result = operation => result,
        }
    }

    pub async fn open_recording(
        self: &Arc<Self>,
        vault: &str,
        session: &str,
        lease_id: &str,
        cancel: CancellationToken,
    ) -> Result<Option<RecordingLease>> {
        self.check_media_request(MediaKind::Recording, &cancel)?;
        let id = normalize_id(lease_id)?;
        self.open_media(
            &["vaults", vault, "sessions", session, "audio", "leases", &id],
            cancel,
            |lease: &RecordingLease| {
                if lease.id != id || lease.vault_id != vault || lease.session_id != session {
                    return Err(invalid_response());
                }
                MediaRef::Recording(lease).validate()
            },
        )
        .await
    }

    pub async fn open_attachment(
        self: &Arc<Self>,
        vault: &str,
        session: &str,
        attachment: &str,
        lease_id: &str,
        cancel: CancellationToken,
    ) -> Result<Option<AttachmentLease>> {
        self.check_media_request(MediaKind::Attachment, &cancel)?;
        validate_attachment_basename(attachment).map_err(|_| invalid("Invalid attachment ID"))?;
        let id = normalize_id(lease_id)?;
        self.open_media(
            &[
                "vaults",
                vault,
                "sessions",
                session,
                "attachments",
                attachment,
                "leases",
                &id,
            ],
            cancel,
            |lease: &AttachmentLease| {
                if lease.id != id
                    || lease.vault_id != vault
                    || lease.session_id != session
                    || lease.attachment_id != attachment
                {
                    return Err(invalid_response());
                }
                MediaRef::Attachment(lease).validate()
            },
        )
        .await
    }

    async fn open_media<T: serde::de::DeserializeOwned + Send>(
        self: &Arc<Self>,
        segments: &[&str],
        cancel: CancellationToken,
        validate: impl FnOnce(&T) -> Result<()> + Send,
    ) -> Result<Option<T>> {
        let _window_work = self.begin_window_work()?;
        let operation = async {
            let response = self.media_request(Method::PUT, segments, &[]).await?;
            if response.status() != StatusCode::OK {
                return Err(media_response_error(response).await);
            }
            let info: Option<T> = json_body(response, 256 * 1024).await?;
            if let Some(info) = &info {
                validate(info)?;
            }
            Ok(info)
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = self.stop.cancelled() => Err(cancelled()),
            result = operation => result,
        }
    }

    pub async fn close_recording(
        self: &Arc<Self>,
        vault: &str,
        lease_id: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        self.close_media(MediaKind::Recording, vault, lease_id, cancel)
            .await
    }
    pub async fn close_attachment(
        self: &Arc<Self>,
        vault: &str,
        lease_id: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        self.close_media(MediaKind::Attachment, vault, lease_id, cancel)
            .await
    }
    async fn close_media(
        self: &Arc<Self>,
        kind: MediaKind,
        vault: &str,
        lease_id: &str,
        cancel: CancellationToken,
    ) -> Result<()> {
        let _window_work = self.begin_window_work()?;
        self.check_media_request(kind, &cancel)?;
        let id = normalize_id(lease_id)?;
        let operation = async {
            let response = self
                .media_request(
                    Method::DELETE,
                    &["vaults", vault, kind.endpoint(), "leases", &id],
                    &[],
                )
                .await?;
            if response.status() == StatusCode::NO_CONTENT {
                Ok(())
            } else {
                Err(media_response_error(response).await)
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = self.stop.cancelled() => Err(cancelled()),
            result = operation => result,
        }
    }

    pub async fn read_recording_chunk(
        self: &Arc<Self>,
        lease: &RecordingLease,
        offset: u64,
        length: usize,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>> {
        self.read_media_chunk(MediaRef::Recording(lease), offset, length, cancel)
            .await
    }
    pub async fn read_attachment_chunk(
        self: &Arc<Self>,
        lease: &AttachmentLease,
        offset: u64,
        length: usize,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>> {
        self.read_media_chunk(MediaRef::Attachment(lease), offset, length, cancel)
            .await
    }
    async fn read_media_chunk(
        self: &Arc<Self>,
        lease: MediaRef<'_>,
        offset: u64,
        length: usize,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>> {
        let _window_work = self.begin_window_work()?;
        self.check_media_request(lease.kind(), &cancel)?;
        lease.validate()?;
        let (vault, id, metadata) = lease.parts();
        let id = normalize_id(id)?;
        if length == 0 || length > RECORDING_CHUNK_LIMIT || offset >= metadata.size_bytes {
            return Err(invalid("Invalid media byte range"));
        }
        let length = (metadata.size_bytes - offset).min(length as u64);
        let end = offset + length - 1;
        let range = format!("bytes={offset}-{end}");
        let etag = format!("\"sha256:{}\"", metadata.sha256);
        let expected_range = format!("bytes {offset}-{end}/{}", metadata.size_bytes);
        let operation = async {
            let mut response = self
                .media_request(
                    Method::GET,
                    &["vaults", vault, lease.kind().endpoint(), "leases", &id],
                    &[("range", &range), ("if-match", &etag)],
                )
                .await?;
            if response.status() != StatusCode::PARTIAL_CONTENT {
                return Err(media_response_error(response).await);
            }
            if response
                .headers()
                .get("content-range")
                .and_then(|value| value.to_str().ok())
                != Some(expected_range.as_str())
                || response
                    .headers()
                    .get("etag")
                    .and_then(|value| value.to_str().ok())
                    != Some(etag.as_str())
                || response
                    .headers()
                    .get("content-type")
                    .and_then(|value| value.to_str().ok())
                    != Some(metadata.content_type.as_str())
                || response.content_length() != Some(length)
            {
                return Err(invalid_response());
            }
            let mut bytes = Vec::with_capacity(length as usize);
            while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
                if bytes.len().saturating_add(chunk.len()) > length as usize {
                    return Err(invalid_response());
                }
                bytes.extend_from_slice(&chunk);
            }
            if bytes.len() != length as usize {
                return Err(invalid_response());
            }
            Ok(bytes)
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = self.stop.cancelled() => Err(cancelled()),
            result = operation => result,
        }
    }

    fn check_media_request(&self, kind: MediaKind, cancel: &CancellationToken) -> Result<()> {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        self.media_capability(kind)
    }
    fn media_capability(&self, kind: MediaKind) -> Result<()> {
        if self.stop.is_cancelled() {
            return Err(cancelled());
        }
        if !self
            .capabilities
            .features
            .iter()
            .any(|feature| feature == kind.capability())
        {
            let description = match kind {
                MediaKind::Recording => "recording",
                MediaKind::Attachment => "attachment",
            };
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                &format!(
                    "This server does not support {description} reads. Upgrade the server, then reconnect."
                ),
            ));
        }
        Ok(())
    }
    async fn media_request(
        self: &Arc<Self>,
        method: Method,
        segments: &[&str],
        headers: &[(&str, &str)],
    ) -> Result<Response> {
        self.media_request_with_timeout(method, segments, headers, Some(Duration::from_secs(35)))
            .await
    }

    async fn media_request_with_timeout(
        self: &Arc<Self>,
        method: Method,
        segments: &[&str],
        headers: &[(&str, &str)],
        timeout: Option<Duration>,
    ) -> Result<Response> {
        let url = self.endpoint(segments)?;
        let access = self.access_token().await?;
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(&access)
            .header("x-loofah-api-major", API_MAJOR.to_string())
            .header("x-request-id", uuid::Uuid::new_v4().to_string())
            .header("accept-encoding", "identity");
        if let Some(timeout) = timeout {
            request = request.timeout(timeout);
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = self.send_window_request(request).await?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.expire_access(&access).await;
        }
        Ok(response)
    }
}

fn normalize_id(id: &str) -> Result<String> {
    uuid::Uuid::parse_str(id)
        .map(|id| id.to_string())
        .map_err(|_| invalid("Invalid media lease identifier"))
}
async fn media_response_error(response: Response) -> hypr_loofah_protocol::ApplicationError {
    if response.status() == StatusCode::PRECONDITION_FAILED {
        return failure(
            ErrorCode::Conflict,
            "The media version changed; reopen it before continuing",
        );
    }
    if response.status().is_success() {
        return invalid_response();
    }
    json_body(response, 131_072)
        .await
        .unwrap_or_else(|_| unavailable())
}
fn invalid_response() -> hypr_loofah_protocol::ApplicationError {
    failure(
        ErrorCode::IncompatibleVersion,
        "The server returned an invalid media response",
    )
}
fn media_head_error(status: StatusCode) -> hypr_loofah_protocol::ApplicationError {
    match status {
        StatusCode::UNAUTHORIZED => failure(ErrorCode::Unauthenticated, "Sign in again"),
        StatusCode::FORBIDDEN => failure(ErrorCode::Forbidden, "Media access was denied"),
        StatusCode::NOT_FOUND => failure(ErrorCode::NotFound, "Media lease is not available"),
        StatusCode::PRECONDITION_FAILED | StatusCode::CONFLICT => failure(
            ErrorCode::Conflict,
            "Media changed; reopen it before continuing",
        ),
        StatusCode::TOO_MANY_REQUESTS => {
            let mut cause = failure(
                ErrorCode::ResourceLimit,
                "Media readers are busy; retry shortly",
            );
            cause.retryable = true;
            cause
        }
        _ => unavailable(),
    }
}
