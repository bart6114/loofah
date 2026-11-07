use futures_util::StreamExt;
use ractor::{ActorProcessingErr, ActorRef};
use tokio_util::sync::CancellationToken;

use crate::{SessionProgressEvent, actors::ChannelMode};
use hypr_audio::{AudioProvider, CaptureConfig, CaptureFrame, CaptureStream};
use hypr_audio_utils::chunk_size_for_stt;

use super::{SourceMsg, SourceState};

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

    let stream_cancel_token = CancellationToken::new();
    st.stream_cancel_token = Some(stream_cancel_token.clone());

    let handle = tokio::spawn(async move {
        let ctx = StreamContext {
            actor: myself2,
            cancel_token: stream_cancel_token,
            mic_device,
            audio,
            device_ingress,
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

    loop {
        let result = tokio::select! {
            _ = ctx.cancel_token.cancelled() => StreamResult::Stop,
            item = capture_stream.next() => handle_capture_item(&ctx, item)
        };

        if matches!(result, StreamResult::Stop) {
            return;
        }
    }
}

fn handle_capture_item(
    ctx: &StreamContext,
    item: Option<Result<CaptureFrame, hypr_audio::Error>>,
) -> StreamResult {
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
            if ctx.device_ingress.is_some() && !ctx.is_cancelled() {
                let _ = ctx.actor.cast(SourceMsg::DeviceHalt(
                    "Audio input ended unexpectedly".into(),
                ));
            }
            StreamResult::Stop
        }
    }
}
