use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use hypr_loofah_client::{
    ProfileLease, capture_spool::CaptureSnapshot, capture_upload::CaptureUploadProgress, failure,
};
use hypr_loofah_protocol::{
    ApplicationError, CaptureIdentity, CapturePublication, CaptureScope, ErrorCode,
};
use serde::Serialize;
use tauri::{Manager, ipc::Channel};
use tokio::sync::{OnceCell, watch};
use tokio_util::sync::CancellationToken;

use crate::device_capture::{
    DeviceCaptureEvent, DeviceCaptureObserver, DeviceHostPhase, DeviceObservationDurability,
    ScopedDeviceCaptureEvent,
    remote::{DeviceCaptureStartRequest, RemoteCaptureCoordinator, profile_scope},
};

#[derive(Clone, Serialize, specta::Type)]
pub struct DeviceRecordingInfo {
    pub profile_id: String,
    pub identity: CaptureIdentity,
    pub phase: Option<DeviceHostPhase>,
    pub first_audio_ms: Option<u64>,
    pub observation_durable: bool,
    pub frames: u64,
    pub pcm_bytes: u64,
    pub sealed: bool,
    pub publication: Option<CapturePublication>,
    pub retirement: Option<hypr_loofah_protocol::CaptureRetirementSnapshot>,
    pub upload: Option<CaptureUploadProgress>,
    pub processing: Option<crate::device_capture::DeviceProcessingProgress>,
    pub failure: Option<String>,
}

#[derive(Clone, Serialize, specta::Type)]
pub struct DeviceRecordingInventory {
    pub profile_id: String,
    pub scope: CaptureScope,
    pub ready: bool,
    pub recordings: Vec<DeviceRecordingInfo>,
}

struct DeviceEvents {
    wake: watch::Sender<u64>,
}
impl Default for DeviceEvents {
    fn default() -> Self {
        Self {
            wake: watch::channel(0).0,
        }
    }
}
impl DeviceCaptureObserver for DeviceEvents {
    fn emit(&self, event: ScopedDeviceCaptureEvent) {
        if !matches!(event.event, DeviceCaptureEvent::Amplitude { .. }) {
            self.wake
                .send_modify(|sequence| *sequence = sequence.wrapping_add(1));
        }
    }
}

#[derive(Default)]
pub struct DeviceRecordingState {
    host: OnceCell<Arc<RemoteCaptureCoordinator>>,
    events: Arc<DeviceEvents>,
    windows: Mutex<HashMap<String, CancellationToken>>,
    subscriptions: Mutex<HashMap<(String, String), Arc<CancellationToken>>>,
    closing: AtomicBool,
    exit_started: AtomicBool,
    inventory: Mutex<Option<DeviceRecordingInventory>>,
    recovery: Mutex<Option<Vec<DeviceRecordingInfo>>>,
    profile_id: OnceLock<String>,
}

impl DeviceRecordingState {
    async fn host<R: tauri::Runtime>(
        &self,
        app: &tauri::AppHandle<R>,
    ) -> Result<Arc<RemoteCaptureCoordinator>, ApplicationError> {
        if self.closing.load(Ordering::SeqCst) {
            return Err(stopping());
        }
        let root = app
            .path()
            .app_local_data_dir()
            .map_err(|_| {
                failure(
                    ErrorCode::Unavailable,
                    "Device capture storage is unavailable",
                )
            })?
            .join("device-captures");
        let host = self
            .host
            .get_or_try_init(|| async { RemoteCaptureCoordinator::new(root, self.events.clone()) })
            .await?
            .clone();
        if self.closing.load(Ordering::SeqCst) {
            host.request_shutdown();
            return Err(stopping());
        }
        Ok(host)
    }

    fn bind_profile(&self, id: &str) -> Result<(), ApplicationError> {
        if self.profile_id.get_or_init(|| id.to_owned()) != id {
            return Err(failure(
                ErrorCode::Conflict,
                "The active recording profile cannot be replaced",
            ));
        }
        Ok(())
    }

    pub fn initialized(&self) -> bool {
        self.host.get().is_some()
    }

