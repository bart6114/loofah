use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Instant, SystemTime};

use hypr_audio::AudioProvider;
use ractor::{
    Actor, ActorCell, ActorProcessingErr, ActorRef, ActorStatus, RpcReplyPort, SupervisionEvent,
};

use crate::actors::session::lifecycle::{
    clear_sentry_session_context, configure_sentry_session_context, emit_session_ended,
};
use crate::actors::{
    ChannelMode, DeviceCaptureKey, DeviceIngress, DeviceRecordingSink, RecordingDestination,
    SessionConfigUpdate, SessionContext, SessionMsg, SessionParams, spawn_session_supervisor,
};
use crate::{
    ListenerRuntime, SessionLifecycleEvent, Snapshot, StartSessionError, State, TranscriptionMode,
};

pub struct DeviceCaptureArgs {
    pub key: DeviceCaptureKey,
    pub sink: Arc<dyn DeviceRecordingSink>,
    pub mode: ChannelMode,
    pub runtime: Arc<dyn ListenerRuntime>,
}

pub enum RootMsg {
    StartSession(SessionParams, RpcReplyPort<Result<(), StartSessionError>>),
    StartDeviceCapture(
        DeviceCaptureArgs,
        RpcReplyPort<Result<(), StartSessionError>>,
    ),
    StopDeviceCapture(DeviceCaptureKey, RpcReplyPort<bool>),
    DeviceDraining(DeviceCaptureKey, ractor::ActorId),
    UpdateSessionConfig(SessionConfigUpdate, RpcReplyPort<()>),
    StopSession(RpcReplyPort<()>),
    GetState(RpcReplyPort<State>),
    GetSnapshot(RpcReplyPort<Snapshot>),
}

pub struct RootArgs {
    pub runtime: Arc<dyn ListenerRuntime>,
    pub audio: Arc<dyn AudioProvider>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum CaptureKey {
    Local(String),
    Device(DeviceCaptureKey),
}
impl CaptureKey {
    fn actor_id(&self) -> String {
        match self {
            Self::Local(id) => id.clone(),
            Self::Device(key) => key.actor_id(),
        }
    }
    fn local_id(&self) -> Option<String> {
        match self {
            Self::Local(id) => Some(id.clone()),
            Self::Device(_) => None,
        }
    }
}

struct TrackedCapture {
    key: CaptureKey,
    runtime: Arc<dyn ListenerRuntime>,
    recording: RecordingDestination,
    supervisor: ActorCell,
}

pub struct RootState {
    runtime: Arc<dyn ListenerRuntime>,
    audio: Arc<dyn AudioProvider>,
    active: Option<TrackedCapture>,
    finalizing: HashMap<CaptureKey, TrackedCapture>,
}

pub struct RootActor;
impl RootActor {
    pub fn name() -> ractor::ActorName {
        "listener_root_actor".into()
    }
}

#[ractor::async_trait]
impl Actor for RootActor {
    type Msg = RootMsg;
    type State = RootState;
    type Arguments = RootArgs;

