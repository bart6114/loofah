use futures_util::StreamExt;
use ractor::{ActorProcessingErr, ActorRef};
use tokio_util::sync::CancellationToken;

use crate::{SessionProgressEvent, actors::ChannelMode};
use hypr_audio::{AudioProvider, CaptureConfig, CaptureFrame, CaptureStream};
use hypr_audio_utils::chunk_size_for_stt;

use super::{ProviderIntegrity, SourceMsg, SourceState, provider_integrity_failure};

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

    let stream_cancel_token = CancellationToken::new();
    st.stream_cancel_token = Some(stream_cancel_token.clone());

    let handle = tokio::spawn(async move {
        let ctx = StreamContext {
            actor: myself2,
            cancel_token: stream_cancel_token,
            mic_device,
            audio,
            device_ingress,
            provider_integrity,
        };

        run_stream_loop(ctx, mode).await;
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

async fn run_stream_loop(ctx: StreamContext, mode: ChannelMode) {
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
            return;
        }
    };

    if let Some(slot) = &ctx.provider_integrity {
        let _ = slot.set(capture_stream.integrity());
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
