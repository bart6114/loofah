use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures_util::FutureExt;
use hypr_loofah_client::capture_spool::{
    CaptureFormat, CaptureIdentity, CaptureScope, CaptureSpool,
};
use tauri_plugin_transcription::{
    DeviceCaptureArgs, DeviceCaptureCompletion, DeviceCaptureCompletionSnapshot, DeviceCaptureKey,
    DeviceCaptureStartError, DeviceFinishReceipt, DeviceRecordingEnd, DeviceSinkError,
};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

use super::{
    DeviceCaptureObserver, PreparedDeviceCapture, SpoolRecordingSink, sink::SinkCompletion,
};

const MAX_RETAINED: usize = 8;
const MAX_HISTORY: usize = 16;

#[derive(Clone)]
pub struct CaptureStartIntent {
    pub profile_id: String,
    pub identity: CaptureIdentity,
    pub format: CaptureFormat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceHostPhase {
    Preparing,
    Starting,
    Capturing,
    Stopping,
    Pending,
    Finished,
    Failed,
    NeverStarted,
}

impl DeviceHostPhase {
    fn is_terminal(self) -> bool {
        matches!(self, Self::Finished | Self::Failed | Self::NeverStarted)
    }
}

#[derive(Clone, Debug)]
pub struct DeviceHostSnapshot {
    pub key: DeviceCaptureKey,
    pub profile_id: String,
    pub identity: CaptureIdentity,
    pub phase: DeviceHostPhase,
    pub completion: DeviceCaptureCompletionSnapshot,
    pub durable_prefix: Option<DeviceFinishReceipt>,
    pub end: Option<DeviceRecordingEnd>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct DeviceCaptureTicket {
    pub key: DeviceCaptureKey,
    status: watch::Receiver<DeviceHostSnapshot>,
}
impl DeviceCaptureTicket {
    pub fn current(&self) -> DeviceHostSnapshot {
        self.status.borrow().clone()
    }
    pub async fn changed(&mut self) -> Result<DeviceHostSnapshot, watch::error::RecvError> {
        self.status.changed().await?;
        Ok(self.status.borrow_and_update().clone())
    }
}

#[derive(Clone, Debug)]
pub struct DeviceShutdownReport {
    pub safe_to_exit: bool,
    pub pending: Vec<DeviceHostSnapshot>,
    pub failures: Vec<DeviceHostSnapshot>,
}

#[ractor::async_trait]
pub(super) trait CaptureDriver: Send + Sync {
    fn ready(&self) -> bool;
    async fn start(&self, args: DeviceCaptureArgs) -> Result<(), DeviceCaptureStartError>;
    async fn stop(&self, key: DeviceCaptureKey) -> bool;
}
struct NativeDriver;
#[ractor::async_trait]
impl CaptureDriver for NativeDriver {
    fn ready(&self) -> bool {
        tauri_plugin_transcription::device_capture_root_ready()
    }
    async fn start(&self, args: DeviceCaptureArgs) -> Result<(), DeviceCaptureStartError> {
        tauri_plugin_transcription::tracked_start_device_capture(args).await
    }
    async fn stop(&self, key: DeviceCaptureKey) -> bool {
        tauri_plugin_transcription::stop_device_capture(key).await
    }
}

struct PreparedState {
    sink: Arc<SpoolRecordingSink>,
    completion: DeviceCaptureCompletion,
}
struct Entry {
    status: watch::Sender<DeviceHostSnapshot>,
    stop: CancellationToken,
    dispatched: AtomicBool,
    stop_worker: AtomicBool,
    observing: AtomicBool,
    settled: CancellationToken,
    prepared: Mutex<Option<PreparedState>>,
}
struct HostState {
    closing: bool,
    entries: HashMap<DeviceCaptureKey, Arc<Entry>>,
    history: VecDeque<DeviceHostSnapshot>,
}

pub struct DeviceCaptureHost {
    spool: Arc<CaptureSpool>,
    driver: Arc<dyn CaptureDriver>,
    observer: Arc<dyn DeviceCaptureObserver>,
    state: Mutex<HostState>,
    changed: watch::Sender<u64>,
    waiters: Arc<Semaphore>,
    executor: tokio::runtime::Handle,
}

impl DeviceCaptureHost {
    pub fn new(spool: Arc<CaptureSpool>, observer: Arc<dyn DeviceCaptureObserver>) -> Arc<Self> {
        Self::with_driver(spool, observer, Arc::new(NativeDriver))
    }
    pub(super) fn with_driver(
        spool: Arc<CaptureSpool>,
        observer: Arc<dyn DeviceCaptureObserver>,
        driver: Arc<dyn CaptureDriver>,
    ) -> Arc<Self> {
        Arc::new(Self {
            spool,
            driver,
            observer,
            state: Mutex::new(HostState {
                closing: false,
                entries: HashMap::new(),
                history: VecDeque::new(),
            }),
            changed: watch::channel(0).0,
            waiters: Arc::new(Semaphore::new(4)),
            // Exit callbacks also invoke this host from Tauri's event-loop thread.
            executor: tokio::runtime::Handle::current(),
        })
    }

