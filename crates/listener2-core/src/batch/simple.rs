use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use owhisper_client::{
    AdapterKind, AquaVoiceAdapter, ArgmaxAdapter, AssemblyAIAdapter, BatchSttAdapter,
    CartesiaAdapter, DeepgramAdapter, ElevenLabsAdapter, FireworksAdapter, FmtrAdapter,
    GladiaAdapter, MistralAdapter, OpenAIAdapter, PyannoteAdapter, SonioxAdapter,
};
use owhisper_interface::batch_stream::BatchStreamEvent;
use tracing::Instrument;

use hypr_audio_chunking::AudioChunk;
use hypr_audio_utils::Source;
use hypr_transcribe_core::{
    TARGET_SAMPLE_RATE, channel_duration_sec, chunk_channel_audio, split_resampled_channels,
};

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
        Argmax => ArgmaxAdapter,
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

        stamp_batch_response(&mut response, &*diarization.segments().await);

        Ok(BatchRunOutput {
            session_id: params.session_id,
            mode: BatchRunMode::Direct,
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
) -> crate::Result<BatchRunOutput> {
    let span = session_span(&params.session_id);

    async {
        let model = listen_params
            .model
            .as_deref()
            .ok_or_else(|| crate::BatchFailure::DirectRequestFailed {
                provider: "soniqo".to_string(),
                message: "Missing Soniqo model.".to_string(),
            })?
            .parse::<hypr_transcribe_soniqo::SoniqoModel>()
            .map_err(|e| crate::BatchFailure::DirectRequestFailed {
                provider: "soniqo".to_string(),
                message: e.to_string(),
            })?
            .batch_model();

        let file_path = params.file_path.clone();
        let file_extension = Path::new(&file_path)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_string();
        let language = listen_params
            .languages
            .first()
            .map(hypr_language::Language::bcp47_code);
        let language_hint = soniqo_language_hint(language.as_deref());
        let language_label = language.as_deref().unwrap_or("auto").to_string();
        let language_hint_label = language_hint.as_deref().unwrap_or("auto").to_string();
        let started_at = Instant::now();

        tracing::info!(
            fmtr.stt.provider.name = "soniqo",
            fmtr.stt.model = %model,
            fmtr.stt.language = %language_label,
            fmtr.stt.language_hint = %language_hint_label,
            file.extension = %file_extension,
            "soniqo_batch_start"
        );

        let session_id = params.session_id.clone();
        let transcribed = tokio::task::spawn_blocking(move || {
            let progress = SoniqoProgressReporter {
                runtime,
                session_id,
            };
            transcribe_soniqo_file(model, &file_path, language_hint.as_deref(), Some(&progress))
        })
        .await
        .map_err(|e| {
            tracing::error!(
                fmtr.stt.provider.name = "soniqo",
                fmtr.stt.model = %model,
                error = %e,
                "soniqo_batch_task_join_failed"
            );
            crate::BatchFailure::DirectRequestFailed {
                provider: "soniqo".to_string(),
                message: format!("Soniqo transcription task failed: {e}"),
            }
        })?
        .map_err(|e| {
            let message = format_user_friendly_error(&e);
            tracing::error!(
                fmtr.stt.provider.name = "soniqo",
                fmtr.stt.model = %model,
                error = %e,
                fmtr.error.user_message = %message,
                "soniqo_batch_failed"
            );
            crate::BatchFailure::DirectRequestFailed {
                provider: "soniqo".to_string(),
                message,
            }
        })?;

        tracing::info!(
            fmtr.stt.provider.name = "soniqo",
            fmtr.stt.model = %model,
            elapsed_ms = started_at.elapsed().as_millis() as u64,
            transcript.channel_count = transcribed.len(),
            "soniqo_batch_completed"
        );

        let mut response = hypr_transcribe_soniqo::batch_response_from_channels(model, transcribed);
        stamp_batch_response(&mut response, &*diarization.segments().await);

        Ok(BatchRunOutput {
            session_id: params.session_id,
            mode: BatchRunMode::Direct,
            response,
        })
    }
    .instrument(span)
    .await
}

fn transcribe_soniqo_file(
    model: hypr_transcribe_soniqo::SoniqoModel,
    file_path: &str,
    language: Option<&str>,
    progress: Option<&SoniqoProgressReporter>,
) -> std::result::Result<Vec<hypr_transcribe_soniqo::FileTranscript>, String> {
    let source = hypr_audio_utils::source_from_path(file_path).map_err(|e| e.to_string())?;
    let channel_count = u16::from(source.channels()).max(1) as usize;
    let sample_rate = u32::from(source.sample_rate());
    let duration_ms = source
        .total_duration()
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64);

    tracing::info!(
        fmtr.stt.provider.name = "soniqo",
        fmtr.stt.model = %model,
        fmtr.stt.language = %language.unwrap_or("auto"),
        audio.channel_count = channel_count,
        audio.sample_rate_hz = sample_rate,
        audio.duration_ms = duration_ms.unwrap_or_default(),
        audio.duration_known = duration_ms.is_some(),
        "soniqo_audio_file_loaded"
    );

    if channel_count <= 1 && !uses_resilient_soniqo_chunking(model) {
        if let Some(progress) = progress {
            progress.emit(SONIQO_PROGRESS_PLANNED);
        }
        tracing::info!(
            fmtr.stt.provider.name = "soniqo",
            fmtr.stt.model = %model,
            "soniqo_single_channel_native_inference_start"
        );
        return hypr_transcribe_soniqo::transcribe_file(model, file_path, language)
            .map(|transcript| {
                if let Some(progress) = progress {
                    progress.emit(SONIQO_PROGRESS_MAX);
                }
                vec![transcript]
            })
            .map_err(|e| e.to_string());
    }

    let resample_started_at = Instant::now();
    let samples =
        hypr_audio_utils::resample_audio(source, TARGET_SAMPLE_RATE).map_err(|e| e.to_string())?;
    tracing::info!(
        fmtr.stt.provider.name = "soniqo",
        fmtr.stt.model = %model,
        elapsed_ms = resample_started_at.elapsed().as_millis() as u64,
        audio.source_sample_rate_hz = sample_rate,
        audio.target_sample_rate_hz = TARGET_SAMPLE_RATE,
        audio.resampled_sample_count = samples.len(),
        "soniqo_audio_resampled"
    );

    let channel_samples =
        collapse_identical_channels(split_resampled_channels(&samples, channel_count));
    tracing::info!(
        fmtr.stt.provider.name = "soniqo",
        fmtr.stt.model = %model,
        audio.source_channel_count = channel_count,
        audio.transcribed_channel_count = channel_samples.len(),
        "soniqo_channels_prepared"
    );

    let transcribed_channel_count = channel_samples.len();
    let plans = channel_samples
        .into_iter()
        .enumerate()
        .map(|(channel_index, samples)| {
            soniqo_channel_plan(
                model,
                channel_index,
                &samples,
                transcribed_channel_count == 2 && channel_index == 0,
            )
        })
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let total_chunks = plans.iter().map(|plan| plan.chunks.len()).sum::<usize>();
    let mut completed_chunks = 0usize;

    if let Some(progress) = progress {
        progress.emit(soniqo_batch_progress(0, total_chunks));
    }

    collect_soniqo_channel_transcripts(plans.into_iter().map(|plan| {
        transcribe_soniqo_channel_chunks(model, plan, language, || {
            completed_chunks += 1;
            if let Some(progress) = progress {
                progress.emit(soniqo_batch_progress(completed_chunks, total_chunks));
            }
        })
    }))
}

