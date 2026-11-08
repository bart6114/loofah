use super::*;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
struct ReceiveCompletion(tokio::sync::watch::Sender<bool>);
impl Drop for ReceiveCompletion { fn drop(&mut self) { self.0.send_replace(true); } }

impl MediaDispatcher {
    pub async fn source(&self, session: &str) -> Result<MediaSessionSnapshot, ApplicationError> {
        let _authorization = self.authorization.authorize().await?;
        let _pin = self.context.admit_session_work(session).await?;
        self.context.ensure_recording_publication_ready(session)?;
        let metadata = self.context.read_metadata(session).await?;
        let source = subtitles::subtitle_source(&self.context, session, "0".repeat(64)).await?;
        if source.session_created_at() != metadata.meta.created_at { return Err(conflict("Media session changed")); }
        let recording = match LocalCli::new(self.context.clone())?.media_source(session).await {
            Ok(recording) => Some(recording),
            Err(cause) if cause.code == ErrorCode::NotFound => None,
            Err(cause) => return Err(cause),
        };
        let snapshot = MediaSessionSnapshot {
            metadata, transcript_revision: source.transcript_revision().into(),
            transcript_present: source.transcript_present(), recording,
        };
        validate_media_session_snapshot(&snapshot).map_err(|_| conflict("Media source changed while reading"))?;
        Ok(snapshot)
    }
    pub async fn receive<R: AsyncRead + Unpin + Send + 'static>(
        self: &Arc<Self>, request: String, revision: String, mut reader: R,
    ) -> Result<MediaRequestStatus, ApplicationError> {
        let owner = self.clone();
        tokio::spawn(async move {
            let _gate = owner.request_gate(&request).await?;
            let mut record = owner.load(&request).await?.ok_or_else(|| error(ErrorCode::NotFound, "Media request not found"))?;
            if record.revision != revision { return Err(conflict("Original media admission revision changed")); }
            if record.phase != MediaPhase::Receiving { return owner.status(&request).await; }
            if owner.stop.is_cancelled() { return Err(unavailable("Media service is stopping")); }
            let (finished, done) = tokio::sync::watch::channel(false);
            let _completion = ReceiveCompletion(finished);
            let receiving = owner.stop.child_token();
            {
                let mut receivers = owner.receivers.lock().await;
                if owner.stop.is_cancelled() { return Err(unavailable("Media service is stopping")); }
                receivers.insert(request.clone(), (receiving.clone(), done));
            }
            let access = owner.authorization.observe().await?;
            let (_, expected) = record.declaration.operation.upload().ok_or_else(|| conflict("This media request has no upload"))?;
            let expected = expected.clone();
            let directory = owner.root.join(format!("{request}.source"));
            let stage = directory.join("partial");
            let pin = owner.context.clone();
            let create_directory = directory.clone();
            let create_stage = stage.clone();
            let file = tokio::task::spawn_blocking(move || {
                let _pin = pin;
                ledger::ensure_directory(&create_directory)?;
                if ledger::check_file(&create_stage)? { std::fs::remove_file(&create_stage).map_err(|_| unavailable("Cannot retire interrupted media upload"))?; }
                let mut options = std::fs::OpenOptions::new(); options.create_new(true).write(true);
                #[cfg(unix)] { use std::os::unix::fs::OpenOptionsExt; options.mode(0o600); }
                options.open(create_stage).map_err(|_| unavailable("Media source staging failed"))
            }).await.map_err(|_| unavailable("Media source owner stopped"))??;
            let mut file = tokio::fs::File::from_std(file);
            let mut digest = Sha256::new(); let mut size = 0_u64; let mut buffer = vec![0; 64 * 1024];
            loop {
                let read = tokio::select! {
                    _ = receiving.cancelled() => return Err(unavailable("Media upload interrupted")),
                    _ = access.cancelled.cancelled() => return Err(error(ErrorCode::Forbidden, "Media access revoked")),
                    _ = access.interrupted.cancelled() => return Err(unavailable("Media upload interrupted")),
                    read = reader.read(&mut buffer) => read.map_err(|_| conflict("Media source stream incomplete"))?,
                };
                if read == 0 { break; }
                size = size.checked_add(read as u64).filter(|size| *size <= expected.size_bytes)
                    .ok_or_else(|| error(ErrorCode::Validation, "Media source exceeds its declaration"))?;
                digest.update(&buffer[..read]);
                file.write_all(&buffer[..read]).await.map_err(|_| unavailable("Media source staging failed"))?;
            }
            if size != expected.size_bytes || ledger::hex(&digest.finalize()) != expected.sha256 {
                return Err(conflict("Media source differs from its immutable declaration"));
            }
            file.sync_all().await.map_err(|_| unavailable("Media source sync failed"))?;
            drop(file);
            let _authorization = owner.authorization.authorize().await?;
            let complete = directory.join("source");
            let pin = owner.context.clone();
            tokio::task::spawn_blocking(move || {
                let _pin = pin;
                if ledger::check_file(&complete)? { return Err(conflict("Verified media source already exists")); }
                hypr_storage::fs::rename_no_replace(&stage, &complete).map_err(|_| unavailable("Media source retention failed"))?;
                ledger::sync_parent(&complete)
            }).await.map_err(|_| unavailable("Media source owner stopped"))??;
            record.phase = MediaPhase::Ready;
            owner.save(record.clone(), false).await?;
            drop(_authorization); drop(access);
            owner.dispatch(record).await?;
            owner.status(&request).await
        }).await.map_err(|_| unavailable("Media receiving requires review"))?
    }
    pub(super) async fn artifact(&self, record: &Admission) -> Result<Option<LocalCliArtifact>, ApplicationError> {
        let Some((_, expected)) = record.declaration.operation.upload() else { return Ok(None); };
        let expected = expected.clone();
        let directory = self.root.join(format!("{}.source", record.declaration.request_id));
        let pin = self.context.clone();
        tokio::task::spawn_blocking(move || {
            if !ledger::existing_directory(&directory)? || !ledger::check_file(&directory.join("source"))? {
                return Err(conflict("Original verified media source unavailable"));
            }
            let path = directory.join("source");
            crate::capture_append::verify_file_sha256(&path, &expected.sha256)?;
            let file = std::fs::File::open(path).map_err(|_| conflict("Original media source unavailable"))?;
            if file.metadata().map_err(|_| conflict("Original media source unavailable"))?.len() != expected.size_bytes {
                return Err(conflict("Original media source length changed"));
            }
            Ok(Some(LocalCliArtifact::new(file, expected.size_bytes, expected.sha256, None, Box::new(pin))))
        }).await.map_err(|_| unavailable("Media source verifier stopped"))?
    }
}
