//! The app owns this coordinator, independently of webview and SDK connection
//! lifetimes. Profile scope is copied once; later selections cannot retarget it.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};

use hypr_loofah_client::{
    ProfileLease, RemoteClient,
    capture_spool::{
        CaptureFormat, CaptureIdentity, CaptureScope, CaptureSnapshot, CaptureSpool,
        CaptureSpoolLimits, CaptureTarget,
    },
    capture_upload::{CaptureUploadBinding, CaptureUploadTicket, CaptureUploader},
};
use hypr_loofah_protocol::{ApplicationError, ErrorCode};
use tokio::sync::{OnceCell, Semaphore, watch};
use tokio_util::sync::CancellationToken;

use super::{
    CaptureStartIntent, DeviceCaptureHost, DeviceCaptureObserver, DeviceCaptureTicket,
    DeviceHostPhase, DeviceShutdownReport,
};

struct ProfileHost {
    spool: Arc<CaptureSpool>,
    device: Arc<DeviceCaptureHost>,
}
struct CaptureEntry {
    identity: CaptureIdentity,
    format: CaptureFormat,
    processing: Option<hypr_capture_store::capture_spool::CaptureProcessingOptions>,
    device: DeviceCaptureTicket,
}
struct UploadEntry {
    identity: CaptureIdentity,
    client: Weak<RemoteClient>,
    ticket: CaptureUploadTicket,
}
struct State {
    closing: bool,
    profiles: HashMap<String, Arc<OnceCell<Arc<ProfileHost>>>>,
    captures: HashMap<(String, String), CaptureEntry>,
    uploads: HashMap<(String, String), UploadEntry>,
}
struct Inner {
    root: PathBuf,
    observer: Arc<dyn DeviceCaptureObserver>,
    state: Mutex<State>,
    starts: Arc<Semaphore>,
    exports: Arc<Semaphore>,
    start_serial: tokio::sync::Mutex<()>,
    upload_updates: watch::Sender<u64>,
    uploads: Arc<CaptureUploader>,
    processing: Arc<super::processing::ProcessingSupervisor>,
    finished: CancellationToken,
    draining: std::sync::atomic::AtomicBool,
    executor: tokio::runtime::Handle,
    #[cfg(test)]
    mapping_pause: Mutex<Option<MappingPause>>,
}

#[cfg(test)]
struct MappingPause {
    entered: tokio::sync::oneshot::Sender<()>,
    release: Arc<Semaphore>,
}

pub struct RemoteCaptureCoordinator {
    inner: Arc<Inner>,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct DeviceCaptureStartRequest {
    pub request_id: String,
    pub scope: CaptureScope,
    pub target: CaptureTarget,
    pub format: CaptureFormat,
    #[serde(default)]
    pub processing: Option<hypr_capture_store::capture_spool::CaptureProcessingOptions>,
}

pub struct RemoteDeviceCapture {
    pub profile_id: String,
    pub identity: CaptureIdentity,
    pub device: DeviceCaptureTicket,
}
pub struct RemoteCaptureShutdown {
    pub safe_to_exit: bool,
    pub devices: Vec<DeviceShutdownReport>,
}

impl Drop for RemoteCaptureCoordinator {
    fn drop(&mut self) {
        self.inner.request_shutdown();
        self.inner.spawn_drain();
    }
}
impl RemoteCaptureCoordinator {
    #[cfg(test)]
    pub(super) fn pause_mapping(
        &self,
        entered: tokio::sync::oneshot::Sender<()>,
        release: Arc<Semaphore>,
    ) {
        *self.inner.mapping_pause.lock().unwrap() = Some(MappingPause { entered, release });
    }

    #[cfg(test)]
    pub(super) fn pending_starts(&self) -> usize {
        8 - self.inner.starts.available_permits()
    }

