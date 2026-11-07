use super::super::{DeviceCaptureEvent, ScopedDeviceCaptureEvent};
use super::*;
use hypr_audio_actual::{
    AudioProvider, CaptureConfig, CaptureDrainControl, CaptureDrainError, CaptureDrainFuture,
    CaptureDrainReceipt, CaptureFrame, CaptureStream,
};
use hypr_loofah_client::capture_spool::{CaptureScope, CaptureSpoolLimits, CaptureTarget};
use ractor::{Actor, ActorRef};
use std::{
    path::PathBuf,
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tauri_plugin_transcription::{
    DeviceCaptureRoot, DeviceCaptureRootArgs, DeviceCaptureRootMsg, DeviceCaptureStartError,
    DeviceListenerRuntime, DeviceSessionDataEvent, DeviceSessionErrorEvent,
    DeviceSessionLifecycleEvent, DeviceSessionProgressEvent,
};
use tokio::sync::{Semaphore, watch};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
struct Observer(Mutex<Vec<ScopedDeviceCaptureEvent>>);
impl DeviceCaptureObserver for Observer {
    fn emit(&self, event: ScopedDeviceCaptureEvent) {
        self.0.lock().unwrap().push(event);
    }
}
fn intent() -> CaptureStartIntent {
    CaptureStartIntent {
        profile_id: "profile".into(),
        identity: CaptureIdentity {
            scope: CaptureScope {
                server: "https://capture.example/".into(),
                member_id: "member".into(),
                vault_id: "remote".into(),
            },
            capture_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            target: CaptureTarget::PendingSession {
                create_request_id: uuid::Uuid::new_v4().to_string(),
            },
        },
        format: CaptureFormat::stereo(),
    }
}
async fn spool() -> (tempfile::TempDir, Arc<CaptureSpool>) {
    let temp = tempfile::tempdir().unwrap();
    let spool = CaptureSpool::open(
        temp.path().canonicalize().unwrap().join("device"),
        CaptureSpoolLimits::default(),
    )
    .await
    .unwrap();
    (temp, spool)
}
async fn until(
    ticket: &mut DeviceCaptureTicket,
    predicate: impl Fn(&DeviceHostSnapshot) -> bool,
) -> DeviceHostSnapshot {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let state = ticket.current();
            if predicate(&state) {
                return state;
            }
            ticket.changed().await.unwrap();
        }
    })
    .await
    .unwrap()
}

enum StartMode {
    Reject,
    NotDispatched,
    Unconfirmed,
    NeverAcknowledged,
}
struct Driver {
    ready: bool,
    mode: StartMode,
    starts: AtomicUsize,
    stops: AtomicUsize,
}
#[ractor::async_trait]
impl CaptureDriver for Driver {
    fn ready(&self) -> bool {
        self.ready
    }
    async fn start(
        &self,
        args: tauri_plugin_transcription::DeviceCaptureArgs,
    ) -> Result<(), DeviceCaptureStartError> {
        self.starts.fetch_add(1, Ordering::Release);
        match self.mode {
            StartMode::Reject => Err(DeviceCaptureStartError::Rejected),
            StartMode::NotDispatched => Err(DeviceCaptureStartError::NotDispatched),
            StartMode::Unconfirmed => Err(DeviceCaptureStartError::Unconfirmed),
            StartMode::NeverAcknowledged => {
                std::future::pending::<()>().await;
                drop(args);
                unreachable!()
            }
        }
    }
    async fn stop(&self, _: DeviceCaptureKey) -> bool {
        self.stops.fetch_add(1, Ordering::Release);
        false
    }
}
fn driver(ready: bool, mode: StartMode) -> Arc<Driver> {
    Arc::new(Driver {
        ready,
        mode,
        starts: AtomicUsize::new(0),
        stops: AtomicUsize::new(0),
    })
}

