//! Upload ownership is independent of microphone/device ownership. Only existing
//! durable chunks leave the device; no uploader path removes or rewrites them.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::FutureExt;
use hypr_loofah_protocol::*;
use sha2::{Digest, Sha256};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

use crate::{
    RemoteClient, RemoteOperation, RemoteReply, Result,
    capture_spool::{CaptureSnapshot, CaptureWriter},
    failure,
};

#[derive(Clone, Debug)]
pub struct CaptureUploadBinding {
    pub profile_id: String,
    pub scope: CaptureScope,
    pub credential_account: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureUploadPhase {
    Queued,
    WaitingForAudio,
    Uploading,
    Retrying,
    AuthenticationRequired,
    PendingSession,
    AppendUnsupported,
    Verified,
    NeedsReview,
    Stopped,
}

#[derive(Clone, Debug, serde::Serialize, specta::Type)]
pub struct CaptureUploadProgress {
    pub phase: CaptureUploadPhase,
    pub acknowledged_chunks: u64,
    pub publication: Option<CapturePublication>,
    pub error: Option<ApplicationError>,
}

struct Control {
    stop: CancellationToken,
    ended: CancellationToken,
    finished: CancellationToken,
    progress: watch::Receiver<CaptureUploadProgress>,
}
impl Drop for Control {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

#[derive(Clone)]
pub struct CaptureUploadTicket(Arc<Control>);
impl CaptureUploadTicket {
    pub fn current(&self) -> CaptureUploadProgress {
        self.0.progress.borrow().clone()
    }
    pub fn subscribe(&self) -> watch::Receiver<CaptureUploadProgress> {
        self.0.progress.clone()
    }
    pub fn request_stop(&self) {
        self.0.stop.cancel();
    }
    pub fn source_finished(&self) {
        self.0.ended.cancel();
    }
    pub fn is_finished(&self) -> bool {
        self.0.finished.is_cancelled()
    }
    pub async fn wait_stopped(&self) -> CaptureUploadProgress {
        self.0.finished.cancelled().await;
        self.current()
    }
}

struct State {
    closing: bool,
    active: HashMap<String, (CancellationToken, CancellationToken)>,
}
struct UploaderInner {
    slots: Arc<Semaphore>,
    state: Mutex<State>,
}

pub struct CaptureUploader {
    inner: Arc<UploaderInner>,
}
impl Drop for CaptureUploader {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}
impl CaptureUploader {
    pub fn new(max_active: u32) -> Result<Arc<Self>> {
        if !(1..=4).contains(&max_active) {
            return Err(failure(
                ErrorCode::Validation,
                "Invalid capture upload concurrency",
            ));
        }
        Ok(Arc::new(Self {
            inner: Arc::new(UploaderInner {
                slots: Arc::new(Semaphore::new(max_active as usize)),
                state: Mutex::new(State {
                    closing: false,
                    active: HashMap::new(),
                }),
            }),
        }))
    }