    #[cfg(test)]
    pub(super) fn install_test_host(
        &self,
        id: String,
        spool: Arc<CaptureSpool>,
        device: Arc<DeviceCaptureHost>,
    ) {
        let cell = OnceCell::new();
        assert!(cell.set(Arc::new(ProfileHost { spool, device })).is_ok());
        self.inner
            .state
            .lock()
            .unwrap()
            .profiles
            .insert(id, Arc::new(cell));
    }
    pub fn new(
        root: PathBuf,
        observer: Arc<dyn DeviceCaptureObserver>,
    ) -> Result<Arc<Self>, ApplicationError> {
        if !root.is_absolute()
            || root
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(error(
                ErrorCode::Validation,
                "Device capture storage must be an absolute trusted path",
            ));
        }
        let updates = watch::channel(0).0;
        Ok(Arc::new(Self {
            inner: Arc::new(Inner {
                root,
                observer,
                state: Mutex::new(State {
                    closing: false,
                    profiles: HashMap::new(),
                    captures: HashMap::new(),
                    uploads: HashMap::new(),
                }),
                starts: Arc::new(Semaphore::new(8)),
                exports: Arc::new(Semaphore::new(2)),
                start_serial: tokio::sync::Mutex::new(()),
                upload_updates: updates.clone(),
                processing: super::processing::ProcessingSupervisor::new(updates),
                uploads: CaptureUploader::new(2)?,
                finished: CancellationToken::new(),
                draining: std::sync::atomic::AtomicBool::new(false),
                executor: tokio::runtime::Handle::current(),
                #[cfg(test)]
                mapping_pause: Mutex::new(None),
            }),
        }))
    }

    #[cfg(test)]
    pub async fn start(
        &self,
        profile: Arc<ProfileLease>,
        client: Option<Arc<RemoteClient>>,
        request_id: String,
        target: CaptureTarget,
        format: CaptureFormat,
    ) -> Result<RemoteDeviceCapture, ApplicationError> {
        let (_, scope) = profile_scope(&profile).await?;
        self.start_scoped(
            profile,
            client,
            DeviceCaptureStartRequest {
                request_id,
                scope,
                target,
                format,
                processing: None,
            },
        )
        .await
    }