#[tokio::test]
async fn no_root_fails_before_spool_begin() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let input = intent();
    let host = DeviceCaptureHost::with_driver(
        spool.clone(),
        Arc::new(Observer::default()),
        driver(false, StartMode::Reject),
    );
    assert!(host.start(input.clone()).is_err());
    assert!(
        spool
            .inventory(input.identity.scope)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(host.shutdown_report().safe_to_exit);
}

#[tokio::test]
async fn definite_rejection_and_root_disappearance_do_not_wait_for_supervisor_receipts() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    for mode in [StartMode::Reject, StartMode::NotDispatched] {
        let (_temp, spool) = spool().await;
        let host = DeviceCaptureHost::with_driver(
            spool,
            Arc::new(Observer::default()),
            driver(true, mode),
        );
        let mut ticket = host.start(intent()).unwrap();
        let state = until(&mut ticket, |state| {
            state.phase == DeviceHostPhase::NeverStarted
        })
        .await;
        assert!(!state.completion.supervisor_ended);
        assert!(
            host.wait_shutdown(Duration::from_secs(1))
                .await
                .unwrap()
                .safe_to_exit
        );
    }
}

#[tokio::test]
async fn unconfirmed_start_and_closed_completion_stay_pending_and_preserve_manifest() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let input = intent();
    let host = DeviceCaptureHost::with_driver(
        spool.clone(),
        Arc::new(Observer::default()),
        driver(true, StartMode::Unconfirmed),
    );
    let mut ticket = host.start(input.clone()).unwrap();
    until(&mut ticket, |state| state.phase == DeviceHostPhase::Pending).await;
    let report = host.wait_shutdown(Duration::from_millis(10)).await.unwrap();
    assert!(!report.safe_to_exit);
    assert_eq!(report.pending.len(), 1);
    assert!(
        report.pending[0]
            .completion
            .resources_released_observed_at
            .is_none()
    );
    assert_eq!(
        spool.inventory(input.identity.scope).await.unwrap().len(),
        1
    );
    assert!(host.start(intent()).is_err());
}