    pub fn request_shutdown(&self) {
        self.closing.store(true, Ordering::SeqCst);
        for (_, cancel) in self.subscriptions.lock().unwrap().drain() {
            cancel.cancel();
        }
        if let Some(host) = self.host.get() {
            host.request_shutdown();
        }
    }

    pub async fn drain(&self, deadline: Duration) -> bool {
        self.request_shutdown();
        match self.host.get() {
            Some(host) => host.wait_shutdown(deadline).await.safe_to_exit,
            None => true,
        }
    }

    pub fn release_window(&self, owner: &str) {
        if let Some(lifetime) = self.windows.lock().unwrap().remove(owner) {
            lifetime.cancel();
        }
        self.subscriptions
            .lock()
            .unwrap()
            .retain(|(window, _), cancel| {
                if window == owner {
                    cancel.cancel();
                    false
                } else {
                    true
                }
            });
    }

    async fn snapshot(
        &self,
        profile: &ProfileLease,
    ) -> Result<DeviceRecordingInventory, ApplicationError> {
        let (profile_id, scope) = profile_scope(profile).await?;
        let cached = self.inventory.lock().unwrap().clone();
        let ready = cached
            .as_ref()
            .is_some_and(|value| value.profile_id == profile_id && value.scope == scope)
            && self
                .host
                .get()
                .is_some_and(|host| !host.operations_pending())
            && !self.closing.load(Ordering::SeqCst);
        let mut items = cached.filter(|_| ready).map_or_else(HashMap::new, |value| {
            value
                .recordings
                .into_iter()
                .map(|item| (item.identity.capture_id.clone(), item))
                .collect()
        });
        for item in self.live(&profile_id) {
            let item = match items.get(&item.identity.capture_id) {
                Some(saved) => overlay_live(saved, item),
                None => item,
            };
            items.insert(item.identity.capture_id.clone(), item);
        }
        let mut recordings = items.into_values().collect::<Vec<_>>();
        if let Some(host) = self.host.get() {
            for capture in &mut recordings {
                if let Some(processing) = host.processing_progress(&profile_id, &capture.identity) {
                    let cancelled = capture
                        .processing
                        .as_ref()
                        .is_some_and(|value| value.declaration.cancel_requested);
                    capture.processing = Some(processing);
                    if cancelled {
                        capture
                            .processing
                            .as_mut()
                            .unwrap()
                            .declaration
                            .cancel_requested = true;
                    }
                }
            }
        }
        recordings.sort_by(|a, b| a.identity.capture_id.cmp(&b.identity.capture_id));
        Ok(DeviceRecordingInventory {
            profile_id,
            scope,
            ready,
            recordings,
        })
    }

    fn live(&self, profile_id: &str) -> Vec<DeviceRecordingInfo> {
        let Some(host) = self.host.get() else {
            return vec![];
        };
        let mut result = host
            .profile_tickets(profile_id)
            .into_iter()
            .map(|capture| {
                let state = capture.device.current();
                let (first_audio_ms, observation_durable) = match state.first_audio_observation {
                    DeviceObservationDurability::NotObserved => (None, false),
                    DeviceObservationDurability::Pending { observed_at_ms } => {
                        (Some(observed_at_ms), false)
                    }
                    DeviceObservationDurability::Durable { observed_at_ms } => {
                        (Some(observed_at_ms), true)
                    }
                    DeviceObservationDurability::Failed { observed_at_ms, .. } => {
                        (observed_at_ms, false)
                    }
                };
                let prefix = state.durable_prefix;
                let upload = host
                    .upload_ticket(profile_id, &capture.identity)
                    .ok()
                    .flatten()
                    .map(|ticket| ticket.current());
                DeviceRecordingInfo {
                    profile_id: profile_id.into(),
                    processing: host.processing_progress(profile_id, &capture.identity),
                    identity: capture.identity,
                    phase: Some(state.phase),
                    first_audio_ms,
                    observation_durable,
                    frames: prefix.map_or(0, |prefix| prefix.frames),
                    pcm_bytes: prefix.map_or(0, |prefix| prefix.pcm_bytes),
                    sealed: state.phase == DeviceHostPhase::Finished
                        && state.end
                            == Some(tauri_plugin_transcription::DeviceRecordingEnd::Complete),
                    publication: upload
                        .as_ref()
                        .and_then(|progress| progress.publication.clone()),
                    upload,
                    retirement: None,
                    failure: state.error.or(state.completion.failure),
                }
            })
            .collect::<Vec<_>>();
        result.sort_by(|left, right| left.identity.capture_id.cmp(&right.identity.capture_id));
        result
    }
}