    pub async fn start_scoped(
        &self,
        profile: Arc<ProfileLease>,
        client: Option<Arc<RemoteClient>>,
        request: DeviceCaptureStartRequest,
    ) -> Result<RemoteDeviceCapture, ApplicationError> {
        let DeviceCaptureStartRequest {
            request_id,
            scope: expected_scope,
            target,
            format,
            processing,
        } = request;
        if !valid_uuid(&request_id) {
            return Err(error(
                ErrorCode::Validation,
                "A canonical capture request UUID is required",
            ));
        }
        let permit = self.inner.starts.clone().try_acquire_owned().map_err(|_| {
            error(
                ErrorCode::ResourceLimit,
                "Too many device captures are being prepared",
            )
        })?;
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let (id, scope) = profile_scope(&profile).await?;
            match &target {
                CaptureTarget::ExistingSession {
                    session_id,
                    expected_audio_sha256: None,
                } => {
                    hypr_loofah_protocol::validate_session_id(session_id)
                        .map_err(|_| error(ErrorCode::Validation, "Invalid recording session"))?;
                }
                CaptureTarget::PendingSession { create_request_id }
                    if valid_uuid(create_request_id) => {}
                _ => {
                    return Err(error(
                        ErrorCode::Validation,
                        "Appending or resuming an existing recording is not available",
                    ));
                }
            }
            hypr_loofah_protocol::validate_capture_format(&format)
                .map_err(|message| error(ErrorCode::Validation, message))?;
            let _serial = inner.start_serial.lock().await;
            {
                let state = inner
                    .state
                    .lock()
                    .expect("device capture coordinator poisoned");
                if let Some((_, entry)) = state.captures.iter().find(|((profile, _), entry)| {
                    profile == &id && entry.identity.request_id == request_id
                }) {
                    if entry.identity.scope != expected_scope
                        || entry.identity.target != target
                        || entry.format != format
                        || entry.processing != processing
                    {
                        return Err(error(
                            ErrorCode::Conflict,
                            "The capture request changed; its original recording was retained",
                        ));
                    }
                    return Ok(RemoteDeviceCapture {
                        profile_id: id,
                        identity: entry.identity.clone(),
                        device: entry.device.clone(),
                    });
                }
            }
            if scope != expected_scope {
                return Err(error(
                    ErrorCode::Conflict,
                    "The selected recording destination changed; recording was not started",
                ));
            }
            if let Some(client) = &client {
                validate_client(&scope, client)?;
            }
            let host = inner.host(&id).await?;
            if host.spool.request_exists(&request_id).await? {
                return Err(error(
                    ErrorCode::Conflict,
                    "Recover the previous capture instead of restarting its request UUID",
                ));
            }
            let identity = CaptureIdentity {
                scope,
                capture_id: uuid::Uuid::new_v4().to_string(),
                request_id,
                target,
            };
            let ticket = {
                let mut state = inner
                    .state
                    .lock()
                    .expect("device capture coordinator poisoned");
                if state.closing {
                    return Err(error(ErrorCode::Unavailable, "Device capture is stopping"));
                }
                if state.captures.len() >= 64 {
                    let retired = state
                        .captures
                        .iter()
                        .find(|(_, entry)| terminal(entry.device.current().phase))
                        .map(|(key, _)| key.clone());
                    if let Some(key) = retired {
                        state.captures.remove(&key);
                    } else {
                        return Err(error(
                            ErrorCode::ResourceLimit,
                            "Too many retained device captures",
                        ));
                    }
                }
                host.device
                    .start(CaptureStartIntent {
                        profile_id: id.clone(),
                        identity: identity.clone(),
                        format: format.clone(),
                        processing: processing.clone(),
                    })
                    .map_err(|cause| error(ErrorCode::Unavailable, &cause.to_string()))?
            };
            #[cfg(test)]
            {
                let pause = inner.mapping_pause.lock().unwrap().take();
                if let Some(pause) = pause {
                    let _ = pause.entered.send(());
                    pause
                        .release
                        .acquire()
                        .await
                        .expect("mapping pause closed")
                        .forget();
                }
            }
            {
                let mut state = inner
                    .state
                    .lock()
                    .expect("device capture coordinator poisoned");
                // This start was admitted before shutdown. Complete its mapping
                // even if the caller disappeared or shutdown began meanwhile.
                state.captures.insert(
                    (id.clone(), identity.capture_id.clone()),
                    CaptureEntry {
                        identity: identity.clone(),
                        format,
                        processing,
                        device: ticket.clone(),
                    },
                );
            }
            let watching = ticket.clone();
            let observed = identity.clone();
            let profile_id = id.clone();
            let following = inner.clone();
            inner.executor.spawn(async move {
                following
                    .follow(host, profile_id, observed, watching, client, profile)
                    .await;
            });
            Ok(RemoteDeviceCapture {
                profile_id: id,
                identity,
                device: ticket,
            })
        })
        .await
        .map_err(|_| error(ErrorCode::Unavailable, "Device capture preparation stopped"))?
    }

    pub fn processing_progress(
        &self,
        profile_id: &str,
        identity: &CaptureIdentity,
    ) -> Option<super::DeviceProcessingProgress> {
        self.inner
            .processing
            .current(profile_id, identity)
            .or_else(|| {
                let state = self.inner.state.lock().unwrap();
                let entry = state
                    .captures
                    .get(&(profile_id.into(), identity.capture_id.clone()))?;
                if entry.identity != *identity {
                    return None;
                }
                let options = entry.processing.as_ref()?;
                Some(super::DeviceProcessingProgress {
                    format: entry.format.clone(),
                    declaration: hypr_capture_store::capture_spool::CaptureProcessingDeclaration {
                        request: options.request(hypr_loofah_protocol::capture_fingerprint(
                            identity,
                            &entry.format,
                        )),
                        cancel_requested: false,
                    },
                    status: None,
                    error: None,
                })
            })
    }

    pub async fn cancel_processing(
        &self,
        profile: Arc<ProfileLease>,
        identity: CaptureIdentity,
    ) -> Result<CaptureSnapshot, ApplicationError> {
        let permit = self.inner.starts.clone().try_acquire_owned().map_err(|_| {
            error(
                ErrorCode::ResourceLimit,
                "Too many device capture operations",
            )
        })?;
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let id = profile.profile().await.id;
            let writer = inner.host(&id).await?.spool.resume(identity).await?;
            let snapshot = writer.cancel_processing().await?;
            inner
                .upload_updates
                .send_modify(|revision| *revision = revision.wrapping_add(1));
            Ok(snapshot)
        })
        .await
        .map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "Refresh capture status to confirm processing cancellation",
            )
        })?
    }

    pub fn upload_updates(&self) -> watch::Receiver<u64> {
        self.inner.upload_updates.subscribe()
    }

    pub fn capture_tickets(
        &self,
        profile_id: &str,
        scope: &CaptureScope,
    ) -> Vec<RemoteDeviceCapture> {
        self.inner
            .state
            .lock()
            .expect("device capture coordinator poisoned")
            .captures
            .iter()
            .filter(|((profile, _), entry)| profile == profile_id && entry.identity.scope == *scope)
            .map(|((profile, _), entry)| RemoteDeviceCapture {
                profile_id: profile.clone(),
                identity: entry.identity.clone(),
                device: entry.device.clone(),
            })
            .collect()
    }

    pub fn operations_pending(&self) -> bool {
        self.inner.starts.available_permits() != 8
    }

    pub fn profile_tickets(&self, profile_id: &str) -> Vec<RemoteDeviceCapture> {
        self.inner
            .state
            .lock()
            .expect("device capture coordinator poisoned")
            .captures
            .iter()
            .filter(|((profile, _), _)| profile == profile_id)
            .map(|((profile, _), entry)| RemoteDeviceCapture {
                profile_id: profile.clone(),
                identity: entry.identity.clone(),
                device: entry.device.clone(),
            })
            .collect()
    }

    pub fn upload_ticket(
        &self,
        profile_id: &str,
        identity: &CaptureIdentity,
    ) -> Result<Option<CaptureUploadTicket>, ApplicationError> {
        self.inner.upload_ticket(profile_id, identity)
    }

    #[cfg(test)]
    pub async fn inventory(
        &self,
        profile: Arc<ProfileLease>,
    ) -> Result<Vec<CaptureSnapshot>, ApplicationError> {
        let (_, scope) = profile_scope(&profile).await?;
        self.inventory_scoped(profile, scope).await
    }

    pub async fn inventory_scoped(
        &self,
        profile: Arc<ProfileLease>,
        expected_scope: CaptureScope,
    ) -> Result<Vec<CaptureSnapshot>, ApplicationError> {
        let permit = self.inner.starts.clone().try_acquire_owned().map_err(|_| {
            error(
                ErrorCode::ResourceLimit,
                "Too many device capture operations",
            )
        })?;
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let (id, scope) = profile_scope(&profile).await?;
            if scope != expected_scope {
                return Err(error(
                    ErrorCode::Conflict,
                    "The recording inventory destination changed",
                ));
            }
            inner.host(&id).await?.spool.inventory(scope).await
        })
        .await
        .map_err(|_| error(ErrorCode::Unavailable, "Device capture inventory stopped"))?
    }

    pub async fn recovery_inventory(
        &self,
        profile: Arc<ProfileLease>,
    ) -> Result<Vec<CaptureSnapshot>, ApplicationError> {
        let permit = self.inner.starts.clone().try_acquire_owned().map_err(|_| {
            error(
                ErrorCode::ResourceLimit,
                "Too many device capture operations",
            )
        })?;
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let id = profile.profile().await.id;
            inner.host(&id).await?.spool.recovery_inventory().await
        })
        .await
        .map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "Device capture recovery inventory stopped",
            )
        })?
    }

    pub async fn finish_recovery(
        &self,
        profile: Arc<ProfileLease>,
        identity: CaptureIdentity,
    ) -> Result<CaptureSnapshot, ApplicationError> {
        let permit = self.inner.starts.clone().try_acquire_owned().map_err(|_| {
            error(
                ErrorCode::ResourceLimit,
                "Too many device capture operations",
            )
        })?;
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let writer = inner.recovery_writer(profile, identity).await?;
            let snapshot = writer.snapshot().await?;
            if snapshot.frames == 0 || snapshot.first_admitted_audio_observed_at_ms.is_none() {
                return Err(error(ErrorCode::Conflict, "Saved capture requires audio and its original producer timestamp. Export the retained audio for local recovery."));
            }
            use sha2::{Digest, Sha256};
            let mut prefix = hypr_capture_store::wave::WavePrefix::new(&snapshot);
            let mut source = Sha256::new();
            for sequence in 0..snapshot.chunk_count {
                let chunk = writer.read_chunk(sequence).await?;
                prefix.admit(&chunk, &snapshot)?;
                source.update(&chunk.bytes);
            }
            prefix.finish_snapshot(&snapshot)?;
            writer.seal_if_matches(hypr_loofah_protocol::CaptureSeal {
                capture_fingerprint: snapshot.capture_fingerprint,
                chunk_count: snapshot.chunk_count,
                frames: snapshot.frames,
                pcm_bytes: snapshot.pcm_bytes,
                chain_digest: snapshot.chain_digest,
                source_sha256: source.finalize().iter().map(|byte|format!("{byte:02x}")).collect(),
            }).await?;
            inner.upload_updates.send_modify(|revision| *revision = revision.wrapping_add(1));
            writer.snapshot().await
        }).await.map_err(|_| error(ErrorCode::Unavailable, "Device recovery stopped; refresh its original capture status"))?
    }

    pub async fn export_recovery(
        &self,
        profile: Arc<ProfileLease>,
        identity: CaptureIdentity,
        destination: PathBuf,
    ) -> Result<hypr_capture_store::wave::WaveExportReceipt, ApplicationError> {
        let export = self
            .inner
            .exports
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                error(
                    ErrorCode::ResourceLimit,
                    "Two audio exports are already running",
                )
            })?;
        let permit = self.inner.starts.clone().try_acquire_owned().map_err(|_| {
            error(
                ErrorCode::ResourceLimit,
                "Too many device capture operations",
            )
        })?;
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _export = export;
            let _permit = permit;
            let writer = inner.recovery_writer(profile, identity).await?;
            hypr_capture_store::wave::export_wave(writer, destination).await
        })
        .await
        .map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "Audio export stopped; the original capture was retained",
            )
        })?
    }

    pub async fn resume_upload(
        &self,
        profile: Arc<ProfileLease>,
        client: Arc<RemoteClient>,
        identity: CaptureIdentity,
    ) -> Result<CaptureUploadTicket, ApplicationError> {
        let permit = self.inner.starts.clone().try_acquire_owned().map_err(|_| {
            error(
                ErrorCode::ResourceLimit,
                "Too many device capture operations",
            )
        })?;
        let inner = self.inner.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let (id, scope) = profile_scope(&profile).await?;
            if identity.scope != scope {
                return Err(error(
                    ErrorCode::Conflict,
                    "Select the capture's original profile destination to resume it",
                ));
            }
            validate_client(&scope, &client)?;
            let host = inner.host(&id).await?;
            let ticket = inner.upload(&host, &id, &identity, client, profile).await?;
            let producer_active = inner
                .state
                .lock()
                .expect("device capture coordinator poisoned")
                .captures
                .get(&(id.clone(), identity.capture_id.clone()))
                .is_some_and(|entry| {
                    entry.identity == identity && !terminal(entry.device.current().phase)
                });
            if !producer_active {
                ticket.source_finished();
            }
            Ok(ticket)
        })
        .await
        .map_err(|_| error(ErrorCode::Unavailable, "Device capture recovery stopped"))?
    }

    pub fn request_stop(
        &self,
        profile_id: &str,
        scope: &CaptureScope,
        key: &tauri_plugin_transcription::DeviceCaptureKey,
    ) -> Result<bool, ApplicationError> {
        let state = self
            .inner
            .state
            .lock()
            .expect("device capture coordinator poisoned");
        let host = state
            .profiles
            .get(profile_id)
            .and_then(|host| host.get())
            .ok_or_else(|| {
                error(
                    ErrorCode::NotFound,
                    "Device capture is not active in this profile",
                )
            })?;
        host.device
            .request_stop(key, profile_id, scope)
            .map_err(|cause| error(ErrorCode::Conflict, &cause.to_string()))
    }

    pub fn request_shutdown(&self) {
        self.inner.request_shutdown();
        self.inner.spawn_drain();
    }
    pub async fn wait_shutdown(&self, deadline: Duration) -> RemoteCaptureShutdown {
        self.request_shutdown();
        let _ = tokio::time::timeout(deadline, self.inner.finished.cancelled()).await;
        let devices = self
            .inner
            .state
            .lock()
            .expect("device capture coordinator poisoned")
            .profiles
            .values()
            .filter_map(|host| host.get().map(|host| host.device.shutdown_report()))
            .collect::<Vec<_>>();
        RemoteCaptureShutdown {
            safe_to_exit: self.inner.finished.is_cancelled()
                && devices.iter().all(|report| report.safe_to_exit),
            devices,
        }
    }
}

