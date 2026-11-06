use std::{sync::Arc, time::Duration};

use hypr_loofah_protocol::{API_MAJOR, ErrorCode, RECORDING_CHUNK_LIMIT, RecordingLease};
use reqwest::{Method, Response, StatusCode};
use tokio_util::sync::CancellationToken;

use crate::{RemoteClient, Result, cancelled, failure, invalid, json_body, unavailable};

impl RemoteClient {
    pub async fn open_recording(
        self: &Arc<Self>,
        vault: &str,
        session: &str,
        lease_id: &str,
        cancel: CancellationToken,
    ) -> Result<Option<RecordingLease>> {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        self.recording_capability()?;
        let lease_id = normalize_id(lease_id)?;
        let operation = async {
            let response = self
                .recording_request(
                    Method::PUT,
                    &[
                        "vaults", vault, "sessions", session, "audio", "leases", &lease_id,
                    ],
                    &[],
                )
                .await?;
            if response.status() != StatusCode::OK {
                return Err(recording_response_error(response).await);
            }
            let info: Option<RecordingLease> = json_body(response, 256 * 1024).await?;
            if let Some(info) = &info {
                if info.id != lease_id || info.vault_id != vault || info.session_id != session {
                    return Err(invalid_response());
                }
                validate_metadata(info)?;
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
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        self.recording_capability()?;
        let lease_id = normalize_id(lease_id)?;
        let operation = async {
            let response = self
                .recording_request(
                    Method::DELETE,
                    &["vaults", vault, "audio", "leases", &lease_id],
                    &[],
                )
                .await?;
            if response.status() == StatusCode::NO_CONTENT {
                return Ok(());
            }
            Err(recording_response_error(response).await)
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = self.stop.cancelled() => Err(cancelled()),
            result = operation => result,
        }
    }

    /// Returns bounded native bytes for a host media adapter without renderer credentials.
    pub async fn read_recording_chunk(
        self: &Arc<Self>,
        lease: &RecordingLease,
        offset: u64,
        length: usize,
        cancel: CancellationToken,
    ) -> Result<Vec<u8>> {
        if cancel.is_cancelled() {
            return Err(cancelled());
        }
        self.recording_capability()?;
        validate_metadata(lease)?;
        let id = normalize_id(&lease.id)?;
        if length == 0 || length > RECORDING_CHUNK_LIMIT || offset >= lease.metadata.size_bytes {
            return Err(invalid("Invalid recording byte range"));
        }
        let length = (lease.metadata.size_bytes - offset).min(length as u64);
        let end = offset + length - 1;
        let range = format!("bytes={offset}-{end}");
        let etag = format!("\"sha256:{}\"", lease.metadata.sha256);
        let expected_range = format!("bytes {offset}-{end}/{}", lease.metadata.size_bytes);
        let operation = async {
            let mut response = self
                .recording_request(
                    Method::GET,
                    &["vaults", &lease.vault_id, "audio", "leases", &id],
                    &[("range", &range), ("if-match", &etag)],
                )
                .await?;
            if response.status() != StatusCode::PARTIAL_CONTENT {
                return Err(recording_response_error(response).await);
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

    fn recording_capability(&self) -> Result<()> {
        if self.stop.is_cancelled() {
            return Err(cancelled());
        }
        if !self
            .capabilities
            .features
            .iter()
            .any(|feature| feature == "recording_leases")
        {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support recording reads. Upgrade the server, then reconnect.",
            ));
        }
        Ok(())
    }

    async fn recording_request(
        self: &Arc<Self>,
        method: Method,
        segments: &[&str],
        headers: &[(&str, &str)],
    ) -> Result<Response> {
        let url = self.endpoint(segments)?;
        let access = self.access_token().await?;
        let mut request = self
            .http
            .request(method, url)
            .bearer_auth(&access)
            .header("x-loofah-api-major", API_MAJOR.to_string())
            .header("x-request-id", uuid::Uuid::new_v4().to_string())
            .header("accept-encoding", "identity")
            .timeout(Duration::from_secs(35));
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        let response = request.send().await.map_err(|_| unavailable())?;
        if response.status() == StatusCode::UNAUTHORIZED {
            self.expire_access(&access).await;
        }
        Ok(response)
    }
}

fn normalize_id(id: &str) -> Result<String> {
    uuid::Uuid::parse_str(id)
        .map(|id| id.to_string())
        .map_err(|_| invalid("Invalid recording lease identifier"))
}
fn validate_metadata(lease: &RecordingLease) -> Result<()> {
    let mime = match lease.metadata.filename.as_str() {
        "audio.mp3" => "audio/mpeg",
        "audio.wav" => "audio/wav",
        "audio.ogg" => "audio/ogg",
        _ => return Err(invalid_response()),
    };
    if lease.metadata.content_type != mime
        || lease.metadata.sha256.len() != 64
        || !lease
            .metadata
            .sha256
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
        || lease.idle_timeout_secs == 0
    {
        return Err(invalid_response());
    }
    Ok(())
}
async fn recording_response_error(response: Response) -> hypr_loofah_protocol::ApplicationError {
    if response.status() == StatusCode::PRECONDITION_FAILED {
        return failure(
            ErrorCode::Conflict,
            "The recording version changed; reopen it before continuing",
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
        "The server returned an invalid recording response",
    )
}
