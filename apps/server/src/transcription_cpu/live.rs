use super::*;

use hypr_loofah_core::capture_live::{
    CaptureLiveControl, CaptureLiveEngine, CaptureLiveEngineReceipt, CaptureLiveEngineSpec,
    CaptureLiveIngress, CaptureLiveSession,
};
use hypr_loofah_protocol::{CaptureChannel, CaptureLiveCaption, CaptureLiveFailure};
use hypr_transcribe_core::live_transcript::LiveTranscriptMapper;
use hypr_transcribe_whisper_local::resources::live::{
    ChannelRole, FrameReceipt, LiveChannels, LiveCompletion, LiveController, LiveFailure,
    LiveIngressError, LiveSpec, NativeDisposition, OwnedLive,
};
use tokio::sync::{mpsc, watch};

struct Control(LiveController);

impl CaptureLiveControl for Control {
    fn try_accept(
        &self,
        first_frame: u64,
        pcm: &Arc<[f32]>,
    ) -> Result<CaptureLiveIngress, CaptureLiveFailure> {
        match self.0.try_accept_frame(first_frame, pcm) {
            Ok(FrameReceipt::Accepted { .. }) => Ok(CaptureLiveIngress::Accepted),
            Ok(FrameReceipt::Duplicate { .. }) => Ok(CaptureLiveIngress::Duplicate),
            Err(LiveIngressError::Busy) => Ok(CaptureLiveIngress::Busy),
            Err(_) => Err(CaptureLiveFailure::SourceChanged),
        }
    }

    fn finish(&self, final_frame: u64) -> Result<(), CaptureLiveFailure> {
        self.0
            .finish(final_frame)
            .map_err(|_| CaptureLiveFailure::SourceChanged)
    }

    fn cancel(&self) {
        self.0.cancel();
    }
}

impl CaptureLiveEngine for CpuTranscriptionHost {
    fn start(
        &self,
        spec: CaptureLiveEngineSpec,
        source_owner: Box<dyn Send>,
    ) -> TranscriptionFuture<'_, Result<CaptureLiveSession, CaptureLiveFailure>> {
        Box::pin(async move {
            if self.failed.load(Ordering::Acquire) {
                return Err(CaptureLiveFailure::EngineUnavailable);
            }
            hypr_loofah_protocol::validate_capture_live_request(&spec.request)
                .map_err(|_| CaptureLiveFailure::InvalidOutput)?;
            hypr_loofah_protocol::validate_capture_format(&spec.format)
                .map_err(|_| CaptureLiveFailure::SourceChanged)?;
            let generation = uuid::Uuid::parse_str(&spec.generation_id)
                .map_err(|_| CaptureLiveFailure::InvalidOutput)?;
            if generation.is_nil() || generation.to_string() != spec.generation_id {
                return Err(CaptureLiveFailure::InvalidOutput);
            }
            let mut params = parameters_for_options(
                &self.model,
                &spec.request.model,
                &spec.request.languages,
                &spec.request.keywords,
            )
            .map_err(|_| CaptureLiveFailure::EngineUnavailable)?;
            let channels = match spec.format.channels.as_slice() {
                [CaptureChannel::Microphone] => LiveChannels::Microphone,
                [CaptureChannel::SystemAudio] => LiveChannels::SystemAudio,
                [CaptureChannel::Mixed] => LiveChannels::MixedMono,
                [CaptureChannel::Microphone, CaptureChannel::SystemAudio] => {
                    LiveChannels::MicSystem
                }
                _ => return Err(CaptureLiveFailure::SourceChanged),
            };
            params.channels = channels.count() as u8;
            params.sample_rate = 16000;
            let admission =
                self.resources
                    .try_live(params.channels)
                    .map_err(|error| match error {
                        ResourceError::Busy => CaptureLiveFailure::ResourceLimit,
                        _ => CaptureLiveFailure::EngineUnavailable,
                    })?;
            if self.failed.load(Ordering::Acquire) {
                return Err(CaptureLiveFailure::EngineUnavailable);
            }
            let owner = Arc::new(Mutex::new(source_owner));
            let live = self
                .resources
                .start_live(
                    admission,
                    LiveSpec {
                        generation: *generation.as_bytes(),
                        start_frame: 0,
                        channels,
                        params,
                    },
                    owner.clone(),
                )
                .map_err(|_| CaptureLiveFailure::EngineUnavailable)?;
            let control = Arc::new(Control(live.controller()));
            let (captions, receiver) = mpsc::channel(16);
            let (completion, completed) = watch::channel(None);
            let failed = self.failed.clone();
            tokio::spawn(async move {
                if AssertUnwindSafe(run(
                    live,
                    spec.generation_id,
                    channels,
                    owner,
                    captions,
                    completion,
                    failed.clone(),
                ))
                .catch_unwind()
                .await
                .is_err()
                {
                    retain_unconfirmed(&failed).await;
                }
            });
            Ok(CaptureLiveSession {
                control,
                captions: receiver,
                completion: completed,
            })
        })
    }
}

