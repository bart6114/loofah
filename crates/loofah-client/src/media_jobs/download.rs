use super::*;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncWrite, AsyncWriteExt, BufWriter};

impl MediaAuthority {
    /// The caller supplies an empty private sink and its owner; neither may be
    /// published until verification and a fresh original-authority commit fence.
    pub async fn download_source_with_guard<
        W: AsyncWrite + Unpin + Send + 'static,
        G: Send + 'static,
    >(
        &self,
        submission: &MediaSubmission,
        sink: W,
        owner: G,
    ) -> Result<(W, G)> {
        self.client.media_source_export_capability(&self.cancel)?;
        self.validate_submission(submission)?;
        // A local cleanup decision is not server acceptance; the source endpoint
        // owns the authoritative retirement/read barrier.
        let revision = submission
            .admission_revision
            .as_deref()
            .filter(|revision| valid_revision(revision))
            .ok_or_else(|| bad_reply("Original media admission is not confirmed"))?;
        let (_, source) =
            submission.declaration.operation.upload().ok_or_else(|| {
                failure(ErrorCode::NotFound, "This request has no uploaded source")
            })?;
        let source = source.clone();
        self.check_current().await?;
        let capacity = self
            .client
            .media_source_readers
            .clone()
            .try_acquire_owned()
            .map_err(|_| failure(ErrorCode::Unavailable, "Original media downloads are busy"))?;
        let guard = self.guard().await?;
        let request = self
            .client
            .http
            .get(self.client.endpoint(&[
                "vaults",
                &self.scope().vault_id,
                "media",
                "requests",
                &submission.declaration.request_id,
                "source",
            ])?)
            .bearer_auth(&guard.access)
            .header("x-loofah-api-major", API_MAJOR.to_string())
            .header("x-request-id", &submission.declaration.request_id)
            .header("x-loofah-media-revision", revision)
            .header("accept-encoding", "identity");
        let revision = revision.to_owned();
        let client = self.client.clone();
        let lifetime = self.lifetime.clone();
        let pin = self.pin_source();
        let cancel = self.cancel.child_token();
        let _cancel_on_drop = cancel.clone().drop_guard();
        guard.check_current()?;
        let window_work = self.client.begin_window_work()?;
        let transfer = tokio::spawn(async move {
            let _window_work = window_work;
            let _capacity = capacity;
            let _pin = pin.clone();
            let mut writer = BufWriter::with_capacity(RECORDING_CHUNK_LIMIT, sink);
            let copied = async {
                let mut response = tokio::select! {
                    _ = cancel.cancelled() => return Err(crate::cancelled()),
                    _ = client.stop.cancelled() => return Err(crate::cancelled()),
                    _ = lifetime.cancelled() => return Err(crate::cancelled()),
                    result = tokio::time::timeout(Duration::from_secs(300), guard.send(request)) => result.map_err(|_|crate::unavailable())??,
                };
                if response.status() != reqwest::StatusCode::OK {
                    return Err(tokio::time::timeout(Duration::from_secs(35), json_body::<ApplicationError>(response, 131_072)).await
                        .unwrap_or_else(|_| Err(crate::unavailable()))
                        .unwrap_or_else(|_| crate::unavailable()));
                }
                let headers = response.headers();
                if headers.get("x-loofah-media-revision").and_then(|value| value.to_str().ok()) != Some(revision.as_str())
                    || headers.get("x-loofah-media-sha256").and_then(|value| value.to_str().ok()) != Some(source.sha256.as_str())
                    || headers.get("content-type").and_then(|value| value.to_str().ok()) != Some("application/octet-stream")
                    || headers.contains_key("content-range")
                    || headers.get("content-encoding").is_some_and(|value| value != "identity")
                    || response.content_length() != Some(source.size_bytes)
                {
                    return Err(bad_reply("Original media source response changed its admitted descriptor"));
                }
                let mut bytes = 0u64;
                let mut hash = Sha256::new();
                loop {
                    pin.check_current()?;
                    let chunk = tokio::select! {
                        _ = cancel.cancelled() => return Err(crate::cancelled()),
                        _ = client.stop.cancelled() => return Err(crate::cancelled()),
                    _ = lifetime.cancelled() => return Err(crate::cancelled()),
                        result = tokio::time::timeout(Duration::from_secs(35), response.chunk()) => result.map_err(|_|crate::unavailable())?.map_err(|_|crate::unavailable())?,
                    };
                    let Some(chunk) = chunk else { break; };
                    bytes = bytes.checked_add(chunk.len() as u64).ok_or_else(||bad_reply("Original source grew beyond its admitted size"))?;
                    if bytes > source.size_bytes { return Err(bad_reply("Original source grew beyond its admitted size")); }
                    for part in chunk.chunks(RECORDING_CHUNK_LIMIT) {
                        pin.check_current()?;
                        if cancel.is_cancelled() { return Err(crate::cancelled()); }
                        writer.write_all(part).await.map_err(sink_error)?;
                        hash.update(part);
                    }
                }
                let actual = hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
                if bytes != source.size_bytes || actual != source.sha256 {
                    return Err(bad_reply("Downloaded media does not match the original admitted source"));
                }
                Ok(())
            }.await;
            // A Tokio file write may still own blocking I/O after write_all returns.
            // Drain on success, corruption and cancellation before releasing its private owner.
            let drained = writer.flush().await.map_err(sink_error);
            copied?;
            drained?;
            pin.check_current()?;
            if cancel.is_cancelled() {
                return Err(crate::cancelled());
            }
            Ok((writer.into_inner(), owner))
        });
        let result = transfer.await.map_err(|_| {
            failure(
                ErrorCode::Internal,
                "Original media source download worker stopped",
            )
        })??;
        self.check_current().await?;
        Ok(result)
    }
}
fn sink_error(cause: std::io::Error) -> ApplicationError {
    tracing::error!(%cause,"Original media source copy failed");
    failure(
        ErrorCode::Internal,
        "Could not save the original media source copy",
    )
}