    pub fn start(
        self: &Arc<Self>,
        intent: CaptureStartIntent,
    ) -> Result<DeviceCaptureTicket, DeviceSinkError> {
        if !self.driver.ready() {
            return Err(DeviceSinkError("Device recorder is not ready".into()));
        }
        if intent.profile_id.is_empty()
            || intent.profile_id.len() > 256
            || intent.profile_id.chars().any(char::is_control)
        {
            return Err(DeviceSinkError(
                "Invalid device capture profile binding".into(),
            ));
        }
        let id = &intent.identity;
        let target_bounded = match &id.target {
            hypr_loofah_client::capture_spool::CaptureTarget::ExistingSession {
                session_id,
                expected_audio_sha256,
            } => {
                session_id.len() <= 512
                    && expected_audio_sha256
                        .as_ref()
                        .is_none_or(|hash| hash.len() == 64)
            }
            hypr_loofah_client::capture_spool::CaptureTarget::PendingSession {
                create_request_id,
            } => create_request_id.len() == 36,
        };
        if id.scope.server.len() > 4096
            || id.scope.member_id.len() > 512
            || id.scope.vault_id.len() > 512
            || id.capture_id.len() != 36
            || id.request_id.len() != 36
            || !target_bounded
            || hypr_loofah_protocol::validate_capture_format(&intent.format).is_err()
        {
            return Err(DeviceSinkError(
                "Invalid or oversized device capture intent".into(),
            ));
        }
        let key = DeviceCaptureKey::new(hypr_loofah_protocol::capture_fingerprint(
            &intent.identity,
            &intent.format,
        ))?;
        let initial = DeviceHostSnapshot {
            key: key.clone(),
            profile_id: intent.profile_id.clone(),
            identity: intent.identity.clone(),
            phase: DeviceHostPhase::Preparing,
            completion: Default::default(),
            durable_prefix: None,
            end: None,
            error: None,
        };
        let (status, rx) = watch::channel(initial);
        let entry = Arc::new(Entry {
            status,
            stop: CancellationToken::new(),
            dispatched: AtomicBool::new(false),
            stop_worker: AtomicBool::new(false),
            observing: AtomicBool::new(false),
            settled: CancellationToken::new(),
            prepared: Mutex::new(None),
        });
        {
            let mut state = self
                .state
                .lock()
                .map_err(|_| DeviceSinkError("Device capture host is unavailable".into()))?;
            if state.closing
                || state.entries.len() >= MAX_RETAINED
                || state.entries.contains_key(&key)
                || state.entries.values().any(|entry| {
                    matches!(
                        entry.status.borrow().phase,
                        DeviceHostPhase::Pending | DeviceHostPhase::Failed
                    )
                })
            {
                return Err(DeviceSinkError(
                    "Device recording admission is unavailable".into(),
                ));
            }
            state.entries.insert(key.clone(), entry.clone());
        }
        let host = self.clone();
        self.executor.spawn(async move {
            if std::panic::AssertUnwindSafe(host.run(entry.clone(), intent))
                .catch_unwind()
                .await
                .is_err()
            {
                if !entry.dispatched.load(Ordering::Acquire) {
                    host.retire(
                        &entry,
                        DeviceHostPhase::NeverStarted,
                        Some("Capture preparation failed before dispatch".into()),
                    );
                    return;
                }
                host.update(&entry, |state| {
                    state.phase = DeviceHostPhase::Pending;
                    state.error =
                        Some("Device capture task failed before confirming cleanup".into());
                });
                entry.stop.cancel();
                host.ensure_stop_worker(&entry);
                host.ensure_observer(&entry);
            }
        });
        Ok(DeviceCaptureTicket { key, status: rx })
    }

