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
struct Observer(Mutex<Vec<ScopedDeviceCaptureEvent>>, AtomicBool);
impl DeviceCaptureObserver for Observer {
    fn emit(&self, event: ScopedDeviceCaptureEvent) {
        if !self.1.load(Ordering::Acquire) {
            self.0.lock().unwrap().push(event);
        }
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

async fn managed_sink(
    host: &Arc<DeviceCaptureHost>,
    ticket: &mut DeviceCaptureTicket,
) -> Arc<SpoolRecordingSink> {
    until(ticket, |state| state.phase == DeviceHostPhase::Capturing).await;
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
    sink
}
fn digest(bytes: impl AsRef<[u8]>) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes.as_ref())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn fixture_pcm() -> Vec<u8> {
    (0..1_920)
        .flat_map(|_| [0.125_f32.to_le_bytes(), (-0.25_f32).to_le_bytes()].concat())
        .collect()
}
fn capture_directory(root: &std::path::Path, identity: &CaptureIdentity) -> PathBuf {
    root.join(digest(serde_json::to_vec(&identity.scope).unwrap()))
        .join(&identity.capture_id)
}
fn timing_fault(
    code: hypr_loofah_protocol::ErrorCode,
    outcome: hypr_loofah_protocol::MutationOutcome,
    retryable: bool,
) -> hypr_loofah_protocol::ApplicationError {
    hypr_loofah_protocol::ApplicationError {
        code,
        mutation_outcome: Some(outcome),
        retryable,
        ..hypr_loofah_client::failure(code, "fixture timing storage failure")
    }
}

#[tokio::test]
async fn actual_root_frame_persists_the_original_observation_as_v2_and_reopens_exact_audio_hashes()
{
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (temp, spool) = spool().await;
    let path = temp.path().canonicalize().unwrap().join("device");
    let (host, input, _gate, root, handle) = root_host(
        spool.clone(),
        Arc::new(Observer::default()),
        Config::default(),
    )
    .await;
    let intent = intent();
    let identity = intent.identity.clone();
    let mut ticket = host.start(intent).unwrap();
    let sink = managed_sink(&host, &mut ticket).await;
    input.send(frame()).await.unwrap();
    let state = until(&mut ticket, |state| {
        matches!(
            &state.first_audio_observation,
            DeviceObservationDurability::Durable { .. }
        )
    })
    .await;
    let expected = super::super::sink::observation_ms(
        state.completion.first_admitted_audio_observed_at.unwrap(),
    )
    .unwrap();
    assert_eq!(
        state.first_audio_observation,
        DeviceObservationDurability::Durable {
            observed_at_ms: expected
        }
    );
    assert!(
        host.wait_shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .safe_to_exit
    );
    let writer = spool.resume(identity.clone()).await.unwrap();
    let snapshot = writer.snapshot().await.unwrap();
    assert_eq!(snapshot.first_admitted_audio_observed_at_ms, Some(expected));
    assert_eq!(
        snapshot.seal.as_ref().unwrap().source_sha256,
        digest(fixture_pcm())
    );
    let original_chunk = writer.read_chunk(0).await.unwrap();
    assert_eq!(original_chunk.bytes, fixture_pcm());
    root.stop(None);
    handle.await.unwrap();
    drop((writer, sink, ticket, host, spool));
    // Shutdown safety can precede the final observer/stop task dropping its spool lease.
    let reopened = tokio::time::timeout(Duration::from_secs(5), async {
        let mut retry = tokio::time::interval(Duration::from_millis(10));
        retry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            retry.tick().await;
            match CaptureSpool::open(path.clone(), CaptureSpoolLimits::default()).await {
                Ok(spool) => break spool,
                Err(error)
                    if error.code == hypr_loofah_protocol::ErrorCode::Unavailable
                        && error.retryable
                        && error.mutation_outcome
                            == Some(hypr_loofah_protocol::MutationOutcome::NotCommitted) => {}
                Err(error) => panic!("Capture spool reopen failed: {error}"),
            }
        }
    })
    .await
    .expect("Capture spool lease was not released after recorder shutdown");
    let writer = reopened.resume(identity.clone()).await.unwrap();
    assert_eq!(writer.snapshot().await.unwrap(), snapshot);
    assert_eq!(
        writer.read_chunk(0).await.unwrap().descriptor,
        original_chunk.descriptor
    );
    let value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(capture_directory(&path, &identity).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(value["schema_version"], 2);
}

#[tokio::test]
async fn pending_observation_outlives_deadline_detached_observer_and_dropped_callers_without_blocking_audio_drain()
 {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (temp, spool) = spool().await;
    let path = temp.path().canonicalize().unwrap().join("device");
    let observer = Arc::new(Observer::default());
    let (host, input, _gate, root, handle) =
        root_host(spool.clone(), observer.clone(), Config::default()).await;
    let intent = intent();
    let identity = intent.identity.clone();
    let mut ticket = host.start(intent).unwrap();
    let sink = managed_sink(&host, &mut ticket).await;
    let gate = Arc::new(Semaphore::new(0));
    sink.observation_test.lock().unwrap().gate = Some(gate.clone());
    input.send(frame()).await.unwrap();
    until(&mut ticket, |state| {
        matches!(
            &state.first_audio_observation,
            DeviceObservationDurability::Pending { .. }
        )
    })
    .await;
    host.request_shutdown();
    let state = until(&mut ticket, |state| {
        state.completion.supervisor_ended
            && state.completion.resources_released_observed_at.is_some()
    })
    .await;
    let expected = super::super::sink::observation_ms(
        state.completion.first_admitted_audio_observed_at.unwrap(),
    )
    .unwrap();
    assert!(
        !host
            .wait_shutdown(Duration::from_millis(10))
            .await
            .unwrap()
            .safe_to_exit
    );
    assert!(
        !observer
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, DeviceCaptureEvent::Sealed(_)))
    );
    let snapshot = spool
        .resume(identity.clone())
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert!(snapshot.seal.is_some());
    assert_eq!(snapshot.frames, 1_920);
    assert_eq!(snapshot.first_admitted_audio_observed_at_ms, None);
    observer.1.store(true, Ordering::Release);
    root.stop(None);
    handle.await.unwrap();
    drop((input, sink, ticket, host, spool));
    assert_eq!(
        CaptureSpool::open(path.clone(), CaptureSpoolLimits::default())
            .await
            .err()
            .unwrap()
            .code,
        hypr_loofah_protocol::ErrorCode::Unavailable
    );
    gate.add_permits(1);
    let reopened = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match CaptureSpool::open(path.clone(), CaptureSpoolLimits::default()).await {
                Ok(spool) => break spool,
                Err(error) if error.code == hypr_loofah_protocol::ErrorCode::Unavailable => {
                    tokio::time::sleep(Duration::from_millis(10)).await
                }
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .unwrap();
    let restored = reopened
        .resume(identity)
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert_eq!(restored.first_admitted_audio_observed_at_ms, Some(expected));
    assert_eq!(restored.seal, snapshot.seal);
}

