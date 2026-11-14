use std::sync::Arc;
use std::time::Instant;

use owhisper_client::{
    AdapterKind, AquaVoiceAdapter, AssemblyAIAdapter, BatchSttAdapter, CartesiaAdapter,
    DeepgramAdapter, ElevenLabsAdapter, FireworksAdapter, FmtrAdapter, GladiaAdapter,
    MistralAdapter, OpenAIAdapter, PyannoteAdapter, SonioxAdapter,
};
use owhisper_interface::batch_stream::BatchStreamEvent;
use tracing::Instrument;

use hypr_audio_chunking::AudioChunk;
use hypr_fs_format::{BatchCompletionStatus, FailedAudioRange};
use hypr_transcribe_core::{TARGET_SAMPLE_RATE, chunk_pcm_channel};

use super::diarize::{SharedDiarization, stamp_batch_response};
use super::{BatchParams, BatchRunMode, BatchRunOutput, format_user_friendly_error, session_span};
use crate::{BatchEvent, BatchRuntime};

const SONIQO_PARAKEET_MAX_CHUNK_SAMPLES: usize = TARGET_SAMPLE_RATE as usize * 59 / 2;
const SONIQO_DIRECT_MIC_MIN_RMS: f64 = 0.0008;
const SONIQO_PROGRESS_PLANNED: f64 = 0.05;
const SONIQO_PROGRESS_RANGE: f64 = 0.90;
const SONIQO_PROGRESS_MAX: f64 = 0.95;

macro_rules! dispatch_batch {
    ($ak:expr, $params:expr, $lp:expr, $diarization:expr,
     { $($var:ident => $adapter:ty),+ $(,)? },
     unsupported: [$($unsup:ident),* $(,)?]
    ) => {
        match $ak {
            $(AdapterKind::$var => {
                run_direct_batch::<$adapter>(
                    &AdapterKind::$var.to_string(),
                    $params,
                    $lp,
                    $diarization,
                ).await
            })+
            $(AdapterKind::$unsup => {
                Err(crate::BatchFailure::DirectBatchUnsupported {
                    provider: AdapterKind::$unsup.to_string(),
                }.into())
            })*
        }
    };
}

pub(super) async fn run_direct_batch_for_adapter_kind(
    adapter_kind: AdapterKind,
    params: BatchParams,
    listen_params: owhisper_interface::ListenParams,
    diarization: SharedDiarization,
) -> crate::Result<BatchRunOutput> {
    dispatch_batch!(adapter_kind, params, listen_params, diarization, {
        Cartesia => CartesiaAdapter,
        Deepgram => DeepgramAdapter,
        Soniox => SonioxAdapter,
        AssemblyAI => AssemblyAIAdapter,
        Fireworks => FireworksAdapter,
        OpenAI => OpenAIAdapter,
        Gladia => GladiaAdapter,
        ElevenLabs => ElevenLabsAdapter,
        Pyannote => PyannoteAdapter,
        Mistral => MistralAdapter,
        Fmtr => FmtrAdapter,
        AquaVoice => AquaVoiceAdapter,
    }, unsupported: [DashScope])
}

async fn run_direct_batch<A: BatchSttAdapter>(
    provider: &str,
    params: BatchParams,
    listen_params: owhisper_interface::ListenParams,
    diarization: SharedDiarization,
) -> crate::Result<BatchRunOutput> {
    let span = session_span(&params.session_id);

    async {
        let client = owhisper_client::BatchClient::<A>::builder()
            .api_base(params.base_url.clone())
            .api_key(params.api_key.clone())
            .params(listen_params)
            .build();

        tracing::debug!("transcribing file: {}", params.file_path);
        let mut response = match client.transcribe_file(&params.file_path).await {
            Ok(response) => response,
            Err(err) => {
                let raw_error = format!("{err:?}");
                let message = format_user_friendly_error(&raw_error);
                tracing::error!(
                    error = %raw_error,
                    fmtr.error.user_message = %message,
                    "batch transcription failed"
                );
                return Err(crate::BatchFailure::DirectRequestFailed {
                    provider: provider.to_string(),
                    message,
                }
                .into());
            }
        };
        tracing::info!("batch transcription completed");

        stamp_batch_response(&mut response, &*diarization.segments().await?);

        Ok(BatchRunOutput {
            session_id: params.session_id,
            mode: BatchRunMode::Direct,
            status: hypr_fs_format::BatchCompletionStatus::Complete,
            unresolved: Vec::new(),
            response,
        })
    }
    .instrument(span)
    .await
}

