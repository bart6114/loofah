use futures_util::{FutureExt, StreamExt};
use ractor::{ActorProcessingErr, ActorRef};
use tokio_util::sync::CancellationToken;

use crate::{SessionProgressEvent, actors::ChannelMode};
use hypr_audio::{AudioProvider, CaptureConfig, CaptureFrame, CaptureStream};
use hypr_audio_utils::chunk_size_for_stt;

use super::{ProviderIntegrity, SourceMsg, SourceState, provider_integrity_failure};

#[derive(Default)]
pub(crate) struct ProducerExit {
    pub(super) resources_stopped: bool,
    pub(super) complete_frames: Option<u64>,
    pub(super) admitted_frames: u64,
    pub(super) error: Option<String>,
}

pub(super) async fn start_source_loop(
    myself: &ActorRef<SourceMsg>,
    st: &mut SourceState,
) -> Result<(), ActorProcessingErr> {
    let new_mode = st
        .capture_mode
        .unwrap_or_else(|| ChannelMode::determine(st.onboarding));

    let mode_changed = st.current_mode != new_mode;
    st.current_mode = new_mode;

    tracing::info!(?new_mode, mode_changed, "start_source_loop");

    st.pipeline.reset();

    let result = start_streams(myself, st).await;

    if result.is_ok() && st.device_ingress.is_none() {
        st.runtime.emit_progress(SessionProgressEvent::AudioReady {
            session_id: st.session_id.clone(),
            device: st.mic_device.clone(),
        });
    }

    result
}

async fn start_streams(
    myself: &ActorRef<SourceMsg>,
    st: &mut SourceState,
) -> Result<(), ActorProcessingErr> {
    let mode = st.current_mode;
    let myself2 = myself.clone();
    let mic_device = st.mic_device.clone();
    let audio = st.audio.clone();
    let device_ingress = st.device_ingress.clone();
    let provider_integrity = st.provider_integrity.clone();
    let device_root = st.device_root.clone();
    let stop_interrupted = st.stop_interrupted.clone();

    let stream_cancel_token = CancellationToken::new();
    st.stream_cancel_token = Some(stream_cancel_token.clone());
    if st.device_ingress.is_some() {
        st.device_lifetime = Some(super::DeviceLifetime {
            cancel: stream_cancel_token.clone(),
            interrupted: stop_interrupted.clone(),
        });
    }

    let handle = tokio::spawn(async move {
        let notify = myself2.clone();
        let device = device_ingress.is_some();
        let ctx = StreamContext {
            actor: myself2,
            cancel_token: stream_cancel_token,
            mic_device,
            audio,
            device_ingress,
            provider_integrity,
            stop_interrupted,
            device_root: device_root.clone(),
        };

        let outcome = std::panic::AssertUnwindSafe(run_stream_loop(ctx, mode))
            .catch_unwind()
            .await
            .unwrap_or_else(|_| ProducerExit {
                error: Some("Audio producer panicked before confirming resource stop".into()),
                ..ProducerExit::default()
            });
        if device && let Some(error) = &outcome.error {
            let _ = notify.cast(SourceMsg::DeviceHalt(error.clone()));
        }
        if outcome.resources_stopped
            && let Some((key, root, supervisor)) = device_root
        {
            let _ = root.cast(crate::actors::RootMsg::DeviceResourcesStopped(
                key, supervisor,
            ));
        }
        outcome
    });

    st.run_task = Some(handle);
    Ok(())
}

struct StreamContext {
    actor: ActorRef<SourceMsg>,
    cancel_token: CancellationToken,
    mic_device: Option<String>,
    audio: std::sync::Arc<dyn AudioProvider>,
    device_ingress: Option<crate::actors::DeviceIngress>,
    provider_integrity: Option<ProviderIntegrity>,
    stop_interrupted: std::sync::Arc<std::sync::atomic::AtomicBool>,
    device_root: Option<(
        crate::actors::DeviceCaptureKey,
        ActorRef<crate::actors::RootMsg>,
        ractor::ActorId,
    )>,
}

