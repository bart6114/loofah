mod pipeline;
mod stream;

use std::sync::{
    Arc,
    mpsc::{self, Receiver},
};

use ractor::{Actor, ActorName, ActorProcessingErr, ActorRef, RpcReplyPort};
use tokio_util::sync::CancellationToken;
use tracing::Instrument;

use crate::{
    ListenerRuntime, SessionErrorEvent, SessionProgressEvent,
    actors::session::session_span,
    actors::{
        ChannelMode, DeviceFinishReceipt, DeviceFrameCredit, DeviceIngress, DeviceRecordingEnd,
        DeviceSinkError, ListenerMsg, RecMsg,
    },
};
use hypr_audio::{AudioProvider, CaptureFrame};

use pipeline::Pipeline;
use stream::start_source_loop;

use hypr_device_monitor::{DeviceMonitorHandle, DeviceSwitch, DeviceSwitchMonitor};

pub enum SourceMsg {
    GetMicDevice(RpcReplyPort<Option<String>>),
    PrepareListenerRefresh(RpcReplyPort<ListenerRefreshReplay>),
    SetListenerRouting(ListenerRouting),
    SetRecorder(Option<ActorRef<RecMsg>>),
    Frame(CaptureFrame),
    DeviceFrame(CaptureFrame, DeviceFrameCredit),
    DeviceHalt(String),
    DrainDevice(RpcReplyPort<Result<DeviceFinishReceipt, DeviceSinkError>>),
    DeviceDrainReady,
    StreamFailed(String),
    RecoveryChanged { microphone: bool, speaker: bool },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListenerRefreshReplay {
    pub duration_secs: f64,
}

#[derive(Clone)]
pub enum ListenerRouting {
    Buffering,
    Attached(ActorRef<ListenerMsg>),
    Dropped,
}

pub struct SourceArgs {
    pub mic_device: Option<String>,
    pub onboarding: bool,
    pub runtime: Arc<dyn ListenerRuntime>,
    pub audio: Arc<dyn AudioProvider>,
    pub session_id: String,
    pub listener_routing: ListenerRouting,
    pub recorder: Option<ActorRef<RecMsg>>,
    pub device_ingress: Option<DeviceIngress>,
    pub capture_mode: Option<ChannelMode>,
    pub device_root: Option<(
        crate::actors::DeviceCaptureKey,
        ActorRef<crate::actors::RootMsg>,
        ractor::ActorId,
    )>,
}

pub(super) type ProviderIntegrity = Arc<std::sync::OnceLock<Arc<hypr_audio::CaptureIntegrity>>>;

pub(super) fn provider_integrity_failure(provider: Option<&ProviderIntegrity>) -> Option<String> {
    let integrity = provider?.get()?;
    if !integrity.discontinuous() {
        return None;
    }
    let (microphone, speaker) = integrity.channels();
    Some(format!(
        "Capture input lost continuity (microphone={microphone}, system audio={speaker})"
    ))
}

pub struct SourceState {
    pub(super) runtime: Arc<dyn ListenerRuntime>,
    pub(super) audio: Arc<dyn AudioProvider>,
    pub(super) session_id: String,
    pub(super) mic_device: Option<String>,
    pub(super) onboarding: bool,
    pub(super) run_task: Option<tokio::task::JoinHandle<()>>,
    pub(super) stream_cancel_token: Option<CancellationToken>,
    pub(super) current_mode: ChannelMode,
    pub(super) capture_mode: Option<ChannelMode>,
    pub(super) device_ingress: Option<DeviceIngress>,
    pub(super) provider_integrity: Option<ProviderIntegrity>,
    device_root: Option<(
        crate::actors::DeviceCaptureKey,
        ActorRef<crate::actors::RootMsg>,
        ractor::ActorId,
    )>,
    drain_started: bool,
    interrupted: Option<String>,
    device_audio_ready: bool,
    drain_reply: Option<RpcReplyPort<Result<DeviceFinishReceipt, DeviceSinkError>>>,
    pub(super) pipeline: Pipeline,
    pub(super) listener_routing: ListenerRouting,
    pub(super) recorder: Option<ActorRef<RecMsg>>,
    _device_watcher: Option<DeviceChangeWatcher>,
    _silence_stream_tx: Option<std::sync::mpsc::Sender<()>>,
}

pub struct SourceActor;

struct DeviceChangeWatcher {
    _handle: DeviceMonitorHandle,
    _thread: std::thread::JoinHandle<()>,
}

impl DeviceChangeWatcher {
    fn spawn(actor: ActorRef<SourceMsg>, device: bool) -> Self {
        let (event_tx, event_rx) = mpsc::channel();
        let handle = DeviceSwitchMonitor::spawn_debounced(event_tx);
        let thread = std::thread::spawn(move || Self::event_loop(event_rx, actor, device));

        Self {
            _handle: handle,
            _thread: thread,
        }
    }

