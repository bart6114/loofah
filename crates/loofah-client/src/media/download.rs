use super::*;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter};

impl RemoteClient {
    /// Provide an empty sink and keep it unpublished until this returns a verified result.
    pub async fn download_attachment<W: AsyncWrite + Unpin + Send + 'static>(
        self: &Arc<Self>,
        lease: AttachmentLease,
        sink: W,
        cancel: CancellationToken,
    ) -> Result<W> {
        self.download_attachment_with_guard(lease, sink, (), cancel)
            .await
            .map(|(sink, ())| sink)
    }

    /// Admission and temporary-file ownership outlive cancelled callers and in-flight writes.
    pub async fn download_attachment_with_guard<
        W: AsyncWrite + Unpin + Send + 'static,
        G: Send + 'static,
    >(
        self: &Arc<Self>,
        lease: AttachmentLease,
        sink: W,
        guard: G,
        cancel: CancellationToken,
    ) -> Result<(W, G)> {
        self.download_attachment_inner(lease, sink, guard, cancel, Duration::from_secs(35))
            .await
    }

    pub(crate) async fn download_attachment_inner<
        W: AsyncWrite + Unpin + Send + 'static,
        G: Send + 'static,
    >(
        self: &Arc<Self>,
        lease: AttachmentLease,
        sink: W,
        guard: G,
        cancel: CancellationToken,
        idle_timeout: Duration,
    ) -> Result<(W, G)> {
        self.check_media_request(MediaKind::Attachment, &cancel)?;
        MediaRef::Attachment(&lease).validate()?;
        normalize_id(&lease.id)?;
        let client = self.clone();
        let cancel = cancel.child_token();
        let _on_drop = cancel.clone().drop_guard();
        tokio::spawn(async move {
            let mut writer = BufWriter::with_capacity(RECORDING_CHUNK_LIMIT, sink);
            let result = transfer(&client, &lease, &mut writer, &cancel, idle_timeout).await;
            // Tokio file writes can outlive their write future. Drain before releasing the guard.
            let result = match writer.flush().await {
                Ok(()) => result,
                Err(cause) => Err(sink_error(cause)),
            };
            let result = if cancel.is_cancelled() || client.stop.is_cancelled() {
                Err(cancelled())
            } else {
                result
            };
            match result {
                Ok(()) => Ok((writer.into_inner(), guard)),
                Err(cause) => {
                    drop(writer);
                    drop(guard);
                    Err(cause)
                }
            }
        })
        .await
        .map_err(|_| failure(ErrorCode::Internal, "Attachment download stopped"))?
    }
}

#[derive(Default)]
struct TransferState {
    offset: u64,
    hash: Sha256,
}

async fn transfer<W: AsyncWrite + Unpin>(
    client: &Arc<RemoteClient>,
    lease: &AttachmentLease,
    sink: &mut W,
    cancel: &CancellationToken,
    idle_timeout: Duration,
) -> Result<()> {
    let mut state = TransferState::default();
    let mut resumes = 0;
    let mut reopened = false;
    loop {
        let result = transfer_response(client, lease, sink, cancel, idle_timeout, &mut state).await;
        match result {
            Ok(()) => {
                let observed: String = state
                    .hash
                    .finalize()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                if observed != lease.metadata.sha256 {
                    return Err(failure(
                        ErrorCode::Conflict,
                        "Downloaded attachment did not match its revision; reload before retrying",
                    ));
                }
                return Ok(());
            }
            Err(cause) if cause.code == ErrorCode::NotFound && !reopened => {
                reopened = true;
                let current = client
                    .open_attachment(
                        &lease.vault_id,
                        &lease.session_id,
                        &lease.attachment_id,
                        &lease.id,
                        cancel.clone(),
                    )
                    .await?
                    .ok_or_else(|| {
                        failure(ErrorCode::NotFound, "Attachment is no longer available")
                    })?;
                if current.metadata != lease.metadata {
                    return Err(failure(
                        ErrorCode::Conflict,
                        "Attachment changed; reload before downloading",
                    ));
                }
            }
            Err(cause)
                if resumes < 2
                    && (matches!(
                        cause.code,
                        ErrorCode::Unavailable | ErrorCode::Unauthenticated
                    ) || (cause.code == ErrorCode::ResourceLimit && cause.retryable)) =>
            {
                let delay = Duration::from_millis(250 * (1 << resumes));
                resumes += 1;
                tokio::select! {
                    _ = cancel.cancelled() => return Err(cancelled()),
                    _ = client.stop.cancelled() => return Err(cancelled()),
                    _ = tokio::time::sleep(delay) => {},
                }
            }
            Err(cause) => return Err(cause),
        }
    }
}