impl StreamContext {
    fn report_failure(&self, reason: impl Into<String>) {
        let _ = self.actor.cast(SourceMsg::StreamFailed(reason.into()));
    }

    fn is_cancelled(&self) -> bool {
        self.cancel_token.is_cancelled()
    }
}

enum StreamResult {
    Continue,
    Stop,
}

async fn run_stream_loop(ctx: StreamContext, mode: ChannelMode) -> ProducerExit {
    let sample_rate = crate::actors::SAMPLE_RATE;
    let chunk_size = chunk_size_for_stt(sample_rate);

    let capture_result: Result<CaptureStream, _> = match mode {
        ChannelMode::MicAndSpeaker => {
            let config = CaptureConfig {
                sample_rate,
                chunk_size,
                mic_device: ctx.mic_device.clone(),
                enable_aec: std::env::var("NO_AEC").as_deref() != Ok("1"),
            };
            ctx.audio.open_capture(config)
        }
        ChannelMode::SpeakerOnly => ctx.audio.open_speaker_capture(sample_rate, chunk_size),
        ChannelMode::MicOnly => {
            ctx.audio
                .open_mic_capture(ctx.mic_device.clone(), sample_rate, chunk_size)
        }
    };

    let mut capture_stream = match capture_result {
        Ok(stream) => stream,
        Err(error) => {
            ctx.report_failure(error.to_string());
            return ProducerExit {
                resources_stopped: true,
                error: Some(error.to_string()),
                ..ProducerExit::default()
            };
        }
    };

    if let Some(slot) = &ctx.provider_integrity {
        let _ = slot.set(capture_stream.integrity());
    }
    if ctx.device_ingress.is_some() {
        return run_device_stream(ctx, capture_stream).await;
    }
    loop {
        if report_integrity_failure(&ctx) {
            break;
        }
        let result = tokio::select! {
            _ = ctx.cancel_token.cancelled() => StreamResult::Stop,
            item = capture_stream.next() => handle_capture_item(&ctx, item)
        };

        if matches!(result, StreamResult::Stop) {
            break;
        }
    }
    drop(capture_stream);
    report_integrity_failure(&ctx);
    ProducerExit::default()
}