pub(super) async fn run_soniqo_batch(
    runtime: Arc<dyn BatchRuntime>,
    params: BatchParams,
    listen_params: owhisper_interface::ListenParams,
    diarization: SharedDiarization,
    mut recovery: hypr_fs_format::BatchRecovery,
) -> crate::Result<BatchRunOutput> {
    let model = recovery
        .model
        .parse::<hypr_transcribe_soniqo::SoniqoModel>()
        .map_err(|e| crate::Error::BatchError(e.to_string()))?;
    if !model.supports_languages(&listen_params.languages) {
        return Err(crate::Error::BatchError(
            "This model does not support the selected meeting languages.".into(),
        ));
    }
    let segments = match diarization.segments().await {
        Ok(segments) => segments,
        Err(error) => {
            let pcm = hypr_audio_utils::PcmDescriptor::open(&params.file_path)
                .map_err(|e| crate::Error::BatchError(e.to_string()))?;
            recovery.diarization_pending = true;
            recovery.status = BatchCompletionStatus::Failed;
            recovery.unresolved = (0..pcm.channels)
                .map(|channel| FailedAudioRange {
                    channel: channel as u32,
                    start_sample: 0,
                    end_sample: pcm.frames as u64,
                    reason: error.to_string(),
                })
                .collect();
            let mut response = hypr_transcribe_soniqo::batch_response_from_channels(
                model,
                (0..pcm.channels)
                    .map(|_| {
                        hypr_transcribe_soniqo::FileTranscript::new(String::new(), pcm.duration())
                    })
                    .collect(),
            );
            response.metadata["batch_recovery"] = serde_json::to_value(&recovery)
                .map_err(|e| crate::Error::BatchError(e.to_string()))?;
            response.metadata["batch_checkpoint_initial"] = serde_json::json!(true);
            let mut output = BatchRunOutput {
                session_id: params.session_id,
                mode: BatchRunMode::Direct,
                status: recovery.status,
                unresolved: recovery.unresolved.clone(),
                response,
            };
            runtime
                .checkpoint(&mut output)
                .await
                .map_err(crate::Error::BatchError)?;
            return Ok(output);
        }
    };
    recovery.planning_pending = false;
    recovery.diarization_pending = false;
    recovery.diarization = segments
        .iter()
        .flat_map(|(channel, segments)| {
            segments
                .iter()
                .map(move |segment| hypr_fs_format::RecoverySpeakerSegment {
                    channel: *channel as u32,
                    start_ms: segment.start_ms,
                    end_ms: segment.end_ms,
                    speaker_index: segment.speaker_index,
                })
        })
        .collect();
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let language = model.language_config(&listen_params.languages);
        let pcm =
            hypr_audio_utils::PcmDescriptor::open(&params.file_path).map_err(|e| e.to_string())?;
        let mut plans = Vec::new();
        let mut outcome = SoniqoOutcome {
            channels: (0..pcm.channels)
                .map(|_| hypr_transcribe_soniqo::FileTranscript::new(String::new(), pcm.duration()))
                .collect(),
            unresolved: Vec::new(),
            processed_chunks: 0,
            cancelled: false,
        };
        for channel in 0..pcm.channels {
            match soniqo_channel_plan(
                model,
                channel,
                &pcm,
                pcm.channels == 2 && channel == 0,
                || runtime.is_cancelled(),
            ) {
                Ok(plan) => {
                    if plan.chunks.is_empty() {
                        outcome.processed_chunks += 1;
                    }
                    for chunk in plan.chunks {
                        outcome.unresolved.push(FailedAudioRange {
                            channel: channel as u32,
                            start_sample: chunk.sample_start as u64,
                            end_sample: chunk.sample_end as u64,
                            reason: "Audio has not been processed yet.".into(),
                        });
                        plans.push((channel, plan.is_direct_mic, chunk));
                    }
                }
                Err(reason) => outcome.unresolved.push(FailedAudioRange {
                    channel: channel as u32,
                    start_sample: 0,
                    end_sample: pcm.frames as u64,
                    reason,
                }),
            }
        }
        let checkpoint =
            |outcome: &SoniqoOutcome, initial: bool| -> Result<BatchRunOutput, String> {
                let mut response = hypr_transcribe_soniqo::batch_response_from_channels(
                    model,
                    outcome.channels.clone(),
                );
                stamp_batch_response(&mut response, &segments);
                let mut recovery = recovery.clone();
                recovery.status = outcome.status();
                recovery.unresolved = outcome.unresolved.clone();
                response.metadata.as_object_mut().unwrap().insert(
                    "batch_recovery".into(),
                    serde_json::to_value(recovery).map_err(|e| e.to_string())?,
                );
                response.metadata.as_object_mut().unwrap().insert(
                    "batch_checkpoint_initial".into(),
                    serde_json::Value::Bool(initial),
                );
                let mut output = BatchRunOutput {
                    session_id: params.session_id.clone(),
                    mode: BatchRunMode::Direct,
                    status: outcome.status(),
                    unresolved: outcome.unresolved.clone(),
                    response,
                };
                handle.block_on(runtime.checkpoint(&mut output))?;
                Ok(output)
            };
        outcome.cancelled = runtime.is_cancelled();
        let mut output = checkpoint(&outcome, false)?;
        let count = plans.len();
        for (index, (channel, is_direct_mic, chunk)) in plans.into_iter().enumerate() {
            if runtime.is_cancelled() {
                outcome.cancelled = true;
                for range in &mut outcome.unresolved {
                    range.reason = "Transcription cancelled".into();
                }
                output = checkpoint(&outcome, false)?;
                break;
            }
            let (start, end) = (chunk.sample_start as u64, chunk.sample_end as u64);
            let decoded = transcribe_soniqo_channel_chunks(
                model,
                SoniqoChannelPlan {
                    channel_index: channel,
                    duration_seconds: pcm.duration(),
                    is_direct_mic,
                    chunks: vec![chunk],
                },
                &pcm,
                &language,
                || runtime.is_cancelled(),
                || {},
            );
            if decoded.unresolved.is_empty() {
                outcome.unresolved.retain(|r| {
                    !(r.channel == channel as u32 && r.start_sample == start && r.end_sample == end)
                });
                outcome.processed_chunks += decoded.processed_chunks;
                let saved = &mut outcome.channels[channel];
                saved.chunks.extend(decoded.transcript.chunks);
                saved.text = saved
                    .chunks
                    .iter()
                    .map(|part| part.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
            } else if let Some(range) = outcome.unresolved.iter_mut().find(|r| {
                r.channel == channel as u32 && r.start_sample == start && r.end_sample == end
            }) {
                range.reason = decoded.unresolved[0].reason.clone();
            }
            outcome.cancelled |= decoded.cancelled;
            output = checkpoint(&outcome, false)?;
            runtime.emit(BatchEvent::BatchResponseStreamed {
                session_id: params.session_id.clone(),
                event: BatchStreamEvent::Progress {
                    percentage: soniqo_batch_progress(index + 1, count),
                    partial_text: None,
                },
            });
        }
        Ok::<_, String>(output)
    })
    .await
    .map_err(|e| crate::Error::BatchError(e.to_string()))?
    .map_err(crate::Error::BatchError)
}

