use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use futures_util::FutureExt;
use hypr_capture_store::capture_spool::{
    CaptureProcessingDeclaration, CaptureSnapshot, CaptureWriter,
};
use hypr_loofah_client::{ProfileLease, RemoteClient};
use hypr_loofah_protocol::{
    ApplicationError, CaptureIdentity, CaptureProcessingSnapshot, CaptureTarget, ErrorCode,
};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Debug, PartialEq, serde::Serialize, specta::Type)]
pub struct DeviceProcessingProgress {
    pub format: hypr_loofah_protocol::CaptureFormat,
    pub declaration: CaptureProcessingDeclaration,
    pub status: Option<CaptureProcessingSnapshot>,
    pub error: Option<ApplicationError>,
}

struct Entry {
    identity: CaptureIdentity,
    client: Weak<RemoteClient>,
    progress: watch::Receiver<DeviceProcessingProgress>,
    stop: CancellationToken,
    finished: CancellationToken,
}

pub(super) struct ProcessingSupervisor {
    entries: Mutex<HashMap<(String, String), Entry>>,
    requests: Arc<Semaphore>,
    updates: watch::Sender<u64>,
    stop: CancellationToken,
}

impl ProcessingSupervisor {
    pub fn new(updates: watch::Sender<u64>) -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(HashMap::new()),
            requests: Arc::new(Semaphore::new(2)),
            updates,
            stop: CancellationToken::new(),
        })
    }

    pub fn current(
        &self,
        profile: &str,
        identity: &CaptureIdentity,
    ) -> Option<DeviceProcessingProgress> {
        self.entries
            .lock()
            .unwrap()
            .get(&(profile.into(), identity.capture_id.clone()))
            .filter(|entry| entry.identity == *identity)
            .map(|entry| entry.progress.borrow().clone())
    }

    pub fn start(
        self: &Arc<Self>,
        profile_id: String,
        writer: CaptureWriter,
        snapshot: CaptureSnapshot,
        client: Arc<RemoteClient>,
        owner: Arc<ProfileLease>,
    ) -> Result<(), ApplicationError> {
        let Some(declaration) = snapshot.processing else {
            return Ok(());
        };
        let key = (profile_id, snapshot.identity.capture_id.clone());
        let mut entries = self.entries.lock().unwrap();
        if self.stop.is_cancelled() {
            return Err(error(
                ErrorCode::Unavailable,
                "Capture processing is stopping",
            ));
        }
        if let Some(entry) = entries.get(&key) {
            if entry.identity != snapshot.identity {
                return Err(error(
                    ErrorCode::Conflict,
                    "Capture processing identity changed",
                ));
            }
            if !entry.finished.is_cancelled() {
                if entry
                    .client
                    .upgrade()
                    .as_ref()
                    .is_some_and(|previous| Arc::ptr_eq(previous, &client))
                {
                    return Ok(());
                }
                entry.stop.cancel();
                return Err(error(
                    ErrorCode::Unavailable,
                    "The original capture processing connection is draining",
                ));
            }
        }
        if entries.len() >= 64 && !entries.contains_key(&key) {
            if let Some(retired) = entries
                .iter()
                .find(|(_, entry)| entry.finished.is_cancelled())
                .map(|(key, _)| key.clone())
            {
                entries.remove(&retired);
            } else {
                return Err(error(
                    ErrorCode::ResourceLimit,
                    "Too many capture processing observers",
                ));
            }
        }
        let stop = self.stop.child_token();
        let finished = CancellationToken::new();
        let (progress, rx) = watch::channel(DeviceProcessingProgress {
            format: snapshot.format,
            declaration,
            status: None,
            error: None,
        });
        entries.insert(
            key,
            Entry {
                identity: snapshot.identity,
                client: Arc::downgrade(&client),
                progress: rx,
                stop: stop.clone(),
                finished: finished.clone(),
            },
        );
        let supervisor = self.clone();
        tokio::spawn(async move {
            let outcome =
                std::panic::AssertUnwindSafe(supervisor.run(&writer, &client, &stop, &progress))
                    .catch_unwind()
                    .await;
            let failure = match outcome {
                Ok(Err(cause)) => Some(cause),
                Err(_) => Some(error(
                    ErrorCode::Unavailable,
                    "Processing observation stopped; recorded audio was retained",
                )),
                _ => None,
            };
            if let Some(cause) = failure {
                progress.send_modify(|value| value.error = Some(cause));
            }
            writer.wait_for_io().await;
            drop(writer);
            drop(owner);
            supervisor
                .updates
                .send_modify(|revision| *revision = revision.wrapping_add(1));
            finished.cancel();
        });
        Ok(())
    }

    async fn run(
        &self,
        writer: &CaptureWriter,
        client: &Arc<RemoteClient>,
        stop: &CancellationToken,
        progress: &watch::Sender<DeviceProcessingProgress>,
    ) -> Result<(), ApplicationError> {
        let mut declared = false;
        let mut delay = Duration::from_millis(500);
        loop {
            if stop.is_cancelled() || client.is_closed() {
                return Ok(());
            }
            let before = progress.borrow().clone();
            let result = tokio::select! {
                biased;
                _ = stop.cancelled() => return Ok(()),
                result = self.cycle(writer, client, stop, progress, &mut declared) => result,
            };
            match result {
                Ok(true) => return Ok(()),
                Ok(false) => delay = Duration::from_millis(500),
                Err(cause)
                    if cause.retryable
                        && matches!(
                            cause.code,
                            ErrorCode::Unavailable | ErrorCode::ResourceLimit
                        ) =>
                {
                    progress.send_modify(|value| value.error = Some(cause));
                    delay = (delay * 2).min(Duration::from_secs(30));
                }
                Err(cause) => return Err(cause),
            }
            if *progress.borrow() != before {
                self.updates
                    .send_modify(|revision| *revision = revision.wrapping_add(1));
            }
            tokio::select! { _ = stop.cancelled() => return Ok(()), _ = tokio::time::sleep(delay) => {} }
        }
    }

    async fn cycle(
        &self,
        writer: &CaptureWriter,
        client: &Arc<RemoteClient>,
        stop: &CancellationToken,
        progress: &watch::Sender<DeviceProcessingProgress>,
        declared: &mut bool,
    ) -> Result<bool, ApplicationError> {
        let snapshot = writer.snapshot().await?;
        let declaration = snapshot.processing.as_ref().ok_or_else(|| {
            error(
                ErrorCode::Conflict,
                "Capture processing declaration is missing",
            )
        })?;
        progress.send_modify(|value| value.declaration = declaration.clone());
        // Raw upload is the authority for server admission; processing never creates or retargets capture storage.
        if snapshot.acknowledged.is_none() {
            return Ok(declaration.cancel_requested);
        }
        let _permit = self
            .requests
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| error(ErrorCode::Unavailable, "Capture processing is stopping"))?;
        let result = if declaration.cancel_requested {
            client
                .cancel_capture_processing(&snapshot.identity, &declaration.request, stop.clone())
                .await
        } else if *declared {
            client
                .capture_processing_status(&snapshot.identity, &declaration.request, stop.clone())
                .await
        } else {
            client
                .declare_capture_processing(&snapshot.identity, &declaration.request, stop.clone())
                .await
        };
        let remote = match result {
            Err(cause) if declaration.cancel_requested && cause.code == ErrorCode::NotFound => {
                return Ok(true);
            }
            result => result?,
        };
        if matches!(
            snapshot.identity.target,
            CaptureTarget::PendingSession { .. }
        ) && remote.session_id.is_some()
        {
            let binding = client
                .capture_status(
                    &snapshot.identity.scope.vault_id,
                    &snapshot.identity.capture_id,
                    stop.clone(),
                )
                .await?;
            validate_pending_binding(&snapshot, &remote, &binding)?;
        }
        *declared = true;
        let terminal = remote.state.is_terminal();
        progress.send_modify(|value| {
            value.status = Some(remote);
            value.error = None;
        });
        Ok(terminal)
    }

    pub fn request_shutdown(&self) {
        self.stop.cancel();
    }
    pub async fn wait_shutdown(&self) {
        self.request_shutdown();
        let finished = self
            .entries
            .lock()
            .unwrap()
            .values()
            .map(|entry| entry.finished.clone())
            .collect::<Vec<_>>();
        for done in finished {
            done.cancelled().await;
        }
    }
}

fn error(code: ErrorCode, message: &str) -> ApplicationError {
    hypr_loofah_client::failure(code, message)
}

fn validate_pending_binding(
    snapshot: &CaptureSnapshot,
    remote: &CaptureProcessingSnapshot,
    binding: &hypr_loofah_protocol::CaptureIngestStatus,
) -> Result<(), ApplicationError> {
    if binding.identity != snapshot.identity
        || binding.format != snapshot.format
        || binding.prefix.capture_fingerprint != snapshot.capture_fingerprint
        || Some(binding.first_admitted_audio_observed_at_ms)
            != snapshot.first_admitted_audio_observed_at_ms
        || binding.session_id != remote.session_id
        || snapshot
            .session_id
            .as_ref()
            .is_some_and(|session| Some(session) != binding.session_id.as_ref())
    {
        return Err(error(
            ErrorCode::IncompatibleVersion,
            "Processing response does not match the original capture publication",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "processing_tests.rs"]
mod tests;