async fn run_device_stream(ctx: StreamContext, mut stream: CaptureStream) -> ProducerExit {
    let control = stream.drain_control();
    let mut outcome = ProducerExit::default();
    let mut stopping = false;
    let mut discard = false;
    loop {
        if !discard
            && (report_integrity_failure(&ctx)
                || ctx
                    .stop_interrupted
                    .load(std::sync::atomic::Ordering::Acquire))
        {
            outcome
                .error
                .get_or_insert("Capture input was interrupted".into());
            discard = true;
        }
        if discard && !stopping {
            stopping = true;
            if let Err(error) = stream.request_stop() {
                outcome.error.get_or_insert(error.to_string());
                break;
            }
        }
        let item = tokio::select! {
            _ = ctx.cancel_token.cancelled(), if !stopping => {
                stopping = true;
                discard = ctx.stop_interrupted.load(std::sync::atomic::Ordering::Acquire);
                if discard { outcome.error.get_or_insert("Capture input was interrupted".into()); }
                if let Err(error) = stream.request_stop() { outcome.error.get_or_insert(error.to_string()); break; }
                continue;
            }
            item = stream.next() => item,
        };
        if item.is_none() {
            if !stopping {
                outcome
                    .error
                    .get_or_insert("Audio input ended unexpectedly".into());
                let _ = ctx.actor.cast(SourceMsg::DeviceHalt(
                    "Audio input ended unexpectedly".into(),
                ));
                let _ = stream.request_stop();
            }
            break;
        }
        if discard {
            continue;
        }
        let frames = match &item {
            Some(Ok(frame)) => frame.raw_mic.len().max(frame.raw_speaker.len()) as u64,
            _ => 0,
        };
        let observed_at =
            (frames != 0 && outcome.admitted_frames == 0).then(std::time::SystemTime::now);
        if matches!(handle_capture_item(&ctx, item), StreamResult::Stop) {
            outcome
                .error
                .get_or_insert("Capture input was interrupted".into());
            discard = true;
        } else {
            outcome.admitted_frames += frames;
            if let Some(at) = observed_at
                && let Some((key, root, supervisor)) = &ctx.device_root
            {
                let _ = root.cast(crate::actors::RootMsg::DeviceFirstAudioObserved(
                    key.clone(),
                    *supervisor,
                    at,
                ));
            }
        }
    }
    if let Some(control) = control {
        match control.completion().await {
            Ok(_) => outcome.resources_stopped = true,
            Err(error) => {
                outcome.error.get_or_insert(error.to_string());
            }
        }
        if outcome.error.is_none() && !discard {
            match stream.drain_receipt().await {
                Ok(receipt) if receipt.frames == outcome.admitted_frames => {
                    outcome.complete_frames = Some(receipt.frames)
                }
                Ok(_) => {
                    outcome.error =
                        Some("Provider drain disagrees with admitted capture frames".into());
                }
                Err(error) => {
                    outcome.error = Some(error.to_string());
                }
            }
        }
    } else {
        outcome
            .error
            .get_or_insert("Provider cannot confirm stopping its resources".into());
    }
    drop(stream);
    if let Some(reason) = provider_integrity_failure(ctx.provider_integrity.as_ref()) {
        outcome.error.get_or_insert(reason);
    }
    if let Some(error) = &outcome.error {
        let _ = ctx.actor.cast(SourceMsg::DeviceHalt(error.clone()));
    }
    outcome
}

fn report_integrity_failure(ctx: &StreamContext) -> bool {
    if let Some(reason) = provider_integrity_failure(ctx.provider_integrity.as_ref()) {
        let _ = ctx.actor.cast(SourceMsg::DeviceHalt(reason));
        true
    } else {
        false
    }
}