fn failure(error: &LiveFailure) -> CaptureLiveFailure {
    match error {
        LiveFailure::Cancelled => CaptureLiveFailure::Interrupted,
        LiveFailure::OutputLost | LiveFailure::InvalidOutput => CaptureLiveFailure::InvalidOutput,
        LiveFailure::Initialization(_) => CaptureLiveFailure::EngineUnavailable,
        _ => CaptureLiveFailure::EngineFailed,
    }
}

async fn run(
    mut live: OwnedLive,
    generation: String,
    channels: LiveChannels,
    owner: SourceOwner,
    captions: mpsc::Sender<CaptureLiveCaption>,
    completed: watch::Sender<Option<Arc<CaptureLiveEngineReceipt>>>,
    failed: Arc<AtomicBool>,
) {
    let mut native = live.completion();
    let mut mapper = LiveTranscriptMapper::new(channels.count()).expect("validated channels");
    let mut raw_count = 0_u64;
    let mut count = 0_u64;
    let mut error = None;
    let mapped = AssertUnwindSafe(async {
        while let Some(caption) = live.results.recv().await {
            if caption.sequence != raw_count {
                error = Some(CaptureLiveFailure::InvalidOutput);
                live.cancel();
                break;
            }
            raw_count += 1;
            let delta = mapper.process(&caption.response);
            let languages = match caption.response.as_ref() {
                owhisper_interface::stream::StreamResponse::TranscriptResponse {
                    channel, ..
                } => channel
                    .alternatives
                    .first()
                    .map(|a| a.languages.clone())
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            let role = match caption.role {
                ChannelRole::Microphone => CaptureChannel::Microphone,
                ChannelRole::SystemAudio => CaptureChannel::SystemAudio,
                ChannelRole::Mixed => CaptureChannel::Mixed,
            };
            let event = CaptureLiveCaption {
                generation_id: generation.clone(),
                sequence: count,
                physical_channel: caption.physical_channel,
                role,
                partials: delta
                    .as_ref()
                    .map(|d| d.partials.clone())
                    .unwrap_or_default(),
                new_words: delta
                    .as_ref()
                    .map(|d| d.new_words.clone())
                    .unwrap_or_default(),
                replaced_ids: delta.map(|d| d.replaced_ids).unwrap_or_default(),
                languages,
            };
            if captions.try_send(event).is_err() {
                error = Some(CaptureLiveFailure::InvalidOutput);
                live.cancel();
                break;
            }
            count += 1;
        }
    })
    .catch_unwind()
    .await;
    if mapped.is_err() {
        error = Some(CaptureLiveFailure::EngineFailed);
        live.cancel();
    }
    // Stop consuming after a known output failure, but retain every owner until
    // the replayable native receipt proves actual join and resource destruction.
    let receipt = loop {
        if let LiveCompletion::Finished(receipt) = native.snapshot() {
            break receipt;
        }
        if native.changed().await.is_none() {
            retain_unconfirmed(&failed).await;
        }
    };
    if receipt.native == NativeDisposition::Unconfirmed || !receipt.owners_released {
        retain_unconfirmed(&failed).await;
    }
    if receipt.caption_count != raw_count {
        error.get_or_insert(CaptureLiveFailure::InvalidOutput);
    }
    if let Some(cause) = &receipt.failure {
        error.get_or_insert_with(|| failure(cause));
    }
    if error.is_none() {
        let tail = mapper.flush();
        if !tail.replaced_ids.is_empty() {
            error = Some(CaptureLiveFailure::InvalidOutput);
        }
        for physical in 0..channels.count() {
            let canonical = if channels.count() == 1 {
                2
            } else {
                physical as i32
            };
            let words: Vec<_> = tail
                .new_words
                .iter()
                .filter(|word| word.channel == canonical)
                .cloned()
                .collect();
            if words.is_empty() {
                continue;
            }
            let event = CaptureLiveCaption {
                generation_id: generation.clone(),
                sequence: count,
                physical_channel: physical as u8,
                role: match channels.role(physical).unwrap() {
                    ChannelRole::Microphone => CaptureChannel::Microphone,
                    ChannelRole::SystemAudio => CaptureChannel::SystemAudio,
                    ChannelRole::Mixed => CaptureChannel::Mixed,
                },
                new_words: words,
                partials: Vec::new(),
                replaced_ids: Vec::new(),
                languages: Vec::new(),
            };
            if captions.try_send(event).is_err() {
                error = Some(CaptureLiveFailure::InvalidOutput);
                break;
            }
            count += 1;
        }
    }
    drop(live);
    drop(mapper);
    drop(owner);
    completed.send_replace(Some(Arc::new(CaptureLiveEngineReceipt {
        generation_id: generation,
        admitted_end_frame: receipt.admitted_end_frame,
        input_consumed_end_frame: receipt.input_consumed_end_frame,
        completed_through_frame: error.is_none().then_some(receipt.admitted_end_frame),
        caption_count: count,
        failure: error,
        native_joined: receipt.native == NativeDisposition::Joined,
        owners_released: true,
    })));
}