struct SoniqoChannelPlan {
    channel_index: usize,
    duration_seconds: f64,
    is_direct_mic: bool,
    chunks: Vec<ChannelChunk>,
}

struct ChannelChunk {
    samples: Vec<f32>,
    sample_start: usize,
    sample_end: usize,
    speech_spans: Vec<(usize, usize)>,
}

impl From<AudioChunk> for ChannelChunk {
    fn from(chunk: AudioChunk) -> Self {
        Self {
            samples: chunk.samples,
            sample_start: chunk.sample_start,
            sample_end: chunk.sample_end,
            speech_spans: Vec::new(),
        }
    }
}

#[cfg(test)]
fn uses_resilient_soniqo_chunking(model: hypr_transcribe_soniqo::SoniqoModel) -> bool {
    matches!(
        model,
        hypr_transcribe_soniqo::SoniqoModel::ParakeetBatch
            | hypr_transcribe_soniqo::SoniqoModel::OnnxParakeetBatch
            | hypr_transcribe_soniqo::SoniqoModel::NemotronStreaming
    )
}

fn soniqo_batch_progress(completed_chunks: usize, total_chunks: usize) -> f64 {
    if total_chunks == 0 {
        return SONIQO_PROGRESS_PLANNED;
    }

    let ratio = completed_chunks as f64 / total_chunks as f64;
    (SONIQO_PROGRESS_PLANNED + ratio * SONIQO_PROGRESS_RANGE).min(SONIQO_PROGRESS_MAX)
}

struct SoniqoChannelOutcome {
    transcript: hypr_transcribe_soniqo::FileTranscript,
    unresolved: Vec<FailedAudioRange>,
    processed_chunks: usize,
    cancelled: bool,
}

impl SoniqoChannelOutcome {
    fn failed(channel: usize, frames: usize, reason: String) -> Self {
        Self {
            transcript: hypr_transcribe_soniqo::FileTranscript::new(
                String::new(),
                frames as f64 / TARGET_SAMPLE_RATE as f64,
            ),
            unresolved: vec![FailedAudioRange {
                channel: channel as u32,
                start_sample: 0,
                end_sample: frames as u64,
                reason,
            }],
            processed_chunks: 0,
            cancelled: false,
        }
    }
}

struct SoniqoOutcome {
    channels: Vec<hypr_transcribe_soniqo::FileTranscript>,
    unresolved: Vec<FailedAudioRange>,
    processed_chunks: usize,
    cancelled: bool,
}

impl SoniqoOutcome {
    fn status(&self) -> BatchCompletionStatus {
        if self.cancelled {
            BatchCompletionStatus::Cancelled
        } else if self.unresolved.is_empty() {
            BatchCompletionStatus::Complete
        } else if self.processed_chunks == 0 {
            BatchCompletionStatus::Failed
        } else {
            BatchCompletionStatus::Incomplete
        }
    }
}

#[cfg(test)]
fn collect_soniqo_channel_transcripts(
    transcripts: impl IntoIterator<Item = SoniqoChannelOutcome>,
) -> SoniqoOutcome {
    let mut output = SoniqoOutcome {
        channels: Vec::new(),
        unresolved: Vec::new(),
        processed_chunks: 0,
        cancelled: false,
    };
    for channel in transcripts {
        output.channels.push(channel.transcript);
        output.unresolved.extend(channel.unresolved);
        output.processed_chunks += channel.processed_chunks;
        output.cancelled |= channel.cancelled;
    }
    output
}