fn handle_capture_item(
    ctx: &StreamContext,
    item: Option<Result<CaptureFrame, hypr_audio::Error>>,
) -> StreamResult {
    if report_integrity_failure(ctx) {
        return StreamResult::Stop;
    }
    match item {
        Some(Ok(frame)) => {
            let message = if let Some(ingress) = &ctx.device_ingress {
                let frames = frame
                    .raw_mic
                    .len()
                    .max(frame.raw_speaker.len())
                    .max(frame.aec_mic.as_ref().map_or(0, |samples| samples.len()));
                if frame
                    .raw_mic
                    .iter()
                    .chain(frame.raw_speaker.iter())
                    .chain(frame.aec_mic.iter().flat_map(|samples| samples.iter()))
                    .any(|sample| !sample.is_finite())
                {
                    let _ = ctx.actor.cast(SourceMsg::DeviceHalt(
                        "Audio input contained nonfinite PCM".into(),
                    ));
                    return StreamResult::Stop;
                }
                match ingress.admit(frames) {
                    Ok(credit) => SourceMsg::DeviceFrame(frame, credit),
                    Err(error) => {
                        let _ = ctx.actor.cast(SourceMsg::DeviceHalt(error.to_string()));
                        return StreamResult::Stop;
                    }
                }
            } else {
                SourceMsg::Frame(frame)
            };
            if ctx.actor.cast(message).is_err() {
                if !ctx.is_cancelled() {
                    tracing::debug!("failed_to_cast_capture_frame");
                }
                return StreamResult::Stop;
            }
            StreamResult::Continue
        }
        Some(Err(hypr_audio::Error::CaptureRecovering {
            microphone,
            speaker,
        })) => {
            if ctx.device_ingress.is_some() {
                let _ = ctx.actor.cast(SourceMsg::DeviceHalt(format!("Capture source recovery interrupted input (microphone={microphone}, system audio={speaker})")));
                return StreamResult::Stop;
            }
            let _ = ctx.actor.cast(SourceMsg::RecoveryChanged {
                microphone,
                speaker,
            });
            StreamResult::Continue
        }
        Some(Err(error)) => {
            tracing::error!(error.message = %error, "capture_stream_failed");
            ctx.report_failure(error.to_string());
            StreamResult::Stop
        }
        None => {
            if ctx.device_ingress.is_some() {
                let _ = ctx.actor.cast(SourceMsg::DeviceHalt(
                    "Audio input ended unexpectedly".into(),
                ));
            }
            StreamResult::Stop
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ractor::Actor;
    struct UnusedAudio;
    impl AudioProvider for UnusedAudio {
        fn open_capture(&self, _: CaptureConfig) -> Result<CaptureStream, hypr_audio::Error> {
            unreachable!()
        }
        fn open_speaker_capture(
            &self,
            _: u32,
            _: usize,
        ) -> Result<CaptureStream, hypr_audio::Error> {
            unreachable!()
        }
        fn open_mic_capture(
            &self,
            _: Option<String>,
            _: u32,
            _: usize,
        ) -> Result<CaptureStream, hypr_audio::Error> {
            unreachable!()
        }
        fn default_device_name(&self) -> String {
            unreachable!()
        }
        fn list_mic_devices(&self) -> Vec<String> {
            unreachable!()
        }
        fn play_silence(&self) -> std::sync::mpsc::Sender<()> {
            unreachable!()
        }
        fn play_bytes(&self, _: &'static [u8]) -> std::sync::mpsc::Sender<()> {
            unreachable!()
        }
        fn probe_mic(&self, _: Option<String>) -> Result<(), hypr_audio::Error> {
            unreachable!()
        }
        fn probe_speaker(&self) -> Result<(), hypr_audio::Error> {
            unreachable!()
        }
    }
    struct Probe;
    #[ractor::async_trait]
    impl Actor for Probe {
        type Msg = SourceMsg;
        type State = tokio::sync::mpsc::UnboundedSender<&'static str>;
        type Arguments = Self::State;
        async fn pre_start(
            &self,
            _: ActorRef<Self::Msg>,
            tx: Self::Arguments,
        ) -> Result<Self::State, ActorProcessingErr> {
            Ok(tx)
        }
        async fn handle(
            &self,
            _: ActorRef<Self::Msg>,
            message: Self::Msg,
            tx: &mut Self::State,
        ) -> Result<(), ActorProcessingErr> {
            let event = match message {
                SourceMsg::DeviceHalt(_) => "interrupted",
                SourceMsg::RecoveryChanged { .. } => "local recovery",
                _ => panic!("unexpected source message"),
            };
            let _ = tx.send(event);
            Ok(())
        }
    }
    #[tokio::test]
    async fn observed_eof_during_cancel_is_interrupted_but_local_recovery_remains_nonfatal() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let (actor, handle) = Actor::spawn(None, Probe, tx).await.unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let ctx = StreamContext {
            actor: actor.clone(),
            cancel_token: cancel,
            mic_device: None,
            audio: std::sync::Arc::new(UnusedAudio),
            device_ingress: Some(crate::actors::DeviceIngress::default()),
            provider_integrity: Some(std::sync::Arc::new(std::sync::OnceLock::new())),
            stop_interrupted: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            device_root: None,
        };
        assert!(matches!(
            handle_capture_item(&ctx, None),
            StreamResult::Stop
        ));
        assert_eq!(rx.recv().await, Some("interrupted"));
        let ctx = StreamContext {
            device_ingress: None,
            provider_integrity: None,
            ..ctx
        };
        assert!(matches!(
            handle_capture_item(
                &ctx,
                Some(Err(hypr_audio::Error::CaptureRecovering {
                    microphone: true,
                    speaker: false
                }))
            ),
            StreamResult::Continue
        ));
        assert_eq!(rx.recv().await, Some("local recovery"));
        actor.stop(None);
        handle.await.unwrap();
    }
}