    pub fn request_stop(
        self: &Arc<Self>,
        key: &DeviceCaptureKey,
        profile_id: &str,
        scope: &CaptureScope,
    ) -> Result<bool, DeviceSinkError> {
        let entry = self
            .state
            .lock()
            .map_err(|_| DeviceSinkError("Device capture host is unavailable".into()))?
            .entries
            .get(key)
            .cloned();
        let Some(entry) = entry else {
            return Ok(false);
        };
        let snapshot = entry.status.borrow().clone();
        if snapshot.profile_id != profile_id || snapshot.identity.scope != *scope {
            return Err(DeviceSinkError(
                "Device capture belongs to another immutable profile scope".into(),
            ));
        }
        entry.stop.cancel();
        self.update(&entry, |state| {
            if !matches!(
                state.phase,
                DeviceHostPhase::Pending | DeviceHostPhase::Failed
            ) {
                state.phase = DeviceHostPhase::Stopping;
            }
        });
        self.ensure_stop_worker(&entry);
        Ok(true)
    }

    pub fn request_shutdown(self: &Arc<Self>) {
        let entries = {
            let mut state = self.state.lock().expect("device capture host poisoned");
            state.closing = true;
            state.entries.values().cloned().collect::<Vec<_>>()
        };
        for entry in entries {
            entry.stop.cancel();
            self.ensure_stop_worker(&entry);
        }
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    pub async fn wait_shutdown(
        self: &Arc<Self>,
        deadline: Duration,
    ) -> Result<DeviceShutdownReport, DeviceSinkError> {
        let _waiter = self
            .waiters
            .clone()
            .try_acquire_owned()
            .map_err(|_| DeviceSinkError("Too many device shutdown waiters".into()))?;
        self.request_shutdown();
        let mut changed = self.changed.subscribe();
        let wait = async {
            loop {
                let report = self.shutdown_report();
                if report.pending.is_empty() {
                    return report;
                }
                if changed.changed().await.is_err() {
                    return self.shutdown_report();
                }
            }
        };
        Ok(tokio::time::timeout(deadline, wait)
            .await
            .unwrap_or_else(|_| self.shutdown_report()))
    }

    pub fn shutdown_report(&self) -> DeviceShutdownReport {
        let state = self.state.lock().expect("device capture host poisoned");
        let mut pending = Vec::new();
        let mut failures = Vec::new();
        for entry in state.entries.values() {
            let snapshot = entry.status.borrow().clone();
            if snapshot.phase == DeviceHostPhase::Failed {
                failures.push(snapshot);
            } else {
                pending.push(snapshot);
            }
        }
        DeviceShutdownReport {
            safe_to_exit: pending.is_empty() && failures.is_empty(),
            pending,
            failures,
        }
    }

    fn update(&self, entry: &Entry, action: impl FnOnce(&mut DeviceHostSnapshot)) -> bool {
        let changed = entry.status.send_if_modified(|state| {
            if state.phase.is_terminal() {
                return false;
            }
            action(state);
            true
        });
        if changed {
            self.changed
                .send_modify(|version| *version = version.wrapping_add(1));
        }
        changed
    }
    fn retire(&self, entry: &Entry, phase: DeviceHostPhase, error: Option<String>) {
        if !self.update(entry, |state| {
            state.phase = phase;
            if error.is_some() {
                state.error = error;
            }
        }) {
            return;
        }
        entry.settled.cancel();
        if phase == DeviceHostPhase::Failed {
            return;
        }
        let snapshot = entry.status.borrow().clone();
        let mut state = self.state.lock().expect("device capture host poisoned");
        state.entries.remove(&snapshot.key);
        state.history.push_back(snapshot);
        while state.history.len() > MAX_HISTORY {
            state.history.pop_front();
        }
        drop(state);
        self.changed
            .send_modify(|version| *version = version.wrapping_add(1));
    }

    fn ensure_stop_worker(self: &Arc<Self>, entry: &Arc<Entry>) {
        if !entry.dispatched.load(Ordering::Acquire)
            || entry.stop_worker.swap(true, Ordering::AcqRel)
        {
            return;
        }
        let host = self.clone();
        let entry = entry.clone();
        self.executor.spawn(async move {
            let mut status = entry.status.subscribe();
            let mut retry_delay = Duration::from_millis(100);
            loop {
                let snapshot = status.borrow_and_update().clone();
                if snapshot.completion.resources_released_observed_at.is_some()
                    || matches!(
                        snapshot.phase,
                        DeviceHostPhase::NeverStarted | DeviceHostPhase::Finished
                    )
                {
                    return;
                }
                let key = snapshot.key;
                let stop = host.driver.stop(key);
                tokio::pin!(stop);
                loop {
                    tokio::select! {
                        _ = &mut stop => break,
                        changed = status.changed() => {
                            if changed.is_err() { return; }
                            let state = status.borrow_and_update().clone();
                            if state.completion.resources_released_observed_at.is_some() || matches!(state.phase, DeviceHostPhase::NeverStarted | DeviceHostPhase::Finished) { return; }
                        }
                    }
                }
                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(Duration::from_secs(2));
            }
        });
    }

    async fn run(self: &Arc<Self>, entry: Arc<Entry>, intent: CaptureStartIntent) {
        if entry.stop.is_cancelled() {
            self.retire(&entry, DeviceHostPhase::NeverStarted, None);
            return;
        }
        let prepared = match PreparedDeviceCapture::prepare(
            self.spool.clone(),
            intent.identity,
            intent.format,
            self.observer.clone(),
        )
        .await
        {
            Ok(prepared) => prepared,
            Err(error) => {
                self.retire(
                    &entry,
                    DeviceHostPhase::NeverStarted,
                    Some(error.to_string()),
                );
                return;
            }
        };
        *entry
            .prepared
            .lock()
            .expect("device capture entry poisoned") = Some(PreparedState {
            sink: prepared.sink.clone(),
            completion: prepared.completion.clone(),
        });
        if entry.stop.is_cancelled() {
            prepared
                .sink
                .interrupted("Capture stopped before dispatch".into());
            self.retire(&entry, DeviceHostPhase::NeverStarted, None);
            return;
        }
        self.update(&entry, |state| state.phase = DeviceHostPhase::Starting);
        self.ensure_observer(&entry);
        entry.dispatched.store(true, Ordering::Release);
        let start = self.driver.start(prepared.into_args());
        tokio::pin!(start);
        let result = tokio::select! {
            biased;
            result = &mut start => result,
            _ = entry.stop.cancelled() => {
                self.ensure_stop_worker(&entry);
                tokio::select! { result = &mut start => result, _ = entry.settled.cancelled() => return }
            }
            _ = entry.settled.cancelled() => return,
        };
        self.apply_start_result(&entry, result);
    }

    fn apply_start_result(
        self: &Arc<Self>,
        entry: &Arc<Entry>,
        result: Result<(), DeviceCaptureStartError>,
    ) {
        if entry.settled.is_cancelled() {
            return;
        }
        match result {
            Err(DeviceCaptureStartError::NotDispatched | DeviceCaptureStartError::Rejected) => {
                self.retire(
                    entry,
                    DeviceHostPhase::NeverStarted,
                    Some("Capture was not started".into()),
                );
                return;
            }
            Err(DeviceCaptureStartError::Unconfirmed) => {
                if self.update(entry, |state| {
                    state.phase = DeviceHostPhase::Pending;
                    state.error = Some(
                        "Capture start was not acknowledged; cleanup remains unconfirmed".into(),
                    );
                }) {
                    entry.stop.cancel();
                }
            }
            Ok(()) => {
                self.update(entry, |state| {
                    state.phase = if entry.stop.is_cancelled() {
                        DeviceHostPhase::Stopping
                    } else {
                        DeviceHostPhase::Capturing
                    }
                });
            }
        }
        if entry.stop.is_cancelled() {
            self.ensure_stop_worker(entry);
        }
    }

    fn ensure_observer(self: &Arc<Self>, entry: &Arc<Entry>) {
        if entry.observing.swap(true, Ordering::AcqRel) {
            return;
        }
        let host = self.clone();
        let entry = entry.clone();
        self.executor.spawn(async move {
            host.observe(entry).await;
        });
    }

    async fn observe(self: &Arc<Self>, entry: Arc<Entry>) {
        let prepared = entry
            .prepared
            .lock()
            .expect("device capture entry poisoned")
            .as_ref()
            .map(|state| (state.sink.clone(), state.completion.clone()));
        let Some((sink, mut completion)) = prepared else {
            std::future::pending::<()>().await;
            return;
        };
        let mut finish = sink.completion();
        let mut completion_open = true;
        loop {
            if entry.settled.is_cancelled() {
                return;
            }
            let resources = completion.current();
            let finalizer = finish.borrow_and_update().clone();
            self.update(&entry, |state| {
                state.completion = resources.clone();
                state.durable_prefix = sink.durable_prefix();
            });
            if resources.supervisor_ended && resources.resources_released_observed_at.is_some() {
                match finalizer {
                    SinkCompletion::Finished { end, receipt } => {
                        self.update(&entry, |state| {
                            state.end = Some(end);
                            state.durable_prefix = Some(receipt);
                        });
                        self.retire(&entry, DeviceHostPhase::Finished, resources.failure);
                        return;
                    }
                    SinkCompletion::Failed {
                        reason,
                        durable_prefix,
                    } => {
                        self.update(&entry, |state| state.durable_prefix = durable_prefix);
                        self.retire(&entry, DeviceHostPhase::Failed, Some(reason));
                        return;
                    }
                    SinkCompletion::Pending => {
                        self.retire(&entry, DeviceHostPhase::Failed, Some("Recorder ended without a durable finish receipt; only the confirmed prefix is preserved".into()));
                        return;
                    }
                }
            }
            tokio::select! {
                _ = entry.settled.cancelled() => return,
                changed = completion.changed(), if completion_open => {
                    if changed.is_err() {
                        completion_open = false;
                        self.update(&entry, |state| { if !entry.settled.is_cancelled() { state.phase = DeviceHostPhase::Pending; state.error = Some("Capture completion channel closed without resource/finalizer proof".into()); } });
                    }
                }
                changed = finish.changed() => { if changed.is_err() { std::future::pending::<()>().await; } }
            }
        }
    }
}

#[cfg(test)]
#[path = "manager_tests.rs"]
mod tests;