impl Inner {
    async fn recovery_writer(
        self: &Arc<Self>,
        profile: Arc<ProfileLease>,
        identity: CaptureIdentity,
    ) -> Result<hypr_loofah_client::capture_spool::CaptureWriter, ApplicationError> {
        let _serial = self.start_serial.lock().await;
        let id = profile.profile().await.id;
        {
            let state = self
                .state
                .lock()
                .expect("device capture coordinator poisoned");
            if state.closing {
                return Err(error(ErrorCode::Unavailable, "Device capture is stopping"));
            }
            if let Some(entry) = state
                .captures
                .get(&(id.clone(), identity.capture_id.clone()))
            {
                if entry.identity != identity {
                    return Err(error(
                        ErrorCode::Conflict,
                        "The original capture identity changed",
                    ));
                }
                if !terminal(entry.device.current().phase) {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Stop recording and wait for device audio to finish saving before recovery",
                    ));
                }
            }
        }
        self.host(&id).await?.spool.resume(identity).await
    }

    async fn host(self: &Arc<Self>, id: &str) -> Result<Arc<ProfileHost>, ApplicationError> {
        let cell = {
            let mut state = self
                .state
                .lock()
                .expect("device capture coordinator poisoned");
            if state.closing {
                return Err(error(ErrorCode::Unavailable, "Device capture is stopping"));
            }
            if !state.profiles.contains_key(id) && state.profiles.len() >= 8 {
                return Err(error(
                    ErrorCode::ResourceLimit,
                    "Too many capture profiles are open",
                ));
            }
            state.profiles.entry(id.into()).or_default().clone()
        };
        cell.get_or_try_init(|| async {
            let configured = self.root.clone();
            let resolved = tokio::task::spawn_blocking(move || {
                std::fs::create_dir_all(&configured)?;
                std::fs::canonicalize(configured)
            })
            .await
            .map_err(|_| {
                error(
                    ErrorCode::Unavailable,
                    "Device capture storage initialization stopped",
                )
            })?
            .map_err(|_| {
                error(
                    ErrorCode::Unavailable,
                    "Device capture storage is unavailable",
                )
            })?;
            let spool =
                CaptureSpool::open(resolved.join(id), CaptureSpoolLimits::default()).await?;
            Ok(Arc::new(ProfileHost {
                device: DeviceCaptureHost::new(spool.clone(), self.observer.clone()),
                spool,
            }))
        })
        .await
        .cloned()
    }
    fn upload_ticket(
        &self,
        profile_id: &str,
        identity: &CaptureIdentity,
    ) -> Result<Option<CaptureUploadTicket>, ApplicationError> {
        let state = self
            .state
            .lock()
            .expect("device capture coordinator poisoned");
        match state
            .uploads
            .get(&(profile_id.into(), identity.capture_id.clone()))
        {
            Some(entry) if entry.identity == *identity => Ok(Some(entry.ticket.clone())),
            Some(_) => Err(error(
                ErrorCode::Conflict,
                "Capture upload scope does not match its original identity",
            )),
            None => Ok(None),
        }
    }

    async fn follow(
        self: Arc<Self>,
        host: Arc<ProfileHost>,
        profile_id: String,
        identity: CaptureIdentity,
        mut device: DeviceCaptureTicket,
        client: Option<Arc<RemoteClient>>,
        owner: Arc<ProfileLease>,
    ) {
        let mut attempted = false;
        loop {
            let snapshot = device.current();
            if !attempted
                && let Some(client) = &client
                && !matches!(
                    snapshot.phase,
                    DeviceHostPhase::Preparing | DeviceHostPhase::NeverStarted
                )
            {
                match self
                    .upload(&host, &profile_id, &identity, client.clone(), owner.clone())
                    .await
                {
                    Ok(_) => attempted = true,
                    Err(cause) => {
                        tracing::warn!(code=?cause.code,"device_capture_upload_remains_queued")
                    }
                }
            }
            if terminal(snapshot.phase) {
                if let Ok(Some(upload)) = self.upload_ticket(&profile_id, &identity) {
                    upload.source_finished();
                    upload.wait_stopped().await;
                }
                return;
            }
            if device.changed().await.is_err() {
                return;
            }
        }
    }

    async fn upload(
        &self,
        host: &ProfileHost,
        profile_id: &str,
        identity: &CaptureIdentity,
        client: Arc<RemoteClient>,
        owner: Arc<ProfileLease>,
    ) -> Result<CaptureUploadTicket, ApplicationError> {
        let _admission = {
            let state = self
                .state
                .lock()
                .expect("device capture coordinator poisoned");
            if state.closing {
                return Err(error(ErrorCode::Unavailable, "Device capture is stopping"));
            }
            self.starts.clone().try_acquire_owned().map_err(|_| {
                error(
                    ErrorCode::ResourceLimit,
                    "Too many device capture operations",
                )
            })?
        };
        let key = (profile_id.into(), identity.capture_id.clone());
        let existing = {
            let state = self
                .state
                .lock()
                .expect("device capture coordinator poisoned");
            state.uploads.get(&key).map(|entry| {
                (
                    entry.identity.clone(),
                    entry.client.upgrade(),
                    entry.ticket.clone(),
                )
            })
        };
        if let Some((original, previous, ticket)) = existing {
            if original != *identity {
                return Err(error(ErrorCode::Conflict, "Capture upload scope changed"));
            }
            if !ticket.is_finished() {
                if previous
                    .as_ref()
                    .is_some_and(|previous| Arc::ptr_eq(previous, &client))
                {
                    return Ok(ticket);
                }
                if previous
                    .as_ref()
                    .is_some_and(|previous| !previous.is_closed())
                {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Stop the original capture upload before reconnecting it",
                    ));
                }
                ticket.wait_stopped().await;
            }
        }
        let writer = host.spool.resume(identity.clone()).await?;
        let snapshot = writer.snapshot().await?;
        let mut state = self
            .state
            .lock()
            .expect("device capture coordinator poisoned");
        if state.closing {
            return Err(error(ErrorCode::Unavailable, "Device capture is stopping"));
        }
        if state.uploads.len() >= 64 && !state.uploads.contains_key(&key) {
            let finished = state
                .uploads
                .iter()
                .find(|(_, entry)| entry.ticket.is_finished())
                .map(|(key, _)| key.clone());
            if let Some(key) = finished {
                state.uploads.remove(&key);
            } else {
                return Err(error(ErrorCode::ResourceLimit, "Too many capture uploads"));
            }
        }
        let ticket = self.uploads.start(
            client.clone(),
            writer.clone(),
            CaptureUploadBinding {
                profile_id: profile_id.into(),
                scope: identity.scope.clone(),
                credential_account: client.credential_account().into(),
            },
            Box::new(owner.clone()),
        )?;
        state.uploads.insert(
            key,
            UploadEntry {
                identity: identity.clone(),
                client: Arc::downgrade(&client),
                ticket: ticket.clone(),
            },
        );
        drop(state);
        if let Err(cause) =
            self.processing
                .start(profile_id.into(), writer, snapshot, client, owner)
        {
            tracing::warn!(code=?cause.code, "device_capture_processing_remains_queued");
        }
        self.upload_updates
            .send_modify(|version| *version = version.wrapping_add(1));
        Ok(ticket)
    }
    fn request_shutdown(&self) {
        let mut state = self
            .state
            .lock()
            .expect("device capture coordinator poisoned");
        state.closing = true;
        for host in state.profiles.values().filter_map(|host| host.get()) {
            host.device.request_shutdown();
        }
        self.uploads.request_shutdown();
        self.processing.request_shutdown();
    }
    fn spawn_drain(self: &Arc<Self>) {
        if self
            .draining
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let inner = self.clone();
        self.executor.spawn(async move {
            let _starts = inner.starts.clone().acquire_many_owned(8).await;
            let hosts = inner
                .state
                .lock()
                .expect("device capture coordinator poisoned")
                .profiles
                .values()
                .filter_map(|host| host.get().cloned())
                .collect::<Vec<_>>();
            for host in &hosts {
                host.device.request_shutdown();
            }
            for host in &hosts {
                loop {
                    match host.device.wait_shutdown(Duration::from_secs(1)).await {
                        Ok(report) if report.pending.is_empty() => break,
                        _ => {}
                    }
                }
            }
            inner.uploads.wait_shutdown().await;
            inner.processing.wait_shutdown().await;
            for host in hosts {
                host.spool.wait_for_io().await;
            }
            inner.finished.cancel();
        });
    }
}