#[tokio::test]
async fn never_acknowledged_start_has_bounded_host_states_and_shutdown_waiters() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let driver = driver(true, StartMode::NeverAcknowledged);
    let host = DeviceCaptureHost::with_driver(spool, Arc::new(Observer::default()), driver.clone());
    let mut tickets = Vec::new();
    for _ in 0..8 {
        tickets.push(host.start(intent()).unwrap());
    }
    assert!(host.start(intent()).is_err());
    for ticket in &mut tickets {
        until(ticket, |state| state.phase == DeviceHostPhase::Starting).await;
    }
    let mut waiters = Vec::new();
    for _ in 0..4 {
        let host = host.clone();
        waiters.push(tokio::spawn(async move {
            host.wait_shutdown(Duration::from_secs(1)).await
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.waiters.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(host.wait_shutdown(Duration::from_millis(1)).await.is_err());
    for waiter in waiters {
        let report = waiter.await.unwrap().unwrap();
        assert!(!report.safe_to_exit);
        assert_eq!(report.pending.len(), 8);
    }
    assert!(driver.stops.load(Ordering::Acquire) >= 8);
    assert!(host.start(intent()).is_err());
}

#[tokio::test]
async fn stop_before_preparation_and_dropped_ticket_never_dispatch_capture() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let driver = driver(true, StartMode::NeverAcknowledged);
    let host = DeviceCaptureHost::with_driver(
        spool.clone(),
        Arc::new(Observer::default()),
        driver.clone(),
    );
    let input = intent();
    let mut ticket = host.start(input.clone()).unwrap();
    assert!(
        host.request_stop(&ticket.key, &input.profile_id, &input.identity.scope)
            .unwrap()
    );
    until(&mut ticket, |state| {
        state.phase == DeviceHostPhase::NeverStarted
    })
    .await;
    drop(ticket);
    assert_eq!(driver.starts.load(Ordering::Acquire), 0);
    assert!(host.shutdown_report().safe_to_exit);
    assert!(
        spool
            .inventory(input.identity.scope)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn control_requires_the_original_profile_and_scope() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let host = DeviceCaptureHost::with_driver(
        spool,
        Arc::new(Observer::default()),
        driver(true, StartMode::NeverAcknowledged),
    );
    let input = intent();
    let ticket = host.start(input.clone()).unwrap();
    assert!(
        host.request_stop(&ticket.key, "other-profile", &input.identity.scope)
            .is_err()
    );
    let mut changed = input.identity.scope.clone();
    changed.member_id = "other-member".into();
    assert!(
        host.request_stop(&ticket.key, &input.profile_id, &changed)
            .is_err()
    );
    assert!(
        host.request_stop(&ticket.key, &input.profile_id, &input.identity.scope)
            .unwrap()
    );
    assert!(
        host.wait_shutdown(Duration::from_secs(1))
            .await
            .unwrap()
            .safe_to_exit
    );
}

struct LocalPanic;
impl hypr_storage::StorageRuntime for LocalPanic {
    fn global_base(&self) -> Result<PathBuf, hypr_storage::Error> {
        panic!("device host opened local storage")
    }
    fn vault_base(&self) -> Result<PathBuf, hypr_storage::Error> {
        panic!("device host opened local vault")
    }
}
impl DeviceListenerRuntime for LocalPanic {
    fn emit_lifecycle(&self, _: DeviceSessionLifecycleEvent) {
        panic!("device host emitted local lifecycle")
    }
    fn emit_progress(&self, _: DeviceSessionProgressEvent) {
        panic!("device host emitted local progress")
    }
    fn emit_error(&self, _: DeviceSessionErrorEvent) {
        panic!("device host emitted local error")
    }
    fn emit_data(&self, _: DeviceSessionDataEvent) {
        panic!("device host emitted local data")
    }
}

struct Audio {
    input: Mutex<Option<tokio::sync::mpsc::Receiver<CaptureFrame>>>,
    resource_gate: Arc<Semaphore>,
    fail_open: AtomicBool,
}
struct Drain {
    stop: CancellationToken,
    done: watch::Receiver<Option<u64>>,
}
impl CaptureDrainControl for Drain {
    fn request_stop(&self) -> Result<(), CaptureDrainError> {
        self.stop.cancel();
        Ok(())
    }
    fn completion(&self) -> CaptureDrainFuture<'_> {
        let mut done = self.done.clone();
        Box::pin(async move {
            loop {
                let count = *done.borrow_and_update();
                if let Some(frames) = count {
                    return Ok(CaptureDrainReceipt { frames });
                }
                done.changed().await.map_err(|_| {
                    CaptureDrainError::Failed("fixture resource ownership disappeared".into())
                })?;
            }
        })
    }
}
impl AudioProvider for Audio {
    fn open_capture(&self, _: CaptureConfig) -> Result<CaptureStream, hypr_audio_actual::Error> {
        if self.fail_open.load(Ordering::Acquire) {
            return Err(hypr_audio_actual::Error::MicOpenFailed);
        }
        let input = self.input.lock().unwrap().take().unwrap();
        let stop = CancellationToken::new();
        let (done, rx) = watch::channel(None);
        let control = Arc::new(Drain {
            stop: stop.clone(),
            done: rx,
        });
        let gate = self.resource_gate.clone();
        let stream = futures_util::stream::unfold(
            (input, stop, done, gate, 0u64, false),
            |(mut input, stop, done, gate, mut frames, mut stopping)| async move {
                let frame = if stopping {
                    input.recv().await
                } else {
                    tokio::select! { _ = stop.cancelled() => { stopping = true; input.close(); input.recv().await }, frame = input.recv() => frame }
                };
                if let Some(frame) = frame {
                    frames += frame.raw_mic.len().max(frame.raw_speaker.len()) as u64;
                    Some((Ok(frame), (input, stop, done, gate, frames, stopping)))
                } else {
                    let _permit = gate.acquire().await.unwrap();
                    done.send_replace(Some(frames));
                    None
                }
            },
        );
        Ok(CaptureStream::new(stream).with_drain_control(control))
    }
    fn open_speaker_capture(
        &self,
        _: u32,
        _: usize,
    ) -> Result<CaptureStream, hypr_audio_actual::Error> {
        unreachable!()
    }
    fn open_mic_capture(
        &self,
        _: Option<String>,
        _: u32,
        _: usize,
    ) -> Result<CaptureStream, hypr_audio_actual::Error> {
        unreachable!()
    }
    fn default_device_name(&self) -> String {
        "fixture".into()
    }
    fn list_mic_devices(&self) -> Vec<String> {
        vec![]
    }
    fn play_silence(&self) -> std::sync::mpsc::Sender<()> {
        std::sync::mpsc::channel().0
    }
    fn play_bytes(&self, _: &'static [u8]) -> std::sync::mpsc::Sender<()> {
        std::sync::mpsc::channel().0
    }
    fn probe_mic(&self, _: Option<String>) -> Result<(), hypr_audio_actual::Error> {
        Ok(())
    }
    fn probe_speaker(&self) -> Result<(), hypr_audio_actual::Error> {
        Ok(())
    }
}
struct RootDriver {
    root: ActorRef<DeviceCaptureRootMsg>,
    finish_gate: Option<Arc<Semaphore>>,
    fail_finish: bool,
}
struct FinishGate {
    inner: Arc<dyn tauri_plugin_transcription::DeviceRecordingSink>,
    gate: Option<Arc<Semaphore>>,
    fail: bool,
}
#[ractor::async_trait]
impl tauri_plugin_transcription::DeviceRecordingSink for FinishGate {
    async fn append(
        &self,
        sequence: u64,
        frames: u32,
        pcm: Arc<[u8]>,
    ) -> Result<tauri_plugin_transcription::DeviceAppendReceipt, DeviceSinkError> {
        self.inner.append(sequence, frames, pcm).await
    }
    async fn finish(
        &self,
        end: DeviceRecordingEnd,
    ) -> Result<DeviceFinishReceipt, DeviceSinkError> {
        if let Some(gate) = &self.gate {
            let _permit = gate.acquire().await.unwrap();
        }
        if self.fail {
            return Err(DeviceSinkError(
                "fixture permanent storage failure; confirmed prefix retained".into(),
            ));
        }
        self.inner.finish(end).await
    }
}
#[ractor::async_trait]
impl CaptureDriver for RootDriver {
    fn ready(&self) -> bool {
        true
    }
    async fn start(
        &self,
        mut args: tauri_plugin_transcription::DeviceCaptureArgs,
    ) -> Result<(), DeviceCaptureStartError> {
        args.sink = Arc::new(FinishGate {
            inner: args.sink,
            gate: self.finish_gate.clone(),
            fail: self.fail_finish,
        });
        ractor::call!(self.root, DeviceCaptureRootMsg::StartDeviceCapture, args)
            .map_err(|_| DeviceCaptureStartError::Unconfirmed)?
            .map_err(|_| DeviceCaptureStartError::Unconfirmed)
    }
    async fn stop(&self, key: DeviceCaptureKey) -> bool {
        ractor::call!(self.root, DeviceCaptureRootMsg::StopDeviceCapture, key).unwrap_or(false)
    }
}
#[derive(Default)]
struct Config {
    resource_gate: Option<Arc<Semaphore>>,
    finish_gate: Option<Arc<Semaphore>>,
    fail_open: bool,
    fail_finish: bool,
}
async fn root_host(
    spool: Arc<CaptureSpool>,
    observer: Arc<Observer>,
    config: Config,
) -> (
    Arc<DeviceCaptureHost>,
    tokio::sync::mpsc::Sender<CaptureFrame>,
    Arc<Semaphore>,
    ActorRef<DeviceCaptureRootMsg>,
    tokio::task::JoinHandle<()>,
) {
    let (tx, rx) = tokio::sync::mpsc::channel(64);
    let gate = config
        .resource_gate
        .unwrap_or_else(|| Arc::new(Semaphore::new(1)));
    let audio = Arc::new(Audio {
        input: Mutex::new(Some(rx)),
        resource_gate: gate.clone(),
        fail_open: AtomicBool::new(config.fail_open),
    });
    let (root, handle) = Actor::spawn(
        None,
        DeviceCaptureRoot,
        DeviceCaptureRootArgs {
            audio,
            runtime: Arc::new(LocalPanic),
        },
    )
    .await
    .unwrap();
    let host = DeviceCaptureHost::with_driver(
        spool,
        observer,
        Arc::new(RootDriver {
            root: root.clone(),
            finish_gate: config.finish_gate,
            fail_finish: config.fail_finish,
        }),
    );
    (host, tx, gate, root, handle)
}
fn frame() -> CaptureFrame {
    CaptureFrame {
        raw_mic: Arc::from(vec![0.125; 1_920]),
        raw_speaker: Arc::from(vec![-0.25; 1_920]),
        aec_mic: None,
    }
}

#[tokio::test]
async fn shutdown_deadline_retains_real_source_then_drains_queued_frames_after_resource_release() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let observer = Arc::new(Observer::default());
    let (host, input, gate, root, handle) = root_host(
        spool.clone(),
        observer.clone(),
        Config {
            resource_gate: Some(Arc::new(Semaphore::new(0))),
            ..Default::default()
        },
    )
    .await;
    let input_scope = intent();
    let identity = input_scope.identity.clone();
    let mut ticket = host.start(input_scope).unwrap();
    for _ in 0..5 {
        input.send(frame()).await.unwrap();
    }
    until(&mut ticket, |state| {
        state.completion.first_admitted_audio_observed_at.is_some()
    })
    .await;
    let first_observed = ticket.current().completion.first_admitted_audio_observed_at;
    let report = host.wait_shutdown(Duration::from_millis(10)).await.unwrap();
    assert!(!report.safe_to_exit);
    assert_eq!(report.pending.len(), 1);
    assert!(
        report.pending[0]
            .completion
            .resources_released_observed_at
            .is_none()
    );
    assert!(host.start(intent()).is_err());
    drop(ticket);
    gate.add_permits(1);
    let report = host.wait_shutdown(Duration::from_secs(5)).await.unwrap();
    assert!(report.safe_to_exit, "{report:?}");
    let snapshots = spool.inventory(identity.scope).await.unwrap();
    assert_eq!(snapshots[0].frames, 5 * 1_920);
    assert!(snapshots[0].seal.is_some());
    assert!(first_observed.is_some());
    assert!(
        observer
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, DeviceCaptureEvent::Sealed(_)))
    );
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn failed_source_has_independent_resource_proof_and_durable_interrupted_finish() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let observer = Arc::new(Observer::default());
    let (host, _input, _gate, root, handle) = root_host(
        spool,
        observer,
        Config {
            fail_open: true,
            ..Default::default()
        },
    )
    .await;
    let mut ticket = host.start(intent()).unwrap();
    let state = until(&mut ticket, |state| {
        state.phase == DeviceHostPhase::Finished
    })
    .await;
    assert_eq!(state.end, Some(DeviceRecordingEnd::Interrupted));
    assert!(state.completion.resources_released_observed_at.is_some());
    assert!(state.completion.first_admitted_audio_observed_at.is_none());
    assert_eq!(state.durable_prefix.unwrap().frames, 0);
    assert!(
        host.wait_shutdown(Duration::from_secs(1))
            .await
            .unwrap()
            .safe_to_exit
    );
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn early_interrupted_ui_is_not_cleanup_or_durability_proof_and_observation_survives() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let observer = Arc::new(Observer::default());
    let (host, input, gate, root, handle) = root_host(
        spool,
        observer,
        Config {
            resource_gate: Some(Arc::new(Semaphore::new(0))),
            ..Default::default()
        },
    )
    .await;
    let mut ticket = host.start(intent()).unwrap();
    input.send(frame()).await.unwrap();
    let initial = until(&mut ticket, |state| {
        state.completion.first_admitted_audio_observed_at.is_some()
    })
    .await;
    let entry = host
        .state
        .lock()
        .unwrap()
        .entries
        .get(&ticket.key)
        .unwrap()
        .clone();
    let sink = entry
        .prepared
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .sink
        .clone();
    sink.interrupted("early display error before owned drain finishes".into());
    let report = host.wait_shutdown(Duration::from_millis(10)).await.unwrap();
    assert!(!report.safe_to_exit);
    assert_eq!(report.pending.len(), 1);
    assert!(
        report.pending[0]
            .completion
            .resources_released_observed_at
            .is_none()
    );
    assert_eq!(
        report.pending[0]
            .completion
            .first_admitted_audio_observed_at,
        initial.completion.first_admitted_audio_observed_at
    );
    gate.add_permits(1);
    assert!(
        host.wait_shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .safe_to_exit
    );
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn resource_release_alone_cannot_finish_shutdown_while_durable_finalizer_is_blocked() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let finish = Arc::new(Semaphore::new(0));
    let (host, input, _gate, root, handle) = root_host(
        spool,
        Arc::new(Observer::default()),
        Config {
            finish_gate: Some(finish.clone()),
            ..Default::default()
        },
    )
    .await;
    let mut ticket = host.start(intent()).unwrap();
    input.send(frame()).await.unwrap();
    until(&mut ticket, |state| {
        state.completion.first_admitted_audio_observed_at.is_some()
    })
    .await;
    host.request_shutdown();
    let state = until(&mut ticket, |state| {
        state.completion.resources_released_observed_at.is_some()
    })
    .await;
    assert!(!state.completion.supervisor_ended);
    assert!(
        !host
            .wait_shutdown(Duration::from_millis(10))
            .await
            .unwrap()
            .safe_to_exit
    );
    finish.add_permits(1);
    assert!(
        host.wait_shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .safe_to_exit
    );
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn terminal_storage_failure_reports_resource_proof_and_preserved_prefix_without_durable_success()
 {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let (host, input, _gate, root, handle) = root_host(
        spool.clone(),
        Arc::new(Observer::default()),
        Config {
            fail_finish: true,
            ..Default::default()
        },
    )
    .await;
    let mut ticket = host.start(intent()).unwrap();
    input.send(frame()).await.unwrap();
    until(&mut ticket, |state| {
        state.completion.first_admitted_audio_observed_at.is_some()
    })
    .await;
    host.request_shutdown();
    let state = until(&mut ticket, |state| state.phase == DeviceHostPhase::Failed).await;
    assert!(state.completion.resources_released_observed_at.is_some());
    assert!(state.completion.supervisor_ended);
    assert_eq!(state.durable_prefix.unwrap().frames, 1_920);
    assert!(state.error.unwrap().contains("durable finish receipt"));
    let report = host.wait_shutdown(Duration::from_secs(1)).await.unwrap();
    assert!(!report.safe_to_exit);
    assert!(report.pending.is_empty());
    assert_eq!(report.failures.len(), 1);
    let snapshot = spool
        .inventory(state.identity.scope)
        .await
        .unwrap()
        .remove(0);
    assert_eq!(snapshot.frames, 1_920);
    assert!(snapshot.seal.is_none());
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_during_owned_preparation_and_dropped_caller_retains_manifest_without_dispatch() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    struct Blocking {
        entered: AtomicBool,
        released: (Mutex<bool>, std::sync::Condvar),
    }
    impl DeviceCaptureObserver for Blocking {
        fn emit(&self, event: ScopedDeviceCaptureEvent) {
            if matches!(event.event, DeviceCaptureEvent::Preparing) {
                self.entered.store(true, Ordering::Release);
                let (lock, changed) = &self.released;
                let mut released = lock.lock().unwrap();
                while !*released {
                    released = changed.wait(released).unwrap();
                }
            }
        }
    }
    struct Release(Arc<Blocking>);
    impl Drop for Release {
        fn drop(&mut self) {
            *self.0.released.0.lock().unwrap() = true;
            self.0.released.1.notify_all();
        }
    }
    let (_temp, spool) = spool().await;
    let observer = Arc::new(Blocking {
        entered: AtomicBool::new(false),
        released: (Mutex::new(false), std::sync::Condvar::new()),
    });
    let release = Release(observer.clone());
    let driver = driver(true, StartMode::NeverAcknowledged);
    let host = DeviceCaptureHost::with_driver(spool.clone(), observer.clone(), driver.clone());
    let input = intent();
    let ticket = host.start(input.clone()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !observer.entered.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        host.request_stop(&ticket.key, &input.profile_id, &input.identity.scope)
            .unwrap()
    );
    drop(ticket);
    drop(release);
    assert!(
        host.wait_shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .safe_to_exit
    );
    assert_eq!(driver.starts.load(Ordering::Acquire), 0);
    assert_eq!(
        spool.inventory(input.identity.scope).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn event_loop_thread_can_request_shutdown_without_a_thread_local_runtime() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let (host, input, gate, root, handle) = root_host(
        spool,
        Arc::new(Observer::default()),
        Config {
            resource_gate: Some(Arc::new(Semaphore::new(0))),
            ..Default::default()
        },
    )
    .await;
    let mut ticket = host.start(intent()).unwrap();
    input.send(frame()).await.unwrap();
    until(&mut ticket, |state| {
        state.completion.first_admitted_audio_observed_at.is_some()
    })
    .await;
    let event_loop_host = host.clone();
    std::thread::spawn(move || {
        assert!(tokio::runtime::Handle::try_current().is_err());
        event_loop_host.request_shutdown();
    })
    .join()
    .unwrap();
    assert!(!host.shutdown_report().safe_to_exit);
    gate.add_permits(1);
    assert!(
        host.wait_shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .safe_to_exit
    );
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn late_start_results_cannot_resurrect_a_retired_capture_ticket() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let (host, input, _gate, root, handle) =
        root_host(spool, Arc::new(Observer::default()), Config::default()).await;
    let mut ticket = host.start(intent()).unwrap();
    input.send(frame()).await.unwrap();
    until(&mut ticket, |state| {
        state.completion.first_admitted_audio_observed_at.is_some()
    })
    .await;
    let entry = host
        .state
        .lock()
        .unwrap()
        .entries
        .get(&ticket.key)
        .unwrap()
        .clone();
    host.request_shutdown();
    let terminal = until(&mut ticket, |state| {
        state.phase == DeviceHostPhase::Finished
    })
    .await;
    assert!(
        host.wait_shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .safe_to_exit
    );
    let history_len = host.state.lock().unwrap().history.len();
    for result in [
        Err(DeviceCaptureStartError::Unconfirmed),
        Err(DeviceCaptureStartError::Rejected),
        Err(DeviceCaptureStartError::NotDispatched),
        Ok(()),
    ] {
        host.apply_start_result(&entry, result);
        let current = ticket.current();
        assert_eq!(current.phase, terminal.phase);
        assert_eq!(current.completion, terminal.completion);
        assert_eq!(current.durable_prefix, terminal.durable_prefix);
        assert_eq!(current.end, terminal.end);
        assert_eq!(current.error, terminal.error);
        assert!(host.shutdown_report().safe_to_exit);
    }
    assert!(!host.update(&entry, |state| state.phase = DeviceHostPhase::Pending));
    host.retire(
        &entry,
        DeviceHostPhase::NeverStarted,
        Some("late rejection".into()),
    );
    assert_eq!(ticket.current().phase, DeviceHostPhase::Finished);
    assert_eq!(host.state.lock().unwrap().history.len(), history_len);
    root.stop(None);
    handle.await.unwrap();
}