async fn transfer_response<W: AsyncWrite + Unpin>(
    client: &Arc<RemoteClient>,
    lease: &AttachmentLease,
    sink: &mut W,
    cancel: &CancellationToken,
    idle_timeout: Duration,
    state: &mut TransferState,
) -> Result<()> {
    let metadata = &lease.metadata;
    let etag = format!("\"sha256:{}\"", metadata.sha256);
    let range = format!("bytes={}-", state.offset);
    let resumed = state.offset != 0;
    let mut headers = vec![("if-match", etag.as_str())];
    if resumed {
        headers.push(("range", range.as_str()));
    }
    let segments = [
        "vaults",
        &lease.vault_id,
        "attachments",
        "leases",
        &lease.id,
    ];
    let response = client.media_request_with_timeout(Method::GET, &segments, &headers, None);
    let mut response = tokio::select! {
        _ = cancel.cancelled() => return Err(cancelled()),
        _ = client.stop.cancelled() => return Err(cancelled()),
        result = tokio::time::timeout(idle_timeout, response) => result.map_err(|_| unavailable())??,
    };
    let status = if resumed {
        StatusCode::PARTIAL_CONTENT
    } else {
        StatusCode::OK
    };
    if response.status() != status {
        return tokio::select! {
            _ = cancel.cancelled() => Err(cancelled()),
            _ = client.stop.cancelled() => Err(cancelled()),
            result = tokio::time::timeout(idle_timeout, media_response_error(response)) => Err(result.unwrap_or_else(|_| unavailable())),
        };
    }
    if response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        != Some(etag.as_str())
        || response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            != Some(metadata.content_type.as_str())
        || response.content_length() != Some(metadata.size_bytes - state.offset)
        || response
            .headers()
            .get("content-encoding")
            .is_some_and(|value| value != "identity")
    {
        return Err(invalid_response());
    }
    if resumed {
        let expected = format!(
            "bytes {}-{}/{}",
            state.offset,
            metadata.size_bytes - 1,
            metadata.size_bytes
        );
        if response
            .headers()
            .get("content-range")
            .and_then(|value| value.to_str().ok())
            != Some(expected.as_str())
        {
            return Err(invalid_response());
        }
    } else if response.headers().contains_key("content-range") {
        return Err(invalid_response());
    }
    let mut deadline = tokio::time::Instant::now() + idle_timeout;
    loop {
        if state.offset == metadata.size_bytes {
            return Ok(());
        }
        let chunk = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            _ = client.stop.cancelled() => return Err(cancelled()),
            result = tokio::time::timeout_at(deadline, response.chunk()) => result.map_err(|_| unavailable())?.map_err(|_| unavailable())?,
        };
        let Some(chunk) = chunk else {
            return Err(unavailable());
        };
        if chunk.len() as u64 > metadata.size_bytes - state.offset {
            return Err(invalid_response());
        }
        for bytes in chunk.chunks(RECORDING_CHUNK_LIMIT) {
            if cancel.is_cancelled() || client.stop.is_cancelled() {
                return Err(cancelled());
            }
            sink.write_all(bytes).await.map_err(sink_error)?;
            state.hash.update(bytes);
            state.offset += bytes.len() as u64;
        }
        if !chunk.is_empty() {
            deadline = tokio::time::Instant::now() + idle_timeout;
        }
    }
}

fn sink_error(cause: std::io::Error) -> hypr_loofah_protocol::ApplicationError {
    tracing::error!(%cause, "cannot write attachment download");
    failure(
        ErrorCode::Internal,
        "Could not write the downloaded attachment",
    )
}