#[tokio::test]
async fn observation_busy_partial_write_and_lost_ack_reconfirm_the_same_value_before_durable_success()
 {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    use hypr_loofah_protocol::{ErrorCode, MutationOutcome};
    for scenario in ["busy", "partial", "lost-ack"] {
        let (temp, spool) = spool().await;
        let path = temp.path().canonicalize().unwrap().join("device");
        let (host, input, _gate, root, handle) = root_host(
            spool.clone(),
            Arc::new(Observer::default()),
            Config::default(),
        )
        .await;
        let intent = intent();
        let identity = intent.identity.clone();
        let mut ticket = host.start(intent).unwrap();
        let sink = managed_sink(&host, &mut ticket).await;
        let partial = capture_directory(&path, &identity).join(".fixture-partial-observation.tmp");
        if scenario == "partial" {
            std::fs::write(&partial, b"{\"schema_version\":2,").unwrap();
        }
        {
            let mut faults = sink.observation_test.lock().unwrap();
            if scenario == "lost-ack" {
                faults.after = Some(timing_fault(
                    ErrorCode::Unavailable,
                    MutationOutcome::Unknown,
                    false,
                ));
            } else {
                faults.before = Some(timing_fault(
                    ErrorCode::Unavailable,
                    if scenario == "busy" {
                        MutationOutcome::NotCommitted
                    } else {
                        MutationOutcome::Unknown
                    },
                    scenario == "busy",
                ));
            }
        }
        input.send(frame()).await.unwrap();
        let state = until(&mut ticket, |state| {
            matches!(
                &state.first_audio_observation,
                DeviceObservationDurability::Durable { .. }
            )
        })
        .await;
        let expected = super::super::sink::observation_ms(
            state.completion.first_admitted_audio_observed_at.unwrap(),
        )
        .unwrap();
        assert!(
            sink.observation_attempts.load(Ordering::Acquire) >= 2,
            "{scenario}"
        );
        assert!(
            host.wait_shutdown(Duration::from_secs(5))
                .await
                .unwrap()
                .safe_to_exit
        );
        let snapshot = spool
            .resume(identity)
            .await
            .unwrap()
            .snapshot()
            .await
            .unwrap();
        assert_eq!(snapshot.first_admitted_audio_observed_at_ms, Some(expected));
        assert_eq!(snapshot.seal.unwrap().source_sha256, digest(fixture_pcm()));
        if scenario == "partial" {
            assert_eq!(std::fs::read(partial).unwrap(), b"{\"schema_version\":2,");
        }
        root.stop(None);
        handle.await.unwrap();
    }
}