    async fn pre_start(
        &self,
        _myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        Ok(RootState {
            runtime: args.runtime,
            audio: args.audio,
            active: None,
            finalizing: HashMap::new(),
        })
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        message: Self::Msg,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            RootMsg::StartSession(params, reply) => {
                let _ =
                    reply.send(start_session_impl(myself.get_cell(), params, None, state).await);
            }
            RootMsg::StartDeviceCapture(args, reply) => {
                let params = SessionParams {
                    session_id: args.key.actor_id(),
                    languages: vec![],
                    onboarding: false,
                    transcription_mode: TranscriptionMode::Batch,
                    model: String::new(),
                    base_url: String::new(),
                    api_key: String::new(),
                    keywords: vec![],
                    participant_human_ids: vec![],
                    self_human_id: None,
                };
                let _ = reply
                    .send(start_session_impl(myself.get_cell(), params, Some(args), state).await);
            }
            RootMsg::StopDeviceCapture(key, reply) => {
                let matches = state
                    .active
                    .as_ref()
                    .is_some_and(|active| active.key == CaptureKey::Device(key));
                if matches {
                    stop_session_impl(state);
                }
                let _ = reply.send(matches);
            }
            RootMsg::DeviceDraining(key, supervisor) => {
                if state.active.as_ref().is_some_and(|capture| {
                    capture.key == CaptureKey::Device(key)
                        && capture.supervisor.get_id() == supervisor
                }) {
                    let capture = state
                        .active
                        .take()
                        .expect("active device capture was just checked");
                    state.finalizing.insert(capture.key.clone(), capture);
                }
            }
            RootMsg::UpdateSessionConfig(update, reply) => {
                if let Some(active) = &state.active
                    && active.key == CaptureKey::Local(update.session_id.clone())
                {
                    let actor: ActorRef<SessionMsg> = active.supervisor.clone().into();
                    let _ = actor.cast(SessionMsg::UpdateConfig(update));
                }
                let _ = reply.send(());
            }
            RootMsg::StopSession(reply) => {
                stop_session_impl(state);
                let _ = reply.send(());
            }
            RootMsg::GetState(reply) => {
                let _ = reply.send(root_snapshot(state).state);
            }
            RootMsg::GetSnapshot(reply) => {
                let _ = reply.send(root_snapshot(state));
            }
        }
        Ok(())
    }

    async fn handle_supervisor_evt(
        &self,
        _myself: ActorRef<Self::Msg>,
        message: SupervisionEvent,
        state: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        match message {
            SupervisionEvent::ActorTerminated(cell, _, reason) => {
                handle_supervisor_completion(state, cell, reason)
            }
            SupervisionEvent::ActorFailed(cell, error) => {
                handle_supervisor_completion(state, cell, Some(format!("{error:?}")))
            }
            _ => {}
        }
        Ok(())
    }
}

fn root_snapshot(state: &RootState) -> Snapshot {
    Snapshot {
        state: if state.active.is_some() {
            State::Active
        } else if state.finalizing.is_empty() {
            State::Inactive
        } else {
            State::Finalizing
        },
        active_session_id: state
            .active
            .as_ref()
            .and_then(|active| active.key.local_id()),
        finalizing_session_ids: state
            .finalizing
            .keys()
            .filter_map(CaptureKey::local_id)
            .collect(),
    }
}

async fn start_session_impl(
    root_cell: ActorCell,
    mut params: SessionParams,
    device: Option<DeviceCaptureArgs>,
    state: &mut RootState,
) -> Result<(), StartSessionError> {
    let stale = state
        .active
        .as_ref()
        .filter(|active| active.supervisor.get_status() == ActorStatus::Stopped)
        .map(|active| active.supervisor.clone());
    if let Some(cell) = stale {
        handle_supervisor_completion(
            state,
            cell,
            Some("Capture stopped before its completion event".into()),
        );
    }
    let key = device
        .as_ref()
        .map(|args| CaptureKey::Device(args.key.clone()))
        .unwrap_or_else(|| CaptureKey::Local(params.session_id.clone()));
    if state.active.is_some()
        || state.finalizing.contains_key(&key)
        || (device.is_some()
            && state
                .finalizing
                .values()
                .filter(|capture| capture.recording.is_device())
                .count()
                >= 2)
    {
        return Err(StartSessionError::SessionAlreadyRunning);
    }
    let requested_transcription_mode = params.transcription_mode;
    params.transcription_mode = params.effective_transcription_mode();
    let (runtime, recording, device_ingress) = if let Some(device) = device {
        (
            device.runtime,
            RecordingDestination::Device {
                key: device.key,
                sink: device.sink,
                mode: device.mode,
            },
            Some(DeviceIngress::default()),
        )
    } else {
        configure_sentry_session_context(&params);
        let vault_dir = state.runtime.vault_base().map_err(|_| {
            clear_sentry_session_context();
            StartSessionError::FailedToResolveSessionsDir
        })?;
        (
            state.runtime.clone(),
            RecordingDestination::LocalDisk {
                vault_dir,
                session_id: params.session_id.clone(),
            },
            None,
        )
    };
    let ctx = SessionContext {
        runtime: runtime.clone(),
        audio: state.audio.clone(),
        requested_transcription_mode,
        params: params.clone(),
        recording: recording.clone(),
        device_root: if recording.is_device() {
            Some(root_cell.clone().into())
        } else {
            None
        },
        device_ingress,
        started_at_instant: Instant::now(),
        started_at_system: SystemTime::now(),
    };
    let (supervisor, _) = spawn_session_supervisor(ctx).await.map_err(|_| {
        if !recording.is_device() {
            clear_sentry_session_context();
        }
        StartSessionError::FailedToStartSession
    })?;
    supervisor.link(root_cell);
    if supervisor.get_status() == ActorStatus::Stopped {
        if !recording.is_device() {
            clear_sentry_session_context();
        }
        return Err(StartSessionError::FailedToStartSession);
    }
    state.active = Some(TrackedCapture {
        key,
        runtime: runtime.clone(),
        recording,
        supervisor,
    });
    runtime.emit_lifecycle(SessionLifecycleEvent::Active {
        session_id: params.session_id,
        requested_transcription_mode,
        current_transcription_mode: params.transcription_mode,
        error: None,
    });
    Ok(())
}

