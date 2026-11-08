use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use tokio::io::{AsyncRead, AsyncReadExt};
#[derive(Clone)]
struct ReceiveCompletion {
    _owner: Arc<ReceiveFinished>,
}
struct ReceiveFinished(tokio::sync::watch::Sender<bool>);
impl Drop for ReceiveFinished {
    fn drop(&mut self) {
        self.0.send_replace(true);
    }
}
enum SourcePart {
    Bytes(Vec<u8>),
    Complete,
}

impl MediaDispatcher {
    pub async fn source(&self, session: &str) -> Result<MediaSessionSnapshot, ApplicationError> {
        let _authorization = self.authorization.authorize().await?;
        let _pin = self.context.admit_session_work(session).await?;
        self.context.ensure_recording_publication_ready(session)?;
        let metadata = self.context.read_metadata(session).await?;
        let source = subtitles::subtitle_source(&self.context, session, "0".repeat(64)).await?;
        if source.session_created_at() != metadata.meta.created_at {
            return Err(conflict("Media session changed"));
        }
        let recording = match LocalCli::new(self.context.clone())?
            .media_source(session)
            .await
        {
            Ok(recording) => Some(recording),
            Err(cause) if cause.code == ErrorCode::NotFound => None,
            Err(cause) => return Err(cause),
        };
        let snapshot = MediaSessionSnapshot {
            metadata,
            transcript_revision: source.transcript_revision().into(),
            transcript_present: source.transcript_present(),
            recording,
        };
        validate_media_session_snapshot(&snapshot)
            .map_err(|_| conflict("Media source changed while reading"))?;
        Ok(snapshot)
    }
    pub async fn receive<R: AsyncRead + Unpin + Send + 'static>(
        self: &Arc<Self>,
        request: String,
        revision: String,
        mut reader: R,
    ) -> Result<MediaRequestStatus, ApplicationError> {
        let owner = self.clone();
        tokio::spawn(async move {
            let _gate = owner.request_gate(&request).await?;
            let mut record = owner.load(&request).await?.ok_or_else(|| error(ErrorCode::NotFound, "Media request not found"))?;
            if record.revision != revision { return Err(conflict("Original media admission revision changed")); }
            if record.phase != MediaPhase::Receiving { return owner.status(&request).await; }
            if owner.stop.is_cancelled() { return Err(unavailable("Media service is stopping")); }
            let (finished, done) = tokio::sync::watch::channel(false);
            let completion = ReceiveCompletion { _owner: Arc::new(ReceiveFinished(finished)) };
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
            let (send, mut receive) = tokio::sync::mpsc::channel::<SourcePart>(2);
            let expected_source = expected.clone();
            let writer_owner = owner.context.clone();
            let writer_completion = completion.clone();
            #[cfg(test)]
            let mut pause = owner.source_write_pause.lock().unwrap().take();
            let writer = tokio::task::spawn_blocking(move || {
                let _owner = writer_owner;
                let _completion = writer_completion;
                let mut file = file;
                let mut digest = Sha256::new();
                let mut size = 0_u64;
                loop {
                    match receive.blocking_recv() {
                        Some(SourcePart::Bytes(bytes)) => {
                            #[cfg(test)]
                            if let Some((entered, release)) = pause.take() {
                                let _ = entered.send(()); let _ = release.recv();
                            }
                            size = size.checked_add(bytes.len() as u64).filter(|size| *size <= expected_source.size_bytes)
                                .ok_or_else(|| conflict("Media source exceeds its immutable declaration"))?;
                            digest.update(&bytes);
                            file.write_all(&bytes).map_err(|_| unavailable("Media source staging failed"))?;
                        }
                        Some(SourcePart::Complete) => {
                            if size != expected_source.size_bytes || ledger::hex(&digest.finalize()) != expected_source.sha256 {
                                return Err(conflict("Media source differs from its immutable declaration"));
                            }
                            file.sync_all().map_err(|_| unavailable("Media source sync failed"))?;
                            return Ok(());
                        }
                        None => return Err(conflict("Media source stream incomplete")),
                    }
                }
            });
            let streamed: Result<(), ApplicationError> = async {
                let mut buffer = vec![0; 64 * 1024];
                loop {
                    let read = tokio::select! {
                        _ = receiving.cancelled() => return Err(unavailable("Media upload interrupted")),
                        _ = access.cancelled.cancelled() => return Err(error(ErrorCode::Forbidden, "Media access revoked")),
                        _ = access.interrupted.cancelled() => return Err(unavailable("Media upload interrupted")),
                        read = reader.read(&mut buffer) => read.map_err(|_| conflict("Media source stream incomplete"))?,
                    };
                    let part = if read == 0 { SourcePart::Complete } else { SourcePart::Bytes(buffer[..read].to_vec()) };
                    tokio::select! {
                        _ = receiving.cancelled() => return Err(unavailable("Media upload interrupted")),
                        _ = access.cancelled.cancelled() => return Err(error(ErrorCode::Forbidden, "Media access revoked")),
                        _ = access.interrupted.cancelled() => return Err(unavailable("Media upload interrupted")),
                        sent = send.send(part) => sent.map_err(|_| unavailable("Media source writer stopped"))?,
                    }
                    if read == 0 { break; }
                }
                Ok(())
            }.await;
            drop(send);
            let stored = writer.await.map_err(|_| unavailable("Media source writer requires review"))?;
            streamed?;
            stored?;
            if receiving.is_cancelled() { return Err(error(ErrorCode::Cancelled, "Media upload stopped after source writer drained")); }
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
    pub(super) async fn artifact(
        &self,
        record: &Admission,
    ) -> Result<Option<LocalCliArtifact>, ApplicationError> {
        self.artifact_with_guard(record, ())
            .await
            .map(|source| source.map(|(artifact, _)| artifact))
    }
    async fn artifact_with_guard<G: Send + 'static>(
        &self,
        record: &Admission,
        guard: G,
    ) -> Result<Option<(LocalCliArtifact, Vec<[u8; 32]>)>, ApplicationError> {
        let Some((_, expected)) = record.declaration.operation.upload() else {
            return Ok(None);
        };
        let expected = expected.clone();
        let directory = self
            .root
            .join(format!("{}.source", record.declaration.request_id));
        let pin = self.context.clone();
        let stop = self.stop.child_token();
        let _cancel_verification_on_drop = stop.clone().drop_guard();
        tokio::task::spawn_blocking(move || {
            let owner = (pin, guard);
            if !ledger::existing_directory(&directory)?
                || !ledger::check_file(&directory.join("source"))?
            {
                return Err(conflict("Original verified media source unavailable"));
            }
            let path = directory.join("source");
            let mut file = std::fs::File::open(path)
                .map_err(|_| conflict("Original media source unavailable"))?;
            if file
                .metadata()
                .map_err(|_| conflict("Original media source unavailable"))?
                .len()
                != expected.size_bytes
            {
                return Err(conflict("Original media source length changed"));
            }
            let mut digest = Sha256::new();
            let mut chunks = Vec::new();
            let mut remaining = expected.size_bytes;
            let mut buffer = [0; 64 * 1024];
            while remaining > 0 {
                if stop.is_cancelled() {
                    return Err(unavailable("Media service is stopping"));
                }
                let count = remaining.min(buffer.len() as u64) as usize;
                file.read_exact(&mut buffer[..count])
                    .map_err(|_| conflict("Original media source unavailable"))?;
                digest.update(&buffer[..count]);
                chunks.push(Sha256::digest(&buffer[..count]).into());
                remaining -= count as u64;
            }
            if ledger::hex(&digest.finalize()) != expected.sha256 {
                return Err(conflict("Original media source changed"));
            }
            file.rewind()
                .map_err(|_| unavailable("Original media source unavailable"))?;
            Ok(Some((
                LocalCliArtifact::new(
                    file,
                    expected.size_bytes,
                    expected.sha256,
                    None,
                    Box::new(owner),
                ),
                chunks,
            )))
        })
        .await
        .map_err(|_| unavailable("Media source verifier stopped"))?
    }
}

#[cfg(test)]
type ReadPause = (
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);

#[derive(Clone)]
pub struct MediaSourceReader {
    source: Arc<std::sync::Mutex<LocalCliArtifact>>,
    chunks: Arc<Vec<[u8; 32]>>,
    owner: Arc<MediaDispatcher>,
    size_bytes: u64,
    sha256: String,
    #[cfg(test)]
    pub(super) read_pause: Arc<std::sync::Mutex<Option<ReadPause>>>,
}
impl MediaDispatcher {
    pub async fn open_source(
        self: &Arc<Self>,
        request: &str,
        revision: &str,
    ) -> Result<MediaSourceReader, ApplicationError> {
        let permit = self
            .source_reads
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable("Media source reader capacity reached"))?;
        if self.stop.is_cancelled() {
            return Err(unavailable("Media service is stopping"));
        }
        let access = self.authorization.observe().await?;
        let record = self
            .load(request)
            .await?
            .ok_or_else(|| error(ErrorCode::NotFound, "Media request not found"))?;
        if record.revision != revision {
            return Err(conflict("Original media admission revision changed"));
        }
        let verified = tokio::select! {
            _ = access.cancelled.cancelled() => return Err(error(ErrorCode::Forbidden, "Media access revoked")),
            _ = access.interrupted.cancelled() => return Err(unavailable("Media source verification interrupted")),
            result = self.artifact_with_guard(&record, permit) => result,
        };
        let (source, chunks) = verified
            .map_err(|_| unavailable("Original verified media source unavailable"))?
            .ok_or_else(|| unavailable("This media request has no retained upload"))?;
        drop(self.authorization.authorize().await?);
        Ok(MediaSourceReader {
            #[cfg(test)]
            read_pause: Default::default(),
            chunks: Arc::new(chunks),
            size_bytes: source.size_bytes,
            sha256: source.sha256.clone(),
            source: Arc::new(std::sync::Mutex::new(source)),
            owner: self.clone(),
        })
    }
}
impl MediaSourceReader {
    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }
    pub async fn read_chunk_with_guard<G: Send + 'static>(
        &self,
        offset: u64,
        length: usize,
        guard: G,
    ) -> Result<Vec<u8>, ApplicationError> {
        if length == 0
            || length > 64 * 1024
            || offset >= self.size_bytes
            || !offset.is_multiple_of(64 * 1024)
        {
            return Err(error(ErrorCode::Validation, "Invalid media source range"));
        }
        if self.owner.stop.is_cancelled() {
            return Err(unavailable("Media service is stopping"));
        }
        let authorization = self.owner.authorization.authorize().await?;
        let source = self.source.clone();
        let requested = length;
        let length = (self.size_bytes - offset).min(64 * 1024) as usize;
        let expected = self.chunks[(offset / (64 * 1024)) as usize];
        #[cfg(test)]
        let pause = self.read_pause.lock().unwrap().take();
        tokio::task::spawn_blocking(move || {
            let _guard = (guard, authorization);
            #[cfg(test)]
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _ = release.recv();
            }
            let mut source = source
                .lock()
                .map_err(|_| unavailable("Media source reader stopped"))?;
            source
                .seek(SeekFrom::Start(offset))
                .map_err(|_| unavailable("Media source read failed"))?;
            let mut bytes = vec![0; length];
            source
                .read_exact(&mut bytes)
                .map_err(|_| unavailable("Media source read failed"))?;
            if <[u8; 32]>::from(Sha256::digest(&bytes)) != expected {
                return Err(unavailable("Original media source changed"));
            }
            bytes.truncate(requested);
            Ok(bytes)
        })
        .await
        .map_err(|_| unavailable("Media source reader stopped"))?
    }
}