fn soniqo_channel_plan(
    model: hypr_transcribe_soniqo::SoniqoModel,
    channel_index: usize,
    pcm: &hypr_audio_utils::PcmDescriptor,
    is_direct_mic: bool,
    cancelled: impl Fn() -> bool,
) -> std::result::Result<SoniqoChannelPlan, String> {
    let duration_seconds = pcm.duration();
    let raw = chunk_pcm_channel::<crate::Error>(pcm, channel_index, &cancelled);
    let chunks = if model == hypr_transcribe_soniqo::SoniqoModel::ParakeetBatch {
        match raw {
            Ok(chunks) => pack_speech_ranges(pcm.frames, chunks, SONIQO_PARAKEET_MAX_CHUNK_SAMPLES),
            Err(error) if !cancelled() => {
                tracing::warn!(%error, "soniqo_speech_chunking_failed_using_fixed_windows");
                (0..pcm.frames)
                    .step_by(SONIQO_PARAKEET_MAX_CHUNK_SAMPLES)
                    .map(|start| ChannelChunk {
                        samples: Vec::new(),
                        sample_start: start,
                        sample_end: (start + SONIQO_PARAKEET_MAX_CHUNK_SAMPLES).min(pcm.frames),
                        speech_spans: Vec::new(),
                    })
                    .collect()
            }
            Err(error) => return Err(error.to_string()),
        }
    } else {
        raw.map_err(|e| e.to_string())?
            .into_iter()
            .map(ChannelChunk::from)
            .collect()
    };
    tracing::info!(
        fmtr.stt.provider.name = "soniqo",
        fmtr.stt.model = %model,
        channel.index = channel_index,
        channel.duration_seconds = duration_seconds,
        channel.sample_count = pcm.frames,
        chunk.count = chunks.len(),
        "soniqo_channel_chunked"
    );

    Ok(SoniqoChannelPlan {
        channel_index,
        duration_seconds,
        is_direct_mic,
        chunks,
    })
}

fn transcribe_soniqo_channel_chunks(
    model: hypr_transcribe_soniqo::SoniqoModel,
    plan: SoniqoChannelPlan,
    pcm: &hypr_audio_utils::PcmDescriptor,
    language: &hypr_transcribe_soniqo::LanguageConfig,
    cancelled: impl Fn() -> bool,
    on_chunk_completed: impl FnMut(),
) -> SoniqoChannelOutcome {
    transcribe_soniqo_channel_chunks_with(
        model,
        plan,
        pcm,
        cancelled,
        on_chunk_completed,
        |samples| transcribe_soniqo_samples(model, samples, language),
    )
}