struct SoniqoProgressReporter {
    runtime: Arc<dyn BatchRuntime>,
    session_id: String,
}

impl SoniqoProgressReporter {
    fn emit(&self, percentage: f64) {
        self.runtime.emit(BatchEvent::BatchResponseStreamed {
            session_id: self.session_id.clone(),
            event: BatchStreamEvent::Progress {
                percentage,
                partial_text: None,
            },
        });
    }
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

fn soniqo_language_hint(language: Option<&str>) -> Option<String> {
    let language = language?.trim();
    if language.is_empty() {
        return None;
    }

    language
        .split(['-', '_'])
        .next()
        .filter(|value| !value.is_empty())
        .map(|value| value.to_lowercase())
}

fn uses_resilient_soniqo_chunking(model: hypr_transcribe_soniqo::SoniqoModel) -> bool {
    matches!(
        model,
        hypr_transcribe_soniqo::SoniqoModel::ParakeetBatch
            | hypr_transcribe_soniqo::SoniqoModel::OnnxParakeetBatch
    )
}

fn soniqo_batch_progress(completed_chunks: usize, total_chunks: usize) -> f64 {
    if total_chunks == 0 {
        return SONIQO_PROGRESS_PLANNED;
    }

    let ratio = completed_chunks as f64 / total_chunks as f64;
    (SONIQO_PROGRESS_PLANNED + ratio * SONIQO_PROGRESS_RANGE).min(SONIQO_PROGRESS_MAX)
}

fn collect_soniqo_channel_transcripts<I>(
    transcripts: I,
) -> std::result::Result<Vec<hypr_transcribe_soniqo::FileTranscript>, String>
where
    I: IntoIterator<Item = std::result::Result<hypr_transcribe_soniqo::FileTranscript, String>>,
{
    let mut output = Vec::new();
    let mut successful_channels = 0usize;
    let mut failed_channels = 0usize;

    for transcript in transcripts {
        match transcript {
            Ok(transcript) => {
                successful_channels += 1;
                output.push(transcript);
            }
            Err(error) => {
                failed_channels += 1;
                tracing::warn!(
                    fmtr.stt.provider.name = "soniqo",
                    error = %error,
                    "soniqo_channel_transcription_failed"
                );
                output.push(hypr_transcribe_soniqo::FileTranscript::new(
                    String::new(),
                    0.05,
                ));
            }
        }
    }

    if successful_channels == 0 && failed_channels > 0 {
        return Err(format!(
            "Soniqo failed to transcribe all {failed_channels} audio channel(s)."
        ));
    }

    Ok(output)
}

fn soniqo_channel_plan(
    model: hypr_transcribe_soniqo::SoniqoModel,
    channel_index: usize,
    samples: &[f32],
    is_direct_mic: bool,
) -> std::result::Result<SoniqoChannelPlan, String> {
    let duration_seconds = channel_duration_sec(samples);
    let chunks = soniqo_channel_chunks(model, samples)?;
    tracing::info!(
        fmtr.stt.provider.name = "soniqo",
        fmtr.stt.model = %model,
        channel.index = channel_index,
        channel.duration_seconds = duration_seconds,
        channel.sample_count = samples.len(),
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
    language: Option<&str>,
    mut on_chunk_completed: impl FnMut(),
) -> std::result::Result<hypr_transcribe_soniqo::FileTranscript, String> {
    let mut texts = Vec::new();
    let mut transcript_chunks = Vec::new();
    let mut successful_chunks = 0usize;
    let mut failed_chunks = 0usize;
    let channel_index = plan.channel_index;
    let is_direct_mic = plan.is_direct_mic;

    for (chunk_index, chunk) in plan.chunks.into_iter().enumerate() {
        let chunk_duration_ms =
            (chunk.sample_end - chunk.sample_start) * 1000 / TARGET_SAMPLE_RATE as usize;
        let chunk_rms = chunk_speech_rms(&chunk);
        if is_direct_mic && chunk_rms < SONIQO_DIRECT_MIC_MIN_RMS {
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

        let text = match transcribe_soniqo_samples(model, &chunk.samples, language) {
            Ok(transcript) => {
                successful_chunks += 1;
                transcript.text
            }
            Err(e) => {
                failed_chunks += 1;
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
            transcript.text_chars = text.chars().count(),
            "soniqo_chunk_native_inference_completed"
        );

        let text = text.trim();
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

    if successful_chunks == 0 && failed_chunks > 0 {
        return Err(format!(
            "Soniqo failed to transcribe all {failed_chunks} chunk(s) for channel {channel_index}."
        ));
    }

    if failed_chunks > 0 {
        tracing::warn!(
            fmtr.stt.provider.name = "soniqo",
            fmtr.stt.model = %model,
            channel.index = channel_index,
            chunk.success_count = successful_chunks,
            chunk.failed_count = failed_chunks,
            "soniqo_channel_completed_with_chunk_failures"
        );
    }

    if transcript_chunks.is_empty() {
        return Ok(hypr_transcribe_soniqo::FileTranscript::new(
            texts.join(" "),
            plan.duration_seconds,
        ));
    }

    Ok(hypr_transcribe_soniqo::FileTranscript::from_chunks(
        transcript_chunks,
        plan.duration_seconds,
    ))
}

fn transcribe_soniqo_samples(
    model: hypr_transcribe_soniqo::SoniqoModel,
    samples: &[f32],
    language: Option<&str>,
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

    hypr_transcribe_soniqo::transcribe_file(model, file.path(), language).map_err(|e| e.to_string())
}

fn soniqo_channel_chunks(
    model: hypr_transcribe_soniqo::SoniqoModel,
    samples: &[f32],
) -> std::result::Result<Vec<ChannelChunk>, String> {
    if uses_resilient_soniqo_chunking(model) {
        return Ok(
            match chunk_channel_audio::<hypr_audio_chunking::Error>(samples) {
                Ok(chunks) => {
                    pack_speech_chunks(samples, chunks, SONIQO_PARAKEET_MAX_CHUNK_SAMPLES)
                }
                Err(error) => {
                    tracing::warn!(
                        fmtr.stt.provider.name = "soniqo",
                        fmtr.stt.model = %model,
                        error = %error,
                        "soniqo_speech_chunking_failed_using_fixed_windows"
                    );
                    split_audio_samples(samples, SONIQO_PARAKEET_MAX_CHUNK_SAMPLES)
                        .into_iter()
                        .map(ChannelChunk::from)
                        .collect()
                }
            },
        );
    }

    Ok(chunk_channel_audio::<hypr_audio_chunking::Error>(samples)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(ChannelChunk::from)
        .collect())
}

fn pack_speech_chunks(
    samples: &[f32],
    chunks: Vec<AudioChunk>,
    max_samples: usize,
) -> Vec<ChannelChunk> {
    let mut packs: Vec<ChannelChunk> = Vec::new();
    let mut current: Option<(usize, usize, Vec<(usize, usize)>)> = None;

    for chunk in chunks {
        let span = (
            chunk.sample_start.min(samples.len()),
            chunk.sample_end.min(samples.len()),
        );
        match current.as_mut() {
            Some((start, end, spans)) if span.1.saturating_sub(*start) <= max_samples => {
                *end = span.1.max(*end);
                spans.push(span);
            }
            _ => {
                if let Some((start, end, spans)) = current.take() {
                    packs.push(pack_from_range(samples, start, end, spans));
                }
                current = Some((span.0, span.1, vec![span]));
            }
        }
    }

    if let Some((start, end, spans)) = current.take() {
        packs.push(pack_from_range(samples, start, end, spans));
    }

    packs
}

fn pack_from_range(
    samples: &[f32],
    sample_start: usize,
    sample_end: usize,
    speech_spans: Vec<(usize, usize)>,
) -> ChannelChunk {
    ChannelChunk {
        samples: samples[sample_start..sample_end].to_vec(),
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

fn split_audio_samples(samples: &[f32], max_samples: usize) -> Vec<AudioChunk> {
    samples
        .chunks(max_samples)
        .enumerate()
        .map(|(index, window)| {
            let sample_start = index * max_samples;
            let sample_end = sample_start + window.len();
            AudioChunk {
                samples: window.to_vec(),
                sample_start,
                sample_end,
            }
        })
        .collect()
}

fn collapse_identical_channels(channels: Vec<Vec<f32>>) -> Vec<Vec<f32>> {
    if channels.len() != 2 || !channels_are_effectively_identical(&channels[0], &channels[1]) {
        return channels;
    }

    channels.into_iter().take(1).collect()
}

fn channels_are_effectively_identical(left: &[f32], right: &[f32]) -> bool {
    if left.len().abs_diff(right.len()) > 1 {
        return false;
    }

    let compared = left.len().min(right.len());
    if compared == 0 {
        return true;
    }

    let mean_abs_diff = left
        .iter()
        .zip(right.iter())
        .map(|(a, b)| (a - b).abs())
        .sum::<f32>()
        / compared as f32;

    mean_abs_diff < 0.0005
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_effectively_identical_stereo_channels() {
        let channels =
            collapse_identical_channels(vec![vec![0.1, 0.2, 0.3], vec![0.1001, 0.2001, 0.3001]]);

        assert_eq!(channels, vec![vec![0.1, 0.2, 0.3]]);
    }

    #[test]
    fn keeps_distinct_stereo_channels() {
        let channels = collapse_identical_channels(vec![vec![0.1, 0.2], vec![0.9, 0.8]]);

        assert_eq!(channels, vec![vec![0.1, 0.2], vec![0.9, 0.8]]);
    }

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
    fn soniqo_language_hint_uses_base_language_code() {
        assert_eq!(soniqo_language_hint(Some("de-DE")).as_deref(), Some("de"));
        assert_eq!(soniqo_language_hint(Some("en_US")).as_deref(), Some("en"));
        assert_eq!(soniqo_language_hint(Some(" fr ")).as_deref(), Some("fr"));
        assert_eq!(soniqo_language_hint(Some("")).as_deref(), None);
        assert_eq!(soniqo_language_hint(None).as_deref(), None);
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
    fn collect_soniqo_channel_transcripts_keeps_channel_slots() {
        let transcripts = collect_soniqo_channel_transcripts([
            Ok(hypr_transcribe_soniqo::FileTranscript::new(
                "hello".to_string(),
                1.0,
            )),
            Err("native chunk failed".to_string()),
        ])
        .unwrap();

        assert_eq!(transcripts.len(), 2);
        assert_eq!(transcripts[0].text, "hello");
        assert_eq!(transcripts[1].text, "");
    }

    #[test]
    fn collect_soniqo_channel_transcripts_preserves_later_channel_index() {
        let transcripts = collect_soniqo_channel_transcripts([
            Err("first failed".to_string()),
            Ok(hypr_transcribe_soniqo::FileTranscript::new(
                "system audio".to_string(),
                1.0,
            )),
        ])
        .unwrap();

        let response = hypr_transcribe_soniqo::batch_response_from_channels(
            hypr_transcribe_soniqo::SoniqoModel::ParakeetBatch,
            transcripts,
        );
        let alternative = &response.results.channels[1].alternatives[0];

        assert_eq!(response.results.channels.len(), 2);
        assert_eq!(response.results.channels[0].alternatives[0].transcript, "");
        assert_eq!(alternative.transcript, "system audio");
        assert_eq!(alternative.words[0].channel, 1);
    }

    #[test]
    fn collect_soniqo_channel_transcripts_errors_when_all_channels_fail() {
        let error = collect_soniqo_channel_transcripts([
            Err("first failed".to_string()),
            Err("second failed".to_string()),
        ])
        .unwrap_err();

        assert_eq!(error, "Soniqo failed to transcribe all 2 audio channel(s).");
    }
}