fn stopping() -> ApplicationError {
    failure(ErrorCode::Unavailable, "Device recording is stopping")
}

async fn profile<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Arc<ProfileLease>, ApplicationError> {
    app.state::<crate::remote::RemoteState>()
        .active_profile(app)
        .await
}

fn saved(profile_id: &str, snapshot: CaptureSnapshot) -> DeviceRecordingInfo {
    DeviceRecordingInfo {
        profile_id: profile_id.into(),
        identity: snapshot.identity,
        phase: None,
        first_audio_ms: snapshot.first_admitted_audio_observed_at_ms,
        observation_durable: snapshot.first_admitted_audio_observed_at_ms.is_some(),
        frames: snapshot.frames,
        pcm_bytes: snapshot.pcm_bytes,
        sealed: snapshot.seal.is_some(),
        publication: snapshot.publication,
        retirement: snapshot.retirement,
        upload: None,
        processing: snapshot.processing.map(|declaration| {
            crate::device_capture::DeviceProcessingProgress {
                format: snapshot.format,
                captions: None,
                declaration,
                status: None,
                error: None,
            }
        }),
        failure: None,
    }
}

fn overlay_live(saved: &DeviceRecordingInfo, mut live: DeviceRecordingInfo) -> DeviceRecordingInfo {
    if live.identity == saved.identity
        && live.frames == saved.frames
        && live.pcm_bytes == saved.pcm_bytes
        && matches!(
            live.phase,
            Some(
                DeviceHostPhase::Finished | DeviceHostPhase::Failed | DeviceHostPhase::NeverStarted
            )
        )
    {
        // Explicit recovery can seal a stopped producer's exact durable prefix.
        live.sealed |= saved.sealed;
        live.publication = live.publication.or_else(|| saved.publication.clone());
    }
    if live.identity == saved.identity {
        live.retirement = saved.retirement.clone();
        if let (Some(stored), Some(current)) = (&saved.processing, &mut live.processing) {
            if stored.declaration.request == current.declaration.request {
                current.declaration.cancel_requested |= stored.declaration.cancel_requested;
            }
        } else if live.processing.is_none() {
            live.processing = saved.processing.clone();
        }
    }
    live
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_finish_recovery<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    identity: CaptureIdentity,
) -> Result<DeviceRecordingInfo, ApplicationError> {
    let profile = profile(&app).await?;
    let profile_id = profile.profile().await.id;
    let state = app.state::<DeviceRecordingState>();
    state.bind_profile(&profile_id)?;
    let snapshot = state
        .host(&app)
        .await?
        .finish_recovery(profile, identity)
        .await?;
    *state.recovery.lock().unwrap() = None;
    *state.inventory.lock().unwrap() = None;
    state
        .events
        .wake
        .send_modify(|revision| *revision = revision.wrapping_add(1));
    Ok(saved(&profile_id, snapshot))
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_export<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    identity: CaptureIdentity,
) -> Result<Option<hypr_capture_store::wave::WaveExportReceipt>, ApplicationError> {
    use tauri_plugin_dialog::DialogExt;

    let profile = profile(&app).await?;
    let profile_id = profile.profile().await.id;
    let state = app.state::<DeviceRecordingState>();
    state.bind_profile(&profile_id)?;
    let host = state.host(&app).await?;
    let (selected, destination) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_file_name(format!(
            "Loofah-{}.wav",
            uuid::Uuid::parse_str(&identity.capture_id)
                .map_err(|_| failure(ErrorCode::Validation, "Invalid capture identifier"))?
        ))
        .add_filter("WAV audio", &["wav"])
        .save_file(move |path| {
            let _ = selected.send(path);
        });
    let Some(destination) = destination
        .await
        .map_err(|_| failure(ErrorCode::Cancelled, "Audio export selection was closed"))?
    else {
        return Ok(None);
    };
    let destination = destination
        .into_path()
        .map_err(|_| failure(ErrorCode::Validation, "Choose a file on this device"))?;
    host.export_recovery(profile, identity, destination)
        .await
        .map(Some)
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_start<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: Option<String>,
    input: DeviceCaptureStartRequest,
) -> Result<DeviceRecordingInfo, ApplicationError> {
    let profile = profile(&app).await?;
    let profile_id = profile.profile().await.id;
    // A retired connection must not prevent device capture. Only an existing,
    // window-owned client for this destination may attach an uploader.
    let client = connection_id
        .and_then(|id| {
            app.state::<crate::remote::RemoteState>()
                .profile(&id, window.label(), &profile_id)
                .ok()
                .map(|(client, _)| client)
        })
        .filter(|client| client.address() == input.scope.server);
    let state = app.state::<DeviceRecordingState>();
    state.bind_profile(&profile_id)?;
    // A later start can retire an in-memory ticket. Reconcile all persisted
    // captures before treating the previous inventory as complete again.
    *state.inventory.lock().unwrap() = None;
    *state.recovery.lock().unwrap() = None;
    let request_id = input.request_id.clone();
    let capture = state
        .host(&app)
        .await?
        .start_scoped(profile, client, input)
        .await?;
    state
        .live(&profile_id)
        .into_iter()
        .find(|item| item.identity == capture.identity)
        .ok_or_else(|| ApplicationError {
            request_id: Some(request_id),
            mutation_outcome: Some(hypr_loofah_protocol::MutationOutcome::Unknown),
            ..failure(
                ErrorCode::Unavailable,
                "Recording was accepted; refresh its device status before retrying",
            )
        })
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_inventory<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<DeviceRecordingInventory, ApplicationError> {
    let profile = profile(&app).await?;
    let (profile_id, scope) = profile_scope(&profile).await?;
    let state = app.state::<DeviceRecordingState>();
    state.bind_profile(&profile_id)?;
    let host = state.host(&app).await?;
    let recordings = host
        .inventory_scoped(profile.clone(), scope.clone())
        .await?
        .into_iter()
        .map(|snapshot| saved(&profile_id, snapshot))
        .collect();
    *state.inventory.lock().unwrap() = Some(DeviceRecordingInventory {
        profile_id,
        scope,
        ready: true,
        recordings,
    });
    state.snapshot(&profile).await
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_recovery_inventory<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    refresh: bool,
) -> Result<Vec<DeviceRecordingInfo>, ApplicationError> {
    let profile = profile(&app).await?;
    let profile_id = profile.profile().await.id;
    let state = app.state::<DeviceRecordingState>();
    state.bind_profile(&profile_id)?;
    let host = state.host(&app).await?;
    let cached = state.recovery.lock().unwrap().clone();
    let mut recordings = if refresh || cached.is_none() {
        let saved = host
            .recovery_inventory(profile)
            .await?
            .into_iter()
            .map(|snapshot| saved(&profile_id, snapshot))
            .collect::<Vec<_>>();
        *state.recovery.lock().unwrap() = Some(saved.clone());
        saved
    } else {
        cached.unwrap()
    };
    for capture in state.live(&profile_id) {
        if let Some(existing) = recordings
            .iter_mut()
            .find(|item| item.identity == capture.identity)
        {
            *existing = overlay_live(existing, capture);
        } else {
            recordings.push(capture);
        }
    }
    for capture in &mut recordings {
        if let Some(processing) = host.processing_progress(&profile_id, &capture.identity) {
            let cancelled = capture
                .processing
                .as_ref()
                .is_some_and(|value| value.declaration.cancel_requested);
            capture.processing = Some(processing);
            if cancelled {
                capture
                    .processing
                    .as_mut()
                    .unwrap()
                    .declaration
                    .cancel_requested = true;
            }
        }
        if let Some(upload) = host.upload_ticket(&profile_id, &capture.identity)? {
            let progress = upload.current();
            if let Some(publication) = &progress.publication {
                capture.publication = Some(publication.clone());
            }
            capture.upload = Some(progress);
        }
    }
    recordings.sort_by(|a, b| {
        (
            &a.identity.scope.server,
            &a.identity.scope.member_id,
            &a.identity.scope.vault_id,
            &a.identity.capture_id,
        )
            .cmp(&(
                &b.identity.scope.server,
                &b.identity.scope.member_id,
                &b.identity.scope.vault_id,
                &b.identity.capture_id,
            ))
    });
    Ok(recordings)
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Option<DeviceRecordingInventory>, ApplicationError> {
    let profile = app
        .state::<crate::remote::RemoteState>()
        .leased_active_profile();
    match profile {
        Some(profile) => Ok(Some(
            app.state::<DeviceRecordingState>()
                .snapshot(&profile)
                .await?,
        )),
        None => Ok(None),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_stop<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    identity: CaptureIdentity,
) -> Result<bool, ApplicationError> {
    let state = app.state::<DeviceRecordingState>();
    let profile_id = state
        .profile_id
        .get()
        .ok_or_else(|| failure(ErrorCode::NotFound, "Device recording is not active"))?
        .clone();
    let host = state
        .host
        .get()
        .ok_or_else(|| failure(ErrorCode::NotFound, "Device recording is not active"))?;
    let capture = host
        .profile_tickets(&profile_id)
        .into_iter()
        .find(|capture| capture.identity == identity)
        .ok_or_else(|| {
            failure(
                ErrorCode::NotFound,
                "The original device recording was not found",
            )
        })?;
    host.request_stop(&profile_id, &identity.scope, &capture.device.key)
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_cancel_processing<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    identity: CaptureIdentity,
) -> Result<DeviceRecordingInfo, ApplicationError> {
    let profile = profile(&app).await?;
    let profile_id = profile.profile().await.id;
    let state = app.state::<DeviceRecordingState>();
    state.bind_profile(&profile_id)?;
    let snapshot = state
        .host(&app)
        .await?
        .cancel_processing(profile, identity)
        .await?;
    *state.inventory.lock().unwrap() = None;
    *state.recovery.lock().unwrap() = None;
    Ok(saved(&profile_id, snapshot))
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_retire<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    identity: CaptureIdentity,
    cleanup_request_id: String,
) -> Result<DeviceRecordingInfo, ApplicationError> {
    let profile = profile(&app).await?;
    let profile_id = profile.profile().await.id;
    let state = app.state::<DeviceRecordingState>();
    state.bind_profile(&profile_id)?;
    let (client, _) = app.state::<crate::remote::RemoteState>().profile(
        &connection_id,
        window.label(),
        &profile_id,
    )?;
    let result = state
        .host(&app)
        .await?
        .retire_capture(profile, client, identity, cleanup_request_id)
        .await;
    *state.inventory.lock().unwrap() = None;
    *state.recovery.lock().unwrap() = None;
    result.map(|snapshot| saved(&profile_id, snapshot))
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_resume_upload<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    connection_id: String,
    identity: CaptureIdentity,
) -> Result<CaptureUploadProgress, ApplicationError> {
    let profile = profile(&app).await?;
    let profile_id = profile.profile().await.id;
    app.state::<DeviceRecordingState>()
        .bind_profile(&profile_id)?;
    let (client, _) = app.state::<crate::remote::RemoteState>().profile(
        &connection_id,
        window.label(),
        &profile_id,
    )?;
    Ok(app
        .state::<DeviceRecordingState>()
        .host(&app)
        .await?
        .resume_upload(profile, client, identity)
        .await?
        .current())
}

#[tauri::command]
#[specta::specta]
pub async fn device_recording_subscribe<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    subscription_id: String,
    channel: Channel<u64>,
) -> Result<(), ApplicationError> {
    if !uuid::Uuid::parse_str(&subscription_id)
        .is_ok_and(|id| !id.is_nil() && id.to_string() == subscription_id)
    {
        return Err(failure(
            ErrorCode::Validation,
            "Invalid recording subscription",
        ));
    }
    let state = app.state::<DeviceRecordingState>();
    let owner = window.label().to_owned();
    let lifetime = state
        .windows
        .lock()
        .unwrap()
        .entry(owner.clone())
        .or_default()
        .clone();
    let profile = profile(&app).await?;
    state.bind_profile(&profile.profile().await.id)?;
    let host = state.host(&app).await?;
    let key = (owner, subscription_id);
    let stop = Arc::new(lifetime.child_token());
    {
        let mut subscriptions = state.subscriptions.lock().unwrap();
        if lifetime.is_cancelled() || state.closing.load(Ordering::SeqCst) {
            return Err(stopping());
        }
        if subscriptions.len() >= 32 || subscriptions.contains_key(&key) {
            return Err(failure(
                ErrorCode::ResourceLimit,
                "Recording subscription capacity reached",
            ));
        }
        subscriptions.insert(key.clone(), stop.clone());
    }
    let mut changes = state.events.wake.subscribe();
    let mut uploads = host.upload_updates();
    // Native status is small and memory-only. The tick also observes actual device
    // completion after the last sink event; it never polls storage or the server.
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        let mut sequence = 0u64;
        let mut previous = Vec::new();
        loop {
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = changes.changed() => {},
                _ = uploads.changed() => {},
                _ = tick.tick() => {},
            }
            if stop.is_cancelled() {
                break;
            }
            let status = match app.state::<DeviceRecordingState>().snapshot(&profile).await {
                Ok(status) => status,
                Err(_) => break,
            };
            let current = match serde_json::to_vec(&status) {
                Ok(value) => value,
                Err(_) => break,
            };
            if current != previous {
                previous = current;
                sequence = sequence.wrapping_add(1);
                if channel.send(sequence).is_err() {
                    break;
                }
            }
        }
        let state = app.state::<DeviceRecordingState>();
        let mut subscriptions = state.subscriptions.lock().unwrap();
        if subscriptions
            .get(&key)
            .is_some_and(|current| Arc::ptr_eq(current, &stop))
        {
            subscriptions.remove(&key);
        }
    });
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn device_recording_unsubscribe<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    subscription_id: String,
) {
    if let Some(stop) = app
        .state::<DeviceRecordingState>()
        .subscriptions
        .lock()
        .unwrap()
        .remove(&(window.label().into(), subscription_id))
    {
        stop.cancel();
    }
}