fn transcribe_soniqo_channel_chunks_with(
    model: hypr_transcribe_soniqo::SoniqoModel,
    plan: SoniqoChannelPlan,
    pcm: &hypr_audio_utils::PcmDescriptor,
    cancelled: impl Fn() -> bool,
    mut on_chunk_completed: impl FnMut(),
    mut decode: impl FnMut(&[f32]) -> Result<hypr_transcribe_soniqo::FileTranscript, String>,
) -> SoniqoChannelOutcome {
    let mut texts = Vec::new();
    let mut transcript_chunks = Vec::new();
    let mut successful_chunks = 0usize;
    let mut unresolved = Vec::new();
    let mut was_cancelled = false;
    let channel_index = plan.channel_index;
    let is_direct_mic = plan.is_direct_mic;

    let mut reader = match pcm.reader() {
        Ok(reader) => reader,
        Err(error) => {
            return SoniqoChannelOutcome::failed(channel_index, pcm.frames, error.to_string());
        }
    };
    for (chunk_index, mut chunk) in plan.chunks.into_iter().enumerate() {
        let failure = |reason| FailedAudioRange {
            channel: channel_index as u32,
            start_sample: chunk.sample_start as u64,
            end_sample: chunk.sample_end as u64,
            reason,
        };
        if cancelled() {
            was_cancelled = true;
            unresolved.push(failure("Transcription cancelled".into()));
            continue;
        }
        chunk.samples = match reader.channel(channel_index, chunk.sample_start..chunk.sample_end) {
            Ok(samples) => samples,
            Err(error) => {
                unresolved.push(failure(error.to_string()));
                on_chunk_completed();
                continue;
            }
        };
        let chunk_duration_ms =
            (chunk.sample_end - chunk.sample_start) * 1000 / TARGET_SAMPLE_RATE as usize;
        let chunk_rms = chunk_speech_rms(&chunk);
        if is_direct_mic && chunk_rms < SONIQO_DIRECT_MIC_MIN_RMS {
            successful_chunks += 1;
            on_chunk_completed();
            tracing::info!(
                fmtr.stt.provider.name = "soniqo",
                fmtr.stt.model = %model,
                channel.index = channel_index,
                chunk.index = chunk_index,
                audio.rms = chunk_rms,
                audio.minimum_rms = SONIQO_DIRECT_MIC_MIN_RMS,
                "soniqo_direct_mic_chunk_skipped"
            );
            continue;
        }

        let chunk_started_at = Instant::now();
        tracing::info!(
            fmtr.stt.provider.name = "soniqo",
            fmtr.stt.model = %model,
            channel.index = channel_index,
            chunk.index = chunk_index,
            chunk.sample_start = chunk.sample_start,
            chunk.sample_end = chunk.sample_end,
            chunk.sample_count = chunk.samples.len(),
            chunk.duration_ms = chunk_duration_ms,
            "soniqo_chunk_native_inference_start"
        );

        let transcript = match decode(&chunk.samples) {
            Ok(transcript) => {
                successful_chunks += 1;
                transcript
            }
            Err(e) => {
                unresolved.push(failure(e.clone()));
                tracing::warn!(
                    fmtr.stt.provider.name = "soniqo",
                    fmtr.stt.model = %model,
                    channel.index = channel_index,
                    chunk.index = chunk_index,
                    elapsed_ms = chunk_started_at.elapsed().as_millis() as u64,
                    error = %e,
                    "soniqo_chunk_native_inference_failed"
                );
                on_chunk_completed();
                continue;
            }
        };
        on_chunk_completed();

        tracing::info!(
            fmtr.stt.provider.name = "soniqo",
            fmtr.stt.model = %model,
            channel.index = channel_index,
            chunk.index = chunk_index,
            elapsed_ms = chunk_started_at.elapsed().as_millis() as u64,
            transcript.text_chars = transcript.text.chars().count(),
            "soniqo_chunk_native_inference_completed"
        );

        if model.resolved() == hypr_transcribe_soniqo::SoniqoModel::OnnxParakeetBatch
            && !transcript.chunks.is_empty()
        {
            let offset = chunk.sample_start as f64 / TARGET_SAMPLE_RATE as f64;
            for mut part in transcript.chunks {
                let real_duration =
                    (chunk.sample_end - chunk.sample_start) as f64 / TARGET_SAMPLE_RATE as f64;
                part.start_seconds = part.start_seconds.clamp(0.0, real_duration);
                part.duration_seconds = part
                    .duration_seconds
                    .clamp(0.0, real_duration - part.start_seconds);
                part.start_seconds += offset;
                for span in &mut part.speech_spans {
                    span.start_seconds += offset;
                    span.end_seconds += offset;
                }
                transcript_chunks.push(part);
            }
            continue;
        }
        let text = transcript.text.trim();
        if !text.is_empty() {
            texts.push(text.to_string());
            transcript_chunks.push(hypr_transcribe_soniqo::FileTranscriptChunk {
                text: text.to_string(),
                start_seconds: chunk.sample_start as f64 / TARGET_SAMPLE_RATE as f64,
                duration_seconds: (chunk.sample_end - chunk.sample_start) as f64
                    / TARGET_SAMPLE_RATE as f64,
                speech_spans: chunk
                    .speech_spans
                    .iter()
                    .map(|&(start, end)| hypr_transcribe_soniqo::SpeechSpan {
                        start_seconds: start as f64 / TARGET_SAMPLE_RATE as f64,
                        end_seconds: end as f64 / TARGET_SAMPLE_RATE as f64,
                    })
                    .collect(),
            });
        }
    }

    // A VAD-empty channel is successfully processed silence.
    if successful_chunks == 0 && unresolved.is_empty() {
        successful_chunks = 1;
    }
    let transcript = if transcript_chunks.is_empty() {
        hypr_transcribe_soniqo::FileTranscript::new(texts.join(" "), plan.duration_seconds)
    } else {
        hypr_transcribe_soniqo::FileTranscript::from_chunks(
            transcript_chunks,
            plan.duration_seconds,
        )
    };
    SoniqoChannelOutcome {
        transcript,
        unresolved,
        processed_chunks: successful_chunks,
        cancelled: was_cancelled,
    }
}

pub(super) fn decode_recovery_range(
    model: hypr_transcribe_soniqo::SoniqoModel,
    pcm: &hypr_audio_utils::PcmDescriptor,
    range: &FailedAudioRange,
    language: &hypr_transcribe_soniqo::LanguageConfig,
) -> Result<Vec<hypr_transcribe_soniqo::FileTranscript>, String> {
    decode_recovery_range_with(model, pcm, range, |samples| {
        transcribe_soniqo_samples(model, samples, language)
    })
}

fn decode_recovery_range_with(
    model: hypr_transcribe_soniqo::SoniqoModel,
    pcm: &hypr_audio_utils::PcmDescriptor,
    range: &FailedAudioRange,
    decode: impl FnMut(&[f32]) -> Result<hypr_transcribe_soniqo::FileTranscript, String>,
) -> Result<Vec<hypr_transcribe_soniqo::FileTranscript>, String> {
    let cap = SONIQO_PARAKEET_MAX_CHUNK_SAMPLES;
    let chunks = (range.start_sample as usize..range.end_sample as usize)
        .step_by(cap)
        .map(|start| {
            pack_from_range(
                start,
                (start + cap).min(range.end_sample as usize),
                Vec::new(),
            )
        })
        .collect();
    let result = transcribe_soniqo_channel_chunks_with(
        model,
        SoniqoChannelPlan {
            channel_index: range.channel as usize,
            duration_seconds: pcm.duration(),
            is_direct_mic: false,
            chunks,
        },
        pcm,
        || false,
        || {},
        decode,
    );
    if !result.unresolved.is_empty() {
        return Err(result.unresolved[0].reason.clone());
    }
    let mut channels: Vec<_> = (0..pcm.channels)
        .map(|_| hypr_transcribe_soniqo::FileTranscript::new(String::new(), pcm.duration()))
        .collect();
    channels[range.channel as usize] = result.transcript;
    Ok(channels)
}