#[tokio::test]
async fn terminal_observation_failure_stops_and_drains_audio_but_never_reports_durable_capture_success()
 {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let observer = Arc::new(Observer::default());
    let (host, input, _gate, root, handle) =
        root_host(spool.clone(), observer.clone(), Config::default()).await;
    let intent = intent();
    let identity = intent.identity.clone();
    let mut ticket = host.start(intent).unwrap();
    let sink = managed_sink(&host, &mut ticket).await;
    sink.observation_test.lock().unwrap().before = Some(timing_fault(
        hypr_loofah_protocol::ErrorCode::ResourceLimit,
        hypr_loofah_protocol::MutationOutcome::NotCommitted,
        false,
    ));
    input.send(frame()).await.unwrap();
    let state = until(&mut ticket, |state| state.phase == DeviceHostPhase::Failed).await;
    assert!(state.completion.resources_released_observed_at.is_some());
    assert!(state.completion.supervisor_ended);
    assert!(matches!(
        &state.first_audio_observation,
        DeviceObservationDurability::Failed { .. }
    ));
    assert_eq!(state.durable_prefix.unwrap().frames, 1_920);
    let report = host.wait_shutdown(Duration::from_secs(1)).await.unwrap();
    assert!(!report.safe_to_exit);
    assert_eq!(report.failures.len(), 1);
    let snapshot = spool
        .resume(identity)
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert_eq!(snapshot.frames, 1_920);
    assert!(snapshot.seal.is_some());
    assert_eq!(snapshot.first_admitted_audio_observed_at_ms, None);
    assert!(
        !observer
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
async fn interrupted_nonzero_capture_waits_for_the_same_durable_observation_without_sealing_audio()
{
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let (host, input, _gate, root, handle) = root_host(
        spool.clone(),
        Arc::new(Observer::default()),
        Config::default(),
    )
    .await;
    let intent = intent();
    let identity = intent.identity.clone();
    let mut ticket = host.start(intent).unwrap();
    let sink = managed_sink(&host, &mut ticket).await;
    let gate = Arc::new(Semaphore::new(0));
    sink.observation_test.lock().unwrap().gate = Some(gate.clone());
    input.send(frame()).await.unwrap();
    until(&mut ticket, |state| {
        matches!(
            &state.first_audio_observation,
            DeviceObservationDurability::Pending { .. }
        )
    })
    .await;
    drop(input);
    until(&mut ticket, |state| {
        state.completion.supervisor_ended
            && state.completion.resources_released_observed_at.is_some()
    })
    .await;
    assert!(
        !host
            .wait_shutdown(Duration::from_millis(10))
            .await
            .unwrap()
            .safe_to_exit
    );
    gate.add_permits(1);
    let state = until(&mut ticket, |state| {
        state.phase == DeviceHostPhase::Finished
    })
    .await;
    assert_eq!(state.end, Some(DeviceRecordingEnd::Interrupted));
    assert!(matches!(
        &state.first_audio_observation,
        DeviceObservationDurability::Durable { .. }
    ));
    assert!(
        host.wait_shutdown(Duration::from_secs(1))
            .await
            .unwrap()
            .safe_to_exit
    );
    let snapshot = spool
        .resume(identity)
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert!(snapshot.first_admitted_audio_observed_at_ms.is_some());
    assert!(snapshot.seal.is_none());
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn zero_audio_capture_never_invents_or_persists_an_observation() {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let (host, _input, _gate, root, handle) = root_host(
        spool.clone(),
        Arc::new(Observer::default()),
        Config::default(),
    )
    .await;
    let intent = intent();
    let identity = intent.identity.clone();
    let mut ticket = host.start(intent).unwrap();
    let sink = managed_sink(&host, &mut ticket).await;
    assert!(
        host.wait_shutdown(Duration::from_secs(5))
            .await
            .unwrap()
            .safe_to_exit
    );
    let state = ticket.current();
    assert_eq!(state.phase, DeviceHostPhase::Finished);
    assert_eq!(
        state.first_audio_observation,
        DeviceObservationDurability::NotObserved
    );
    assert_eq!(state.completion.first_admitted_audio_observed_at, None);
    assert_eq!(sink.observation_attempts.load(Ordering::Acquire), 0);
    let snapshot = spool
        .resume(identity)
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert_eq!(snapshot.frames, 0);
    assert_eq!(snapshot.first_admitted_audio_observed_at_ms, None);
    root.stop(None);
    handle.await.unwrap();
}

#[tokio::test]
async fn observation_conversion_rejects_pre_epoch_and_unrepresentable_values_without_guessing_now()
{
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    use std::time::SystemTime;
    assert_eq!(
        super::super::sink::observation_ms(SystemTime::UNIX_EPOCH).unwrap(),
        0
    );
    assert!(
        super::super::sink::observation_ms(SystemTime::UNIX_EPOCH - Duration::from_millis(1))
            .is_err()
    );
    if let Some(at) = SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(u64::MAX / 1_000 + 1))
    {
        assert!(super::super::sink::observation_ms(at).is_err());
    }
    let (_temp, spool) = spool().await;
    let identity = intent().identity;
    let prepared = PreparedDeviceCapture::prepare(
        spool.clone(),
        identity.clone(),
        CaptureFormat::stereo(),
        Arc::new(Observer::default()),
    )
    .await
    .unwrap();
    prepared
        .sink
        .record_observation(SystemTime::UNIX_EPOCH - Duration::from_millis(1))
        .await;
    assert!(matches!(
        &*prepared.sink.observation().borrow(),
        DeviceObservationDurability::Failed {
            observed_at_ms: None,
            ..
        }
    ));
    assert_eq!(
        spool.inventory(identity.scope).await.unwrap()[0].first_admitted_audio_observed_at_ms,
        None
    );
}

#[tokio::test]
async fn repeated_ambiguous_observation_acknowledgments_never_promote_visible_metadata_to_durable_success()
 {
    let _capture_test_guard = super::super::CAPTURE_TEST_LOCK.lock().await;
    use hypr_loofah_protocol::{ErrorCode, MutationOutcome};
    let (_temp, spool) = spool().await;
    let observer = Arc::new(Observer::default());
    let (host, input, _gate, root, handle) =
        root_host(spool.clone(), observer.clone(), Config::default()).await;
    let intent = intent();
    let identity = intent.identity.clone();
    let mut ticket = host.start(intent).unwrap();
    let sink = managed_sink(&host, &mut ticket).await;
    {
        let mut faults = sink.observation_test.lock().unwrap();
        faults.before = Some(timing_fault(
            ErrorCode::Unavailable,
            MutationOutcome::Unknown,
            false,
        ));
        faults.after = Some(timing_fault(
            ErrorCode::Unavailable,
            MutationOutcome::Unknown,
            false,
        ));
    }
    input.send(frame()).await.unwrap();
    let state = until(&mut ticket, |state| state.phase == DeviceHostPhase::Failed).await;
    assert!(matches!(
        &state.first_audio_observation,
        DeviceObservationDurability::Failed { .. }
    ));
    assert_eq!(sink.observation_attempts.load(Ordering::Acquire), 2);
    assert!(state.completion.resources_released_observed_at.is_some());
    assert!(state.completion.supervisor_ended);
    assert!(
        !host
            .wait_shutdown(Duration::from_secs(1))
            .await
            .unwrap()
            .safe_to_exit
    );
    let snapshot = spool
        .resume(identity)
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert!(snapshot.first_admitted_audio_observed_at_ms.is_some());
    assert_eq!(snapshot.seal.unwrap().source_sha256, digest(fixture_pcm()));
    assert!(
        !observer
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, DeviceCaptureEvent::Sealed(_)))
    );
    root.stop(None);
    handle.await.unwrap();
}