pub fn begin_exit<R: tauri::Runtime>(app: tauri::AppHandle<R>) {
    let state = app.state::<DeviceRecordingState>();
    if state.exit_started.swap(true, Ordering::SeqCst) {
        return;
    }
    state.request_shutdown();
    if let Some(owner) = app.try_state::<crate::local_owner::LocalOwnerState>() {
        owner.request_shutdown();
    }
    if let Some(processing) = app.try_state::<crate::processing::ProcessingState>() {
        processing.request_shutdown();
    }
    if let Some(transcription) =
        app.try_state::<crate::local_transcription::LocalTranscriptionState>()
    {
        transcription.request_shutdown();
    }
    tauri::async_runtime::spawn(async move {
        while !app
            .state::<DeviceRecordingState>()
            .drain(Duration::from_secs(5))
            .await
        {
            let _ = tauri::Emitter::emit(&app, "device-recording-exit-pending", ());
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        if let Some(owner) = app.try_state::<crate::local_owner::LocalOwnerState>() {
            while !owner.drain(Duration::from_secs(5)).await {
                let _ = tauri::Emitter::emit(&app, "device-recording-exit-pending", ());
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        if let Some(store) = app.try_state::<Arc<crate::session_store::SessionStore>>()
            && let Err(error) = store.flush_all().await
        {
            tracing::error!(%error, "session store flush failed during recording shutdown");
        }
        if let Some(processing) = app.try_state::<crate::processing::ProcessingState>() {
            match tokio::time::timeout(crate::EXIT_FLUSH_TIMEOUT, processing.shutdown()).await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => tracing::error!(%error, "processing shutdown failed"),
                Err(_) => tracing::warn!("processing exceeded app exit deadline"),
            }
        }
        if let Some(transcription) =
            app.try_state::<crate::local_transcription::LocalTranscriptionState>()
        {
            if let Err(error) = transcription.shutdown().await {
                tracing::error!(%error, "local transcription shutdown failed");
                return;
            }
        }
        crate::mark_exit_flush_complete();
        app.exit(0);
    });
}

pub fn allow_frontend_exit_flush<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        if !app
            .state::<DeviceRecordingState>()
            .exit_started
            .load(Ordering::SeqCst)
        {
            begin_exit(app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_inventory_preserves_only_the_exact_explicitly_recovered_prefix() {
        let mut stored = DeviceRecordingInfo {
            profile_id: "profile".into(),
            identity: CaptureIdentity {
                scope: CaptureScope {
                    server: "https://capture.example/".into(),
                    member_id: "member".into(),
                    vault_id: "vault".into(),
                },
                capture_id: uuid::Uuid::new_v4().to_string(),
                request_id: uuid::Uuid::new_v4().to_string(),
                target: hypr_loofah_protocol::CaptureTarget::PendingSession {
                    create_request_id: uuid::Uuid::new_v4().to_string(),
                },
            },
            phase: None,
            first_audio_ms: Some(1_794_000_000_001),
            observation_durable: true,
            frames: 1,
            pcm_bytes: 8,
            sealed: true,
            publication: None,
            retirement: None,
            upload: None,
            processing: None,
            failure: None,
        };
        let mut live = stored.clone();
        live.phase = Some(DeviceHostPhase::Finished);
        live.sealed = false;
        assert!(overlay_live(&stored, live.clone()).sealed);
        live.phase = Some(DeviceHostPhase::Pending);
        assert!(!overlay_live(&stored, live.clone()).sealed);
        live.phase = Some(DeviceHostPhase::Finished);
        live.frames = 2;
        assert!(!overlay_live(&stored, live.clone()).sealed);
        live.frames = 1;
        stored.identity.scope.vault_id = "other-vault".into();
        assert!(!overlay_live(&stored, live).sealed);
    }

    #[tokio::test]
    async fn uninitialized_recording_state_drains_without_opening_storage_or_credentials() {
        let state = DeviceRecordingState::default();
        assert!(!state.initialized());
        assert!(state.drain(Duration::from_millis(1)).await);
        assert!(!state.initialized());
        assert!(state.closing.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn unscanned_or_retargeted_inventory_is_unknown_not_idle() {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let profiles =
            hypr_loofah_client::ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        let profile = profiles
            .create("company", "https://capture.example")
            .await
            .unwrap();
        let mut stored = profile.profile().await;
        stored.member_id = Some("member".into());
        stored.vault_id = Some("vault".into());
        profile.save(stored).await.unwrap();
        let state = DeviceRecordingState::default();
        let snapshot = state.snapshot(&profile).await.unwrap();
        assert!(!snapshot.ready);
        assert!(snapshot.recordings.is_empty());
        assert!(!root.path().join("captures").exists());
        let host =
            RemoteCaptureCoordinator::new(root.path().join("captures"), state.events.clone())
                .unwrap();
        state.host.set(host).ok().unwrap();
        *state.inventory.lock().unwrap() = Some(DeviceRecordingInventory {
            ready: true,
            ..snapshot.clone()
        });
        assert!(state.snapshot(&profile).await.unwrap().ready);
        let mut stored = profile.profile().await;
        stored.vault_id = Some("another-vault".into());
        profile.save(stored).await.unwrap();
        let next = state.snapshot(&profile).await.unwrap();
        assert!(!next.ready);
        assert_eq!(next.scope.vault_id, "another-vault");
        assert!(next.recordings.is_empty());
        assert!(state.drain(Duration::from_secs(1)).await);
    }
}