fn transcribe_soniqo_samples(
    model: hypr_transcribe_soniqo::SoniqoModel,
    samples: &[f32],
    language: &hypr_transcribe_soniqo::LanguageConfig,
) -> std::result::Result<hypr_transcribe_soniqo::FileTranscript, String> {
    let file = tempfile::Builder::new()
        .prefix("soniqo_channel_")
        .suffix(".wav")
        .tempfile()
        .map_err(|e| e.to_string())?;
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: TARGET_SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    {
        let mut writer = hound::WavWriter::create(file.path(), spec).map_err(|e| e.to_string())?;
        for sample in samples {
            writer.write_sample(*sample).map_err(|e| e.to_string())?;
        }
        writer.finalize().map_err(|e| e.to_string())?;
    }

    hypr_transcribe_soniqo::transcribe_file_with_config(model, file.path(), language)
        .map_err(|e| e.to_string())
}

fn pack_speech_ranges(
    frame_count: usize,
    chunks: Vec<AudioChunk>,
    max_samples: usize,
) -> Vec<ChannelChunk> {
    let mut packs: Vec<ChannelChunk> = Vec::new();
    let mut current: Option<(usize, usize, Vec<(usize, usize)>)> = None;

    for chunk in chunks {
        let span = (
            chunk.sample_start.min(frame_count),
            chunk.sample_end.min(frame_count),
        );
        match current.as_mut() {
            Some((start, end, spans)) if span.1.saturating_sub(*start) <= max_samples => {
                *end = span.1.max(*end);
                spans.push(span);
            }
            _ => {
                if let Some((start, end, spans)) = current.take() {
                    packs.push(pack_from_range(start, end, spans));
                }
                current = Some((span.0, span.1, vec![span]));
            }
        }
    }

    if let Some((start, end, spans)) = current.take() {
        packs.push(pack_from_range(start, end, spans));
    }

    packs
}

fn pack_from_range(
    sample_start: usize,
    sample_end: usize,
    speech_spans: Vec<(usize, usize)>,
) -> ChannelChunk {
    ChannelChunk {
        samples: Vec::new(),
        sample_start,
        sample_end,
        speech_spans,
    }
}

fn chunk_speech_rms(chunk: &ChannelChunk) -> f64 {
    if chunk.speech_spans.is_empty() {
        return audio_rms(&chunk.samples);
    }

    let mut sum = 0.0f64;
    let mut count = 0usize;
    for &(start, end) in &chunk.speech_spans {
        let local_start = start
            .saturating_sub(chunk.sample_start)
            .min(chunk.samples.len());
        let local_end = end
            .saturating_sub(chunk.sample_start)
            .min(chunk.samples.len());
        for sample in &chunk.samples[local_start..local_end] {
            sum += f64::from(*sample) * f64::from(*sample);
            count += 1;
        }
    }

    if count == 0 {
        0.0
    } else {
        (sum / count as f64).sqrt()
    }
}

fn audio_rms(samples: &[f32]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }

    let sum = samples
        .iter()
        .map(|sample| f64::from(*sample) * f64::from(*sample))
        .sum::<f64>();
    (sum / samples.len() as f64).sqrt()
}