    pub fn start(
        self: &Arc<Self>,
        client: Arc<RemoteClient>,
        writer: CaptureWriter,
        binding: CaptureUploadBinding,
        owner: Box<dyn Send>,
    ) -> Result<CaptureUploadTicket> {
        if !uuid::Uuid::parse_str(&binding.profile_id)
            .is_ok_and(|id| !id.is_nil() && id.to_string() == binding.profile_id)
            || binding.scope != writer.identity().scope
            || binding.scope.server != client.address()
            || binding.credential_account != client.credential_account()
        {
            return Err(failure(
                ErrorCode::Conflict,
                "Capture connection does not match its immutable profile scope",
            ));
        }
        let key = format!("{}:{}", binding.profile_id, writer.identity().capture_id);
        let stop = CancellationToken::new();
        let ended = CancellationToken::new();
        let finished = CancellationToken::new();
        let (progress, rx) = watch::channel(CaptureUploadProgress {
            phase: CaptureUploadPhase::Queued,
            acknowledged_chunks: 0,
            publication: None,
            error: None,
        });
        {
            let mut state =
                self.inner.state.lock().map_err(|_| {
                    failure(ErrorCode::Unavailable, "Capture uploader is unavailable")
                })?;
            if state.closing || state.active.len() >= 64 || state.active.contains_key(&key) {
                return Err(failure(
                    ErrorCode::ResourceLimit,
                    "Capture upload is already queued or its queue is full",
                ));
            }
            state
                .active
                .insert(key.clone(), (stop.clone(), finished.clone()));
        }
        let ticket = CaptureUploadTicket(Arc::new(Control {
            stop: stop.clone(),
            ended: ended.clone(),
            finished: finished.clone(),
            progress: rx,
        }));
        let uploader = self.inner.clone();
        tokio::spawn(async move {
            let slot = tokio::select! {
                biased;
                _ = stop.cancelled() => None,
                slot = uploader.slots.clone().acquire_owned() => slot.ok(),
            };
            let result = std::panic::AssertUnwindSafe(async {
                if slot.is_some() {
                    run(&client, &writer, &binding, &stop, &ended, &progress).await
                } else {
                    Ok(())
                }
            })
            .catch_unwind()
            .await;
            match result {
                Ok(Err(error)) => set(&progress, classify(&error), Some(error)),
                Err(_) => set(
                    &progress,
                    CaptureUploadPhase::NeedsReview,
                    Some(failure(
                        ErrorCode::Internal,
                        "Capture upload stopped unexpectedly; all device audio was retained",
                    )),
                ),
                Ok(Ok(())) => {}
            }
            writer.wait_for_io().await;
            if !matches!(
                progress.borrow().phase,
                CaptureUploadPhase::Verified
                    | CaptureUploadPhase::PendingSession
                    | CaptureUploadPhase::AppendUnsupported
                    | CaptureUploadPhase::AuthenticationRequired
                    | CaptureUploadPhase::NeedsReview
            ) {
                set(&progress, CaptureUploadPhase::Stopped, None);
            }
            drop(writer);
            drop(owner);
            drop(slot);
            drop(client);
            drop(binding);
            uploader
                .state
                .lock()
                .expect("capture uploader poisoned")
                .active
                .remove(&key);
            finished.cancel();
        });
        Ok(ticket)
    }