pub(crate) async fn profile_scope(
    profile: &ProfileLease,
) -> Result<(String, CaptureScope), ApplicationError> {
    let snapshot = profile.profile().await;
    if !valid_uuid(&snapshot.id) {
        return Err(error(
            ErrorCode::Validation,
            "Invalid device profile identity",
        ));
    }
    let scope = CaptureScope {
        server: snapshot.server,
        member_id: snapshot
            .member_id
            .ok_or_else(|| error(ErrorCode::Validation, "Select a member before recording"))?,
        vault_id: snapshot
            .vault_id
            .ok_or_else(|| error(ErrorCode::Validation, "Select a vault before recording"))?,
    };
    Ok((snapshot.id, scope))
}
fn validate_client(scope: &CaptureScope, client: &RemoteClient) -> Result<(), ApplicationError> {
    if client.address() != scope.server {
        return Err(error(
            ErrorCode::Conflict,
            "The recording connection belongs to another server",
        ));
    }
    Ok(())
}
fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn error(code: ErrorCode, message: &str) -> ApplicationError {
    hypr_loofah_client::failure(code, message)
}

fn terminal(phase: DeviceHostPhase) -> bool {
    matches!(
        phase,
        DeviceHostPhase::Finished | DeviceHostPhase::Failed | DeviceHostPhase::NeverStarted
    )
}