#[cfg(test)]
fn pack_speech_chunks(
    samples: &[f32],
    chunks: Vec<AudioChunk>,
    max_samples: usize,
) -> Vec<ChannelChunk> {
    pack_speech_ranges(samples.len(), chunks, max_samples)
        .into_iter()
        .map(|mut chunk| {
            chunk.samples = samples[chunk.sample_start..chunk.sample_end].to_vec();
            chunk
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn speech_chunk(start_seconds: usize, end_seconds: usize) -> AudioChunk {
        let rate = TARGET_SAMPLE_RATE as usize;
        AudioChunk {
            samples: vec![0.5; (end_seconds - start_seconds) * rate],
            sample_start: start_seconds * rate,
            sample_end: end_seconds * rate,
        }
    }

    #[test]
    fn packs_adjacent_speech_chunks_into_one_window() {
        let rate = TARGET_SAMPLE_RATE as usize;
        let samples = vec![0.1; rate * 60];
        let packs = pack_speech_chunks(
            &samples,
            vec![
                speech_chunk(2, 6),
                speech_chunk(10, 14),
                speech_chunk(20, 25),
            ],
            SONIQO_PARAKEET_MAX_CHUNK_SAMPLES,
        );

        assert_eq!(packs.len(), 1);
        assert_eq!(packs[0].sample_start, rate * 2);
        assert_eq!(packs[0].sample_end, rate * 25);
        assert_eq!(
            packs[0].speech_spans,
            vec![
                (rate * 2, rate * 6),
                (rate * 10, rate * 14),
                (rate * 20, rate * 25)
            ]
        );
        assert_eq!(packs[0].samples.len(), rate * 23);
    }

    #[test]
    fn starts_new_pack_when_window_capacity_is_exceeded() {
        let rate = TARGET_SAMPLE_RATE as usize;
        let samples = vec![0.1; rate * 120];
        let packs = pack_speech_chunks(
            &samples,
            vec![
                speech_chunk(0, 20),
                speech_chunk(40, 50),
                speech_chunk(52, 56),
            ],
            SONIQO_PARAKEET_MAX_CHUNK_SAMPLES,
        );

        assert_eq!(packs.len(), 2);
        assert_eq!(packs[0].sample_start, 0);
        assert_eq!(packs[0].sample_end, rate * 20);
        assert_eq!(packs[1].sample_start, rate * 40);
        assert_eq!(packs[1].sample_end, rate * 56);
        assert_eq!(
            packs[1].speech_spans,
            vec![(rate * 40, rate * 50), (rate * 52, rate * 56)]
        );
    }

    #[test]
    fn speech_rms_ignores_silence_between_spans() {
        let rate = TARGET_SAMPLE_RATE as usize;
        let mut samples = vec![0.0f32; rate * 10];
        samples[rate * 2..rate * 3].fill(0.5);
        let packs = pack_speech_chunks(
            &samples,
            vec![speech_chunk(2, 3), speech_chunk(8, 9)],
            SONIQO_PARAKEET_MAX_CHUNK_SAMPLES,
        );
        let pack = packs.into_iter().next().unwrap();

        let rms = chunk_speech_rms(&pack);
        assert!((rms - (0.125f64).sqrt()).abs() < 1e-6);
        assert!(audio_rms(&pack.samples) < rms);
    }

    #[test]
    fn direct_mic_rms_floor_keeps_audible_speech() {
        assert!(audio_rms(&[0.0; 16_000]) < SONIQO_DIRECT_MIC_MIN_RMS);
        assert!(audio_rms(&[0.01; 16_000]) >= SONIQO_DIRECT_MIC_MIN_RMS);
    }

    #[test]
    fn parakeet_batch_window_bounds_force_coreml_shape_3000() {
        let minimum_mel_frames = TARGET_SAMPLE_RATE as usize * 20 / 160 + 1;
        let maximum_mel_frames = SONIQO_PARAKEET_MAX_CHUNK_SAMPLES / 160 + 1;

        assert!(minimum_mel_frames > 2000);
        assert!(maximum_mel_frames <= 3000);
    }

    #[test]
    fn parakeet_batch_uses_resilient_chunking() {
        assert!(uses_resilient_soniqo_chunking(
            hypr_transcribe_soniqo::SoniqoModel::ParakeetBatch
        ));
        assert!(!uses_resilient_soniqo_chunking(
            hypr_transcribe_soniqo::SoniqoModel::Omnilingual
        ));
    }

    #[test]
    fn soniqo_progress_starts_after_chunk_planning() {
        assert_eq!(soniqo_batch_progress(0, 10), SONIQO_PROGRESS_PLANNED);
        assert_eq!(soniqo_batch_progress(0, 0), SONIQO_PROGRESS_PLANNED);
    }

    #[test]
    fn soniqo_progress_caps_before_completion() {
        assert!((soniqo_batch_progress(5, 10) - 0.5).abs() < 1e-9);
        assert_eq!(soniqo_batch_progress(10, 10), SONIQO_PROGRESS_MAX);
        assert_eq!(soniqo_batch_progress(11, 10), SONIQO_PROGRESS_MAX);
    }

    #[test]
    fn decoder_failure_retains_successful_chunks_and_empty_decode_resolves_audio() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = hound::WavWriter::create(
            file.path(),
            hound::WavSpec {
                channels: 1,
                sample_rate: TARGET_SAMPLE_RATE,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..48000 {
            writer.write_sample(0.1_f32).unwrap();
        }
        writer.finalize().unwrap();
        let pcm = hypr_audio_utils::PcmDescriptor::open(file.path()).unwrap();
        let plan = SoniqoChannelPlan {
            channel_index: 0,
            duration_seconds: 3.0,
            is_direct_mic: false,
            chunks: (0..3)
                .map(|index| pack_from_range(index * 16000, (index + 1) * 16000, vec![]))
                .collect(),
        };
        let mut index = 0;
        let mut completed = 0;
        let outcome = transcribe_soniqo_channel_chunks_with(
            hypr_transcribe_soniqo::SoniqoModel::OnnxParakeetBatch,
            plan,
            &pcm,
            || false,
            || completed += 1,
            |_| {
                index += 1;
                match index {
                    1 => Ok(hypr_transcribe_soniqo::FileTranscript::new(
                        "kept words".into(),
                        1.0,
                    )),
                    2 => Err("injected decoder failure".into()),
                    _ => Ok(hypr_transcribe_soniqo::FileTranscript::new(
                        String::new(),
                        1.0,
                    )),
                }
            },
        );
        assert_eq!(completed, 3);
        assert_eq!(outcome.transcript.text, "kept words");
        assert_eq!(outcome.processed_chunks, 2);
        assert_eq!(outcome.unresolved.len(), 1);
        assert_eq!(outcome.unresolved[0].start_sample, 16000);
        assert_eq!(outcome.unresolved[0].end_sample, 32000);
    }

    #[test]
    fn recovery_reads_only_the_saved_channel_and_sample_interval() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = hound::WavWriter::create(
            file.path(),
            hound::WavSpec {
                channels: 2,
                sample_rate: TARGET_SAMPLE_RATE,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for index in 0..48000 {
            writer.write_sample(-0.1_f32).unwrap();
            writer.write_sample(index as f32 / 16000.0).unwrap();
        }
        writer.finalize().unwrap();
        let pcm = hypr_audio_utils::PcmDescriptor::open(file.path()).unwrap();
        let range = FailedAudioRange {
            channel: 1,
            start_sample: 16000,
            end_sample: 32000,
            reason: "missing".into(),
        };
        let mut calls = 0;
        let channels = decode_recovery_range_with(
            hypr_transcribe_soniqo::SoniqoModel::OnnxParakeetBatch,
            &pcm,
            &range,
            |samples| {
                calls += 1;
                assert_eq!(samples.len(), 16000);
                assert_eq!(samples[0], 1.0);
                assert!(samples.iter().all(|sample| *sample >= 1.0 && *sample < 2.0));
                Ok(hypr_transcribe_soniqo::FileTranscript::new(
                    "missing words".into(),
                    1.0,
                ))
            },
        )
        .unwrap();
        assert_eq!(calls, 1);
        assert!(channels[0].text.is_empty());
        assert_eq!(channels[1].chunks[0].start_seconds, 1.0);
        assert_eq!(channels[1].chunks[0].duration_seconds, 1.0);
    }

    #[test]
    fn cancellation_retains_prior_text_and_does_not_decode_remaining_ranges() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = hound::WavWriter::create(
            file.path(),
            hound::WavSpec {
                channels: 1,
                sample_rate: TARGET_SAMPLE_RATE,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for _ in 0..32000 {
            writer.write_sample(0.1_f32).unwrap();
        }
        writer.finalize().unwrap();
        let pcm = hypr_audio_utils::PcmDescriptor::open(file.path()).unwrap();
        let cancelled = std::cell::Cell::new(false);
        let result = transcribe_soniqo_channel_chunks_with(
            hypr_transcribe_soniqo::SoniqoModel::OnnxParakeetBatch,
            SoniqoChannelPlan {
                channel_index: 0,
                duration_seconds: 2.0,
                is_direct_mic: false,
                chunks: vec![
                    pack_from_range(0, 16000, vec![]),
                    pack_from_range(16000, 32000, vec![]),
                ],
            },
            &pcm,
            || cancelled.get(),
            || {},
            |_| {
                cancelled.set(true);
                Ok(hypr_transcribe_soniqo::FileTranscript::new(
                    "kept".into(),
                    1.0,
                ))
            },
        );
        assert!(result.cancelled);
        assert_eq!(result.transcript.text, "kept");
        assert_eq!(result.unresolved.len(), 1);
        assert_eq!(result.unresolved[0].start_sample, 16000);
    }

    fn success(text: &str) -> SoniqoChannelOutcome {
        SoniqoChannelOutcome {
            transcript: hypr_transcribe_soniqo::FileTranscript::new(text.into(), 1.0),
            unresolved: Vec::new(),
            processed_chunks: 1,
            cancelled: false,
        }
    }

    #[test]
    fn partial_channel_retains_text_and_absolute_failure_range() {
        let mut channel = success("kept words");
        channel.unresolved.push(FailedAudioRange {
            channel: 0,
            start_sample: 16000,
            end_sample: 32000,
            reason: "decoder failed".into(),
        });
        let output = collect_soniqo_channel_transcripts([channel, success("")]);
        assert_eq!(output.status(), BatchCompletionStatus::Incomplete);
        assert_eq!(output.channels[0].text, "kept words");
        assert_eq!(output.unresolved[0].start_sample, 16000);
        assert_eq!(output.unresolved.len(), 1);
    }

    #[test]
    fn whole_channel_failure_preserves_channel_slots() {
        let output = collect_soniqo_channel_transcripts([
            SoniqoChannelOutcome::failed(0, 32000, "read failed".into()),
            success("system audio"),
        ]);
        assert_eq!(output.status(), BatchCompletionStatus::Incomplete);
        let response = hypr_transcribe_soniqo::batch_response_from_channels(
            hypr_transcribe_soniqo::SoniqoModel::ParakeetBatch,
            output.channels,
        );
        assert_eq!(
            response.results.channels[1].alternatives[0].words[0].channel,
            1
        );
    }

    #[test]
    fn total_failure_and_silence_have_distinct_status() {
        let output = collect_soniqo_channel_transcripts([SoniqoChannelOutcome::failed(
            0,
            16000,
            "failed".into(),
        )]);
        assert_eq!(output.status(), BatchCompletionStatus::Failed);
        assert_eq!(output.unresolved[0].end_sample, 16000);
        assert_eq!(
            collect_soniqo_channel_transcripts([success("")]).status(),
            BatchCompletionStatus::Complete
        );
    }
}