    fn event_loop(event_rx: Receiver<DeviceSwitch>, actor: ActorRef<SourceMsg>, device: bool) {
        loop {
            match event_rx.recv() {
                Ok(DeviceSwitch::DefaultInputChanged) => {
                    tracing::info!("default_input_changed_restarting_source");
                    if device {
                        let _ = actor.cast(SourceMsg::DeviceHalt(
                            "Audio device changed; recording interrupted".into(),
                        ));
                    } else {
                        actor.stop(Some("device_change".to_string()));
                    }
                }
                #[cfg(target_os = "windows")]
                Ok(DeviceSwitch::DefaultOutputChanged { .. } | DeviceSwitch::DeviceListChanged) => {
                    tracing::info!("audio_endpoint_changed_restarting_source");
                    if device {
                        let _ = actor.cast(SourceMsg::DeviceHalt(
                            "Audio device changed; recording interrupted".into(),
                        ));
                    } else {
                        actor.stop(Some("device_change".to_string()));
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
    }
}

impl SourceActor {
    pub fn name() -> ActorName {
        "source".into()
    }
}

#[ractor::async_trait]
impl Actor for SourceActor {
    type Msg = SourceMsg;
    type State = SourceState;
    type Arguments = SourceArgs;

    async fn pre_start(
        &self,
        myself: ActorRef<Self::Msg>,
        args: Self::Arguments,
    ) -> Result<Self::State, ActorProcessingErr> {
        let session_id = args.session_id.clone();
        let span = session_span(&session_id);

        async {
            args.runtime
                .emit_progress(SessionProgressEvent::AudioInitializing {
                    session_id: session_id.clone(),
                });

            #[cfg(not(target_os = "windows"))]
            let device_watcher = Some(DeviceChangeWatcher::spawn(
                myself.clone(),
                args.device_ingress.is_some(),
            ));
            #[cfg(target_os = "windows")]
            let device_watcher = None;

            let silence_stream_tx = Some(args.audio.play_silence());
            #[cfg(target_os = "windows")]
            let mic_device = args.mic_device;
            #[cfg(not(target_os = "windows"))]
            let mic_device = args
                .mic_device
                .or_else(|| Some(args.audio.default_device_name()));
            tracing::info!(mic_device = ?mic_device);

            let pipeline = Pipeline::new(args.runtime.clone(), args.session_id.clone());
            let provider_integrity = args
                .device_ingress
                .as_ref()
                .map(|_| Arc::new(std::sync::OnceLock::new()));

            let mut st = SourceState {
                runtime: args.runtime,
                audio: args.audio,
                session_id: args.session_id,
                mic_device,
                onboarding: args.onboarding,
                run_task: None,
                stream_cancel_token: None,
                _device_watcher: device_watcher,
                _silence_stream_tx: silence_stream_tx,
                current_mode: ChannelMode::MicAndSpeaker,
                capture_mode: args.capture_mode,
                device_ingress: args.device_ingress,
                provider_integrity,
                device_root: args.device_root,
                drain_started: false,
                interrupted: None,
                device_audio_ready: false,
                drain_reply: None,
                pipeline,
                listener_routing: args.listener_routing,
                recorder: args.recorder,
            };

            start_source_loop(&myself, &mut st).await?;
            Ok(st)
        }
        .instrument(span)
        .await
    }

    async fn handle(
        &self,
        myself: ActorRef<Self::Msg>,
        msg: Self::Msg,
        st: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        let span = session_span(&st.session_id);
        let _guard = span.enter();

        match msg {
            SourceMsg::GetMicDevice(reply) => {
                if !reply.is_closed() {
                    let _ = reply.send(st.mic_device.clone());
                }
            }
            SourceMsg::PrepareListenerRefresh(reply) => {
                st.listener_routing = ListenerRouting::Buffering;
                let replay = st.pipeline.prepare_listener_refresh();
                if !reply.is_closed() {
                    let _ = reply.send(replay);
                }
            }
            SourceMsg::SetListenerRouting(routing) => {
                st.listener_routing = routing;
                st.pipeline
                    .on_listener_routing_changed(&st.listener_routing);
            }
            SourceMsg::SetRecorder(recorder) => {
                st.recorder = recorder;
            }
            SourceMsg::Frame(frame) => {
                st.pipeline.dispatch_frame(
                    frame,
                    st.current_mode,
                    &st.listener_routing,
                    st.recorder.as_ref(),
                );
            }
            SourceMsg::DeviceFrame(frame, credit) => {
                if !st.device_audio_ready {
                    st.device_audio_ready = true;
                    st.runtime.emit_progress(SessionProgressEvent::AudioReady {
                        session_id: st.session_id.clone(),
                        device: st.mic_device.clone(),
                    });
                }
                if let Err(error) = st.pipeline.dispatch_device_frame(
                    frame,
                    st.current_mode,
                    st.recorder.as_ref(),
                    credit,
                ) {
                    begin_device_drain(&myself, st, Some(error.0), None).await;
                }
            }
            SourceMsg::DeviceHalt(reason) => {
                begin_device_drain(&myself, st, Some(reason), None).await;
            }
            SourceMsg::DrainDevice(reply) => {
                if st.drain_reply.is_some() {
                    let _ = reply.send(Err(DeviceSinkError(
                        "Device recording already has a drain waiter".into(),
                    )));
                } else {
                    st.drain_reply = Some(reply);
                    begin_device_drain(&myself, st, None, None).await;
                }
            }
            SourceMsg::DeviceDrainReady => {
                if let Some(reason) = provider_integrity_failure(st.provider_integrity.as_ref()) {
                    st.interrupted.get_or_insert(reason);
                }
                let end = if st.interrupted.is_some() {
                    DeviceRecordingEnd::Interrupted
                } else {
                    DeviceRecordingEnd::Complete
                };
                let result = if let Some(recorder) = &st.recorder {
                    ractor::call!(recorder, RecMsg::DeviceFinish, end)
                        .map_err(|_| {
                            DeviceSinkError(
                                "Device recorder did not confirm its final drain".into(),
                            )
                        })
                        .and_then(|result| result)
                } else {
                    Err(DeviceSinkError(
                        "Device recorder disappeared before its final drain".into(),
                    ))
                };
                let reason = st
                    .interrupted
                    .clone()
                    .or_else(|| result.as_ref().err().map(ToString::to_string));
                if let Some(reply) = st.drain_reply.take() {
                    let _ = reply.send(result);
                }
                myself.stop(reason);
            }
            SourceMsg::StreamFailed(reason) => {
                if st.device_ingress.is_some() {
                    begin_device_drain(&myself, st, Some(reason), None).await;
                    return Ok(());
                }
                tracing::error!(%reason, "source_stream_failed_stopping");
                st.runtime.emit_error(SessionErrorEvent::AudioError {
                    session_id: st.session_id.clone(),
                    error: reason.clone(),
                    device: st.mic_device.clone(),
                    is_fatal: true,
                });
                myself.stop(Some(reason));
            }
            SourceMsg::RecoveryChanged {
                microphone,
                speaker,
            } => {
                if st.device_ingress.is_some() {
                    begin_device_drain(&myself, st, Some(format!("Capture source recovery interrupted input (microphone={microphone}, system audio={speaker})")), None).await;
                    return Ok(());
                }
                let error = match (microphone, speaker) {
                    (true, true) => {
                        "Microphone and system audio disconnected. Reconnecting; the recording contains silence until they return."
                    }
                    (true, false) => {
                        "Microphone disconnected. Reconnecting; system audio is still recording."
                    }
                    (false, true) => {
                        "System audio disconnected. Reconnecting; the microphone is still recording."
                    }
                    (false, false) => "",
                };
                st.runtime.emit_error(SessionErrorEvent::AudioError {
                    session_id: st.session_id.clone(),
                    error: error.to_string(),
                    device: st.mic_device.clone(),
                    is_fatal: false,
                });
            }
        }

        Ok(())
    }

    async fn post_stop(
        &self,
        _myself: ActorRef<Self::Msg>,
        st: &mut Self::State,
    ) -> Result<(), ActorProcessingErr> {
        if let Some(cancel_token) = st.stream_cancel_token.take() {
            cancel_token.cancel();
        }
        if let Some(task) = st.run_task.take() {
            task.abort();
        }

        Ok(())
    }
}

async fn begin_device_drain(
    myself: &ActorRef<SourceMsg>,
    st: &mut SourceState,
    reason: Option<String>,
    reply: Option<RpcReplyPort<Result<DeviceFinishReceipt, DeviceSinkError>>>,
) {
    if let Some(reason) = reason {
        st.interrupted.get_or_insert(reason);
    }
    if st.drain_started {
        return;
    }
    st.drain_started = true;
    if let Some((key, root, supervisor)) = &st.device_root {
        let _ = root.cast(crate::actors::RootMsg::DeviceDraining(
            key.clone(),
            *supervisor,
        ));
    }
    st.runtime
        .emit_lifecycle(crate::SessionLifecycleEvent::Finalizing {
            session_id: st.session_id.clone(),
        });
    if let Some(cancel) = st.stream_cancel_token.take() {
        cancel.cancel();
    }
    if let Some(task) = st.run_task.take()
        && let Err(error) = task.await
    {
        st.interrupted
            .get_or_insert(format!("Audio producer failed: {error}"));
    }
    if let Some(reason) = provider_integrity_failure(st.provider_integrity.as_ref()) {
        st.interrupted.get_or_insert(reason);
    }
    // The joined producer cannot enqueue later frames. This mailbox barrier follows all admitted frames.
    if let Some(reply) = reply {
        st.drain_reply = Some(reply);
    }
    let _ = myself.cast(SourceMsg::DeviceDrainReady);
}