fn stop_session_impl(state: &mut RootState) {
    if let Some(active) = state.active.take() {
        active
            .runtime
            .emit_lifecycle(SessionLifecycleEvent::Finalizing {
                session_id: active.key.actor_id(),
            });
        let actor: ActorRef<SessionMsg> = active.supervisor.clone().into();
        if actor.cast(SessionMsg::Shutdown).is_err() {
            active
                .supervisor
                .stop(Some("session_stop_cast_failed".into()));
        }
        state.finalizing.insert(active.key.clone(), active);
    }
}

fn handle_supervisor_completion(state: &mut RootState, cell: ActorCell, reason: Option<String>) {
    let capture = if state
        .active
        .as_ref()
        .is_some_and(|active| active.supervisor.get_id() == cell.get_id())
    {
        state.active.take()
    } else {
        let key = state
            .finalizing
            .iter()
            .find(|(_, capture)| capture.supervisor.get_id() == cell.get_id())
            .map(|(key, _)| key.clone());
        key.and_then(|key| state.finalizing.remove(&key))
    };
    let Some(capture) = capture else {
        return;
    };
    match &capture.recording {
        RecordingDestination::LocalDisk {
            vault_dir,
            session_id,
        } => {
            emit_session_ended(
                &*capture.runtime,
                vault_dir,
                session_id,
                reason,
                state.active.is_none(),
            );
        }
        RecordingDestination::Device { .. } => {
            capture
                .runtime
                .emit_lifecycle(SessionLifecycleEvent::Inactive {
                    session_id: capture.key.actor_id(),
                    audio_path: None,
                    error: reason,
                })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::actors::{
        DeviceAppendReceipt, DeviceFinishReceipt, DeviceRecordingEnd, DeviceSinkError,
    };
    use crate::{SessionDataEvent, SessionErrorEvent, SessionProgressEvent};
    use hypr_audio::{CaptureConfig, CaptureFrame, CaptureStream};
    use std::{
        collections::VecDeque,
        sync::{
            Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    #[derive(Default)]
    struct Runtime {
        events: Mutex<Vec<SessionLifecycleEvent>>,
    }
    impl hypr_storage::StorageRuntime for Runtime {
        fn global_base(&self) -> Result<std::path::PathBuf, hypr_storage::Error> {
            panic!("device capture opened local storage")
        }
        fn vault_base(&self) -> Result<std::path::PathBuf, hypr_storage::Error> {
            panic!("device capture opened local vault")
        }
    }
    impl ListenerRuntime for Runtime {
        fn emit_lifecycle(&self, event: SessionLifecycleEvent) {
            self.events.lock().unwrap().push(event);
        }
        fn emit_progress(&self, _event: SessionProgressEvent) {}
        fn emit_error(&self, _event: SessionErrorEvent) {}
        fn emit_data(&self, _event: SessionDataEvent) {}
    }
    type Frames = tokio::sync::mpsc::Receiver<Result<CaptureFrame, hypr_audio::Error>>;
    #[derive(Default)]
    struct Audio(Mutex<VecDeque<Frames>>, std::sync::atomic::AtomicUsize);
    impl Audio {
        fn source(&self) -> tokio::sync::mpsc::Sender<Result<CaptureFrame, hypr_audio::Error>> {
            let (tx, rx) = tokio::sync::mpsc::channel(32);
            self.0.lock().unwrap().push_back(rx);
            tx
        }
        fn stream(&self) -> CaptureStream {
            self.1.fetch_add(1, Ordering::SeqCst);
            let rx = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected extra/restarted audio source");
            CaptureStream::new(futures_util::stream::unfold(rx, |mut rx| async {
                rx.recv().await.map(|item| (item, rx))
            }))
        }
    }
    impl AudioProvider for Audio {
        fn open_capture(&self, config: CaptureConfig) -> Result<CaptureStream, hypr_audio::Error> {
            assert_eq!(config.sample_rate, 16_000);
            Ok(self.stream())
        }
        fn open_speaker_capture(
            &self,
            rate: u32,
            _chunk: usize,
        ) -> Result<CaptureStream, hypr_audio::Error> {
            assert_eq!(rate, 16_000);
            Ok(self.stream())
        }
        fn open_mic_capture(
            &self,
            _device: Option<String>,
            rate: u32,
            _chunk: usize,
        ) -> Result<CaptureStream, hypr_audio::Error> {
            assert_eq!(rate, 16_000);
            Ok(self.stream())
        }
        fn default_device_name(&self) -> String {
            "fake".into()
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
        fn probe_mic(&self, _: Option<String>) -> Result<(), hypr_audio::Error> {
            Ok(())
        }
        fn probe_speaker(&self) -> Result<(), hypr_audio::Error> {
            Ok(())
        }
    }
    struct Sink {
        blocked: AtomicBool,
        fail: AtomicBool,
        gate: Arc<tokio::sync::Semaphore>,
        entered: tokio::sync::Notify,
        totals: Mutex<DeviceFinishReceipt>,
        end: Mutex<Option<DeviceRecordingEnd>>,
    }
    impl Sink {
        fn new(blocked: bool) -> Arc<Self> {
            Arc::new(Self {
                blocked: AtomicBool::new(blocked),
                fail: AtomicBool::new(false),
                gate: Arc::new(tokio::sync::Semaphore::new(0)),
                entered: tokio::sync::Notify::new(),
                totals: Mutex::new(DeviceFinishReceipt {
                    chunk_count: 0,
                    frames: 0,
                    pcm_bytes: 0,
                }),
                end: Mutex::new(None),
            })
        }
        fn release(&self) {
            self.blocked.store(false, Ordering::SeqCst);
            self.gate.add_permits(1);
        }
    }
    #[ractor::async_trait]
    impl DeviceRecordingSink for Sink {
        async fn append(
            &self,
            sequence: u64,
            frames: u32,
            pcm: Arc<[u8]>,
        ) -> Result<DeviceAppendReceipt, DeviceSinkError> {
            self.entered.notify_one();
            if self.fail.load(Ordering::SeqCst) {
                return Err(DeviceSinkError("Storage is full".into()));
            }
            if self.blocked.load(Ordering::SeqCst) {
                self.gate.clone().acquire_owned().await.unwrap().forget();
            }
            let mut totals = self.totals.lock().unwrap();
            assert_eq!(sequence, totals.chunk_count);
            totals.chunk_count += 1;
            totals.frames += u64::from(frames);
            totals.pcm_bytes += pcm.len() as u64;
            Ok(DeviceAppendReceipt { sequence, frames })
        }
        async fn finish(
            &self,
            end: DeviceRecordingEnd,
        ) -> Result<DeviceFinishReceipt, DeviceSinkError> {
            *self.end.lock().unwrap() = Some(end);
            Ok(*self.totals.lock().unwrap())
        }
    }
    fn args(id: char, sink: Arc<Sink>, runtime: Arc<Runtime>) -> DeviceCaptureArgs {
        DeviceCaptureArgs {
            key: DeviceCaptureKey::new(id.to_string().repeat(64)).unwrap(),
            sink,
            runtime,
            mode: ChannelMode::MicAndSpeaker,
        }
    }
    fn frame() -> CaptureFrame {
        CaptureFrame {
            raw_mic: Arc::from(vec![0.125; 16_000]),
            raw_speaker: Arc::from(vec![-0.5; 16_000]),
            aec_mic: None,
        }
    }
    async fn ended(runtime: &Runtime) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if runtime
                    .events
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|event| matches!(event, SessionLifecycleEvent::Inactive { .. }))
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn device_finalizer_is_scoped_and_caller_timeout_does_not_discard_admitted_audio() {
        let local = Arc::new(Runtime::default());
        let audio = Arc::new(Audio::default());
        let first = audio.source();
        let second = audio.source();
        let (root, root_handle) = Actor::spawn(
            None,
            RootActor,
            RootArgs {
                runtime: local.clone(),
                audio,
            },
        )
        .await
        .unwrap();
        let first_runtime = Arc::new(Runtime::default());
        let second_runtime = Arc::new(Runtime::default());
        let first_sink = Sink::new(true);
        let second_sink = Sink::new(false);
        let first_args = args('1', first_sink.clone(), first_runtime.clone());
        let first_key = first_args.key.clone();
        ractor::call!(root, RootMsg::StartDeviceCapture, first_args)
            .unwrap()
            .unwrap();
        first.send(Ok(frame())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), first_sink.entered.notified())
            .await
            .unwrap();
        assert!(ractor::call!(root, RootMsg::StopDeviceCapture, first_key.clone()).unwrap());
        let second_args = args('2', second_sink.clone(), second_runtime.clone());
        let second_key = second_args.key.clone();
        ractor::call!(root, RootMsg::StartDeviceCapture, second_args)
            .unwrap()
            .unwrap();
        root.cast(RootMsg::DeviceDraining(second_key.clone(), root.get_id()))
            .unwrap();
        assert_eq!(
            ractor::call!(root, RootMsg::GetState).unwrap(),
            State::Active
        );
        // An external observation timeout does not own or cancel the recorder's pending append.
        assert!(
            tokio::time::timeout(Duration::from_millis(25), ended(&first_runtime))
                .await
                .is_err()
        );
        assert_eq!(first_sink.totals.lock().unwrap().frames, 0);
        first_sink.release();
        ended(&first_runtime).await;
        assert_eq!(first_sink.totals.lock().unwrap().frames, 16_000);
        assert_eq!(
            *first_sink.end.lock().unwrap(),
            Some(DeviceRecordingEnd::Complete)
        );
        assert_eq!(
            ractor::call!(root, RootMsg::GetState).unwrap(),
            State::Active
        );
        assert!(!ractor::call!(root, RootMsg::StopDeviceCapture, first_key).unwrap());
        let snapshot = ractor::call!(root, RootMsg::GetSnapshot).unwrap();
        assert!(snapshot.active_session_id.is_none());
        assert!(snapshot.finalizing_session_ids.is_empty());
        assert!(ractor::call!(root, RootMsg::StopDeviceCapture, second_key).unwrap());
        ended(&second_runtime).await;
        assert!(local.events.lock().unwrap().is_empty());
        drop((first, second));
        root.stop(None);
        root_handle.await.unwrap();
    }

    #[tokio::test]
    async fn storage_backpressure_stops_producer_then_drains_all_admitted_frames_without_sealing() {
        let audio = Arc::new(Audio::default());
        let frames = audio.source();
        let runtime = Arc::new(Runtime::default());
        let sink = Sink::new(true);
        let (root, handle) = Actor::spawn(
            None,
            RootActor,
            RootArgs {
                runtime: Arc::new(Runtime::default()),
                audio,
            },
        )
        .await
        .unwrap();
        ractor::call!(
            root,
            RootMsg::StartDeviceCapture,
            args('3', sink.clone(), runtime.clone())
        )
        .unwrap()
        .unwrap();
        for _ in 0..9 {
            frames.send(Ok(frame())).await.unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), sink.entered.notified())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        assert!(sink.end.lock().unwrap().is_none());
        assert_eq!(
            ractor::call!(root, RootMsg::GetState).unwrap(),
            State::Finalizing
        );
        sink.release();
        ended(&runtime).await;
        let totals = *sink.totals.lock().unwrap();
        assert_eq!(totals.frames, 128_000);
        assert_eq!(totals.chunk_count, 8);
        assert_eq!(
            *sink.end.lock().unwrap(),
            Some(DeviceRecordingEnd::Interrupted)
        );
        assert!(runtime.events.lock().unwrap().iter().any(|event| matches!(
            event,
            SessionLifecycleEvent::Inactive { error: Some(_), .. }
        )));
        root.stop(None);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn device_finalizer_admission_is_bounded() {
        let audio = Arc::new(Audio::default());
        let first = audio.source();
        let second = audio.source();
        let (root, handle) = Actor::spawn(
            None,
            RootActor,
            RootArgs {
                runtime: Arc::new(Runtime::default()),
                audio,
            },
        )
        .await
        .unwrap();
        let first_sink = Sink::new(true);
        let second_sink = Sink::new(true);
        let first_runtime = Arc::new(Runtime::default());
        let second_runtime = Arc::new(Runtime::default());
        let first_args = args('4', first_sink.clone(), first_runtime.clone());
        let first_key = first_args.key.clone();
        ractor::call!(root, RootMsg::StartDeviceCapture, first_args)
            .unwrap()
            .unwrap();
        first.send(Ok(frame())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), first_sink.entered.notified())
            .await
            .unwrap();
        assert!(ractor::call!(root, RootMsg::StopDeviceCapture, first_key).unwrap());
        let second_args = args('5', second_sink.clone(), second_runtime.clone());
        let second_key = second_args.key.clone();
        ractor::call!(root, RootMsg::StartDeviceCapture, second_args)
            .unwrap()
            .unwrap();
        second.send(Ok(frame())).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), second_sink.entered.notified())
            .await
            .unwrap();
        assert!(ractor::call!(root, RootMsg::StopDeviceCapture, second_key).unwrap());
        assert!(matches!(
            ractor::call!(
                root,
                RootMsg::StartDeviceCapture,
                args('6', Sink::new(false), Arc::new(Runtime::default()))
            )
            .unwrap(),
            Err(StartSessionError::SessionAlreadyRunning)
        ));
        first_sink.release();
        second_sink.release();
        ended(&first_runtime).await;
        ended(&second_runtime).await;
        root.stop(None);
        handle.await.unwrap();
    }
    #[tokio::test]
    async fn permanent_recorder_failure_interrupts_without_restarting_or_sealing() {
        let audio = Arc::new(Audio::default());
        let frames = audio.source();
        let runtime = Arc::new(Runtime::default());
        let sink = Sink::new(false);
        sink.fail.store(true, Ordering::SeqCst);
        let (root, handle) = Actor::spawn(
            None,
            RootActor,
            RootArgs {
                runtime: Arc::new(Runtime::default()),
                audio: audio.clone(),
            },
        )
        .await
        .unwrap();
        ractor::call!(
            root,
            RootMsg::StartDeviceCapture,
            args('7', sink.clone(), runtime.clone())
        )
        .unwrap()
        .unwrap();
        frames.send(Ok(frame())).await.unwrap();
        ended(&runtime).await;
        assert_eq!(audio.1.load(Ordering::SeqCst), 1);
        assert!(sink.end.lock().unwrap().is_none());
        assert_eq!(
            ractor::call!(root, RootMsg::GetState).unwrap(),
            State::Inactive
        );
        assert!(runtime.events.lock().unwrap().iter().any(|event| matches!(
            event,
            SessionLifecycleEvent::Inactive { error: Some(_), .. }
        )));
        root.stop(None);
        handle.await.unwrap();
    }
}