    pub fn request_shutdown(&self) {
        let mut state = self.inner.state.lock().expect("capture uploader poisoned");
        state.closing = true;
        for (stop, _) in state.active.values() {
            stop.cancel();
        }
    }
    pub async fn wait_shutdown(&self) {
        self.request_shutdown();
        let finished = self
            .inner
            .state
            .lock()
            .expect("capture uploader poisoned")
            .active
            .values()
            .map(|(_, done)| done.clone())
            .collect::<Vec<_>>();
        for done in finished {
            done.cancelled().await;
        }
    }
}

fn set(
    progress: &watch::Sender<CaptureUploadProgress>,
    phase: CaptureUploadPhase,
    error: Option<ApplicationError>,
) {
    progress.send_modify(|value| {
        value.phase = phase;
        value.error = error;
    });
}
fn classify(error: &ApplicationError) -> CaptureUploadPhase {
    if matches!(
        error.code,
        ErrorCode::Unauthenticated | ErrorCode::Forbidden
    ) {
        CaptureUploadPhase::AuthenticationRequired
    } else {
        CaptureUploadPhase::NeedsReview
    }
}
fn retryable(error: &ApplicationError) -> bool {
    error.retryable
        && matches!(
            error.code,
            ErrorCode::Unavailable | ErrorCode::ResourceLimit
        )
}

async fn run(
    client: &Arc<RemoteClient>,
    writer: &CaptureWriter,
    binding: &CaptureUploadBinding,
    stop: &CancellationToken,
    ended: &CancellationToken,
    progress: &watch::Sender<CaptureUploadProgress>,
) -> Result<()> {
    match &writer.identity().target {
        CaptureTarget::PendingSession { .. } => {
            set(progress, CaptureUploadPhase::PendingSession, None);
            return Ok(());
        }
        CaptureTarget::ExistingSession {
            expected_audio_sha256: Some(_),
            ..
        } => {
            set(progress, CaptureUploadPhase::AppendUnsupported, None);
            return Ok(());
        }
        _ => {}
    }
    for feature in [
        "capture_ingestion",
        "capture_publication",
        "recording_leases",
    ] {
        if !client
            .capabilities()
            .features
            .iter()
            .any(|known| known == feature)
        {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "This server does not support verified capture publication",
            ));
        }
    }
    let mut delay = Duration::from_secs(1);
    loop {
        if stop.is_cancelled() {
            return Ok(());
        }
        if client.is_closed() {
            set(progress, CaptureUploadPhase::AuthenticationRequired, None);
            return Ok(());
        }
        match cycle(client, writer, binding, stop, ended, progress).await {
            Ok(true) => return Ok(()),
            Ok(false) => {
                delay = Duration::from_secs(1);
                tokio::select! {_ = stop.cancelled()=>return Ok(()),_ = tokio::time::sleep(Duration::from_millis(500))=>{}}
            }
            Err(_) if stop.is_cancelled() => return Ok(()),
            Err(_) if client.is_closed() => {
                set(progress, CaptureUploadPhase::AuthenticationRequired, None);
                return Ok(());
            }
            Err(error) if retryable(&error) => {
                set(progress, CaptureUploadPhase::Retrying, Some(error));
                tokio::select! {_ = stop.cancelled()=>return Ok(()),_ = tokio::time::sleep(delay)=>{}}
                delay = (delay * 2).min(Duration::from_secs(30));
            }
            Err(error) => return Err(error),
        }
    }
}

async fn cycle(
    client: &Arc<RemoteClient>,
    writer: &CaptureWriter,
    binding: &CaptureUploadBinding,
    stop: &CancellationToken,
    ended: &CancellationToken,
    progress: &watch::Sender<CaptureUploadProgress>,
) -> Result<bool> {
    let snapshot = writer.snapshot().await?;
    let Some(observation) = snapshot.first_admitted_audio_observed_at_ms else {
        if ended.is_cancelled() {
            return Err(review("Capture has no durable producer observation"));
        }
        set(progress, CaptureUploadPhase::WaitingForAudio, None);
        return Ok(false);
    };
    if snapshot.seal.is_none() && ended.is_cancelled() {
        return Err(review(
            "Interrupted capture needs review; it was not automatically sealed or resumed",
        ));
    }
    if snapshot.seal.is_none() && snapshot.chunk_count == progress.borrow().acknowledged_chunks {
        set(progress, CaptureUploadPhase::WaitingForAudio, None);
        return Ok(false);
    }
    let me = client
        .execute(
            RemoteOperation::Me,
            &uuid::Uuid::new_v4().to_string(),
            stop.clone(),
        )
        .await?;
    if !matches!(me,RemoteReply::Me(me) if me.member.id==binding.scope.member_id) {
        return Err(review(
            "The authenticated member changed; reconnect the original capture profile",
        ));
    }
    let vault = &binding.scope.vault_id;
    let id = &snapshot.identity.capture_id;
    if let Some(publication) = &snapshot.publication {
        verify(client, publication, vault, stop).await?;
        progress.send_modify(|p| {
            p.phase = CaptureUploadPhase::Verified;
            p.acknowledged_chunks = publication.source.chunk_count;
            p.publication = Some(publication.clone());
            p.error = None;
        });
        return Ok(true);
    }
    let mut remote = client
        .begin_capture(
            &CaptureIngestRequest {
                identity: snapshot.identity.clone(),
                format: snapshot.format.clone(),
                first_admitted_audio_observed_at_ms: observation,
            },
            stop.clone(),
        )
        .await?;
    validate_status(&remote, &snapshot)?;
    if snapshot.acknowledged.as_ref() != Some(&remote.prefix) {
        writer.acknowledge_prefix(remote.prefix.clone()).await?;
    }
    progress.send_modify(|p| {
        p.phase = CaptureUploadPhase::Uploading;
        p.acknowledged_chunks = remote.prefix.chunk_count;
        p.error = None;
    });
    if remote.publication.is_none() {
        let mut cursor = remote.prefix.chunk_count;
        for _ in 0..16 {
            if stop.is_cancelled() {
                return Ok(false);
            }
            let current = writer.snapshot().await?;
            if cursor >= current.chunk_count {
                break;
            }
            let chunk = writer.read_chunk(cursor).await?;
            let ack = client
                .append_capture_chunk(vault, id, &chunk.descriptor, chunk.bytes, stop.clone())
                .await?;
            writer.acknowledge_prefix(ack.clone()).await?;
            cursor = ack.chunk_count;
            progress.send_modify(|p| p.acknowledged_chunks = cursor);
        }
        let current = writer.snapshot().await?;
        let Some(seal) = current.seal.clone() else {
            set(progress, CaptureUploadPhase::WaitingForAudio, None);
            return Ok(false);
        };
        if cursor < seal.chunk_count {
            return Ok(false);
        }
        remote = client.seal_capture(vault, id, &seal, stop.clone()).await?;
        validate_status(&remote, &current)?;
        remote = client
            .publish_capture(vault, id, &seal, stop.clone())
            .await?;
        validate_status(&remote, &current)?;
    }
    let publication = remote
        .publication
        .ok_or_else(|| review("Publication did not confirm a canonical recording"))?;
    verify(client, &publication, vault, stop).await?;
    writer.observe_publication(publication.clone()).await?;
    progress.send_modify(|p| {
        p.phase = CaptureUploadPhase::Verified;
        p.publication = Some(publication);
        p.error = None;
    });
    Ok(true)
}

fn validate_status(remote: &CaptureIngestStatus, local: &CaptureSnapshot) -> Result<()> {
    if remote.identity != local.identity
        || remote.format != local.format
        || Some(remote.first_admitted_audio_observed_at_ms)
            != local.first_admitted_audio_observed_at_ms
        || remote.session_id != local.session_id
        || (remote.seal.is_some() && remote.seal != local.seal)
    {
        return Err(review(
            "Server capture does not match this device's immutable declaration",
        ));
    }
    Ok(())
}
fn review(message: &str) -> ApplicationError {
    failure(ErrorCode::Conflict, message)
}

async fn verify(
    client: &Arc<RemoteClient>,
    publication: &CapturePublication,
    vault: &str,
    stop: &CancellationToken,
) -> Result<()> {
    let id = uuid::Uuid::new_v4().to_string();
    let lease = client
        .open_recording(vault, &publication.session_id, &id, stop.clone())
        .await?
        .ok_or_else(|| review("Published recording is absent; device audio was retained"))?;
    let result = async {
        if lease.metadata.sha256 != publication.canonical_audio_sha256
            || lease.metadata.size_bytes != publication.canonical_audio_bytes
        {
            return Err(review(
                "Canonical recording changed; device audio was retained",
            ));
        }
        let mut hash = Sha256::new();
        let mut offset = 0;
        while offset < lease.metadata.size_bytes {
            let bytes = client
                .read_recording_chunk(&lease, offset, RECORDING_CHUNK_LIMIT, stop.clone())
                .await?;
            if bytes.is_empty() {
                return Err(review("Canonical recording verification ended early"));
            }
            offset += bytes.len() as u64;
            hash.update(bytes);
        }
        let actual: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if actual != publication.canonical_audio_sha256 {
            return Err(review(
                "Canonical recording checksum did not match; device audio was retained",
            ));
        }
        Ok(())
    }
    .await;
    let _ = client.close_recording(vault, &id, stop.clone()).await;
    result
}
