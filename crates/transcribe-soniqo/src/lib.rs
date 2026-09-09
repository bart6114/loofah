use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Instant;

use owhisper_interface::{batch, stream};

pub mod diarize;

pub const LOCAL_BASE_URL: &str = "soniqo://local";
const SYNTHETIC_BATCH_WORD_SECONDS: f64 = 0.4;
const MIN_SYNTHETIC_DURATION_SECONDS: f64 = 0.05;

pub fn is_local_base_url(base_url: &str) -> bool {
    base_url.trim_end_matches('/') == LOCAL_BASE_URL
}

pub fn is_loopback_http_base_url(base_url: &str) -> bool {
    let Some(rest) = base_url
        .trim()
        .strip_prefix("http://")
        .or_else(|| base_url.trim().strip_prefix("https://"))
    else {
        return false;
    };

    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();

    let host = authority
        .strip_prefix('[')
        .and_then(|value| value.split(']').next())
        .unwrap_or_else(|| authority.split(':').next().unwrap_or_default());

    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|addr| addr.is_loopback())
}

pub fn local_model_from_request(base_url: &str, model: &str) -> Option<SoniqoModel> {
    let model = model.parse().ok()?;

    if is_local_base_url(base_url) || is_loopback_http_base_url(base_url) {
        Some(model)
    } else {
        None
    }
}

#[derive(
    Debug, Clone, Copy, serde::Serialize, serde::Deserialize, specta::Type, Eq, Hash, PartialEq,
)]
pub enum SoniqoModel {
    #[serde(rename = "onnx-parakeet-streaming")]
    OnnxParakeetStreaming,
    #[serde(rename = "onnx-parakeet-batch")]
    OnnxParakeetBatch,
    #[serde(rename = "soniqo-parakeet-streaming")]
    ParakeetStreaming,
    #[serde(rename = "soniqo-parakeet-batch")]
    ParakeetBatch,
    #[serde(rename = "soniqo-omnilingual")]
    Omnilingual,
    #[serde(rename = "soniqo-qwen3-small")]
    Qwen3Small,
    #[serde(rename = "soniqo-qwen3-large")]
    Qwen3Large,
}

impl SoniqoModel {
    const ALL: &'static [Self] = &[
        Self::ParakeetStreaming,
        Self::ParakeetBatch,
        Self::Omnilingual,
    ];

    const KNOWN: &'static [Self] = &[
        Self::OnnxParakeetStreaming,
        Self::OnnxParakeetBatch,
        Self::ParakeetStreaming,
        Self::ParakeetBatch,
        Self::Omnilingual,
        Self::Qwen3Small,
        Self::Qwen3Large,
    ];

    const SELECTABLE: &'static [Self] = &[Self::ParakeetStreaming, Self::ParakeetBatch];

    pub const fn all() -> &'static [Self] {
        if cfg!(target_os = "windows") {
            Self::selectable()
        } else {
            Self::ALL
        }
    }

    pub const fn selectable() -> &'static [Self] {
        if cfg!(target_os = "windows") {
            &[Self::OnnxParakeetStreaming, Self::OnnxParakeetBatch]
        } else {
            Self::SELECTABLE
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ParakeetStreaming => "soniqo-parakeet-streaming",
            Self::OnnxParakeetStreaming => "onnx-parakeet-streaming",
            Self::OnnxParakeetBatch => "onnx-parakeet-batch",
            Self::ParakeetBatch => "soniqo-parakeet-batch",
            Self::Omnilingual => "soniqo-omnilingual",
            Self::Qwen3Small => "soniqo-qwen3-small",
            Self::Qwen3Large => "soniqo-qwen3-large",
        }
    }

    pub const fn repo(self) -> &'static str {
        match self {
            Self::ParakeetStreaming => "aufklarer/Parakeet-EOU-120M-CoreML-INT8",
            Self::OnnxParakeetStreaming => "altunenes/parakeet-rs",
            Self::OnnxParakeetBatch => "istupakov/parakeet-tdt-0.6b-v3-onnx",
            Self::ParakeetBatch => "aufklarer/Parakeet-TDT-v3-CoreML-INT8",
            Self::Omnilingual => "aufklarer/Omnilingual-ASR-CTC-300M-CoreML-INT8-10s",
            Self::Qwen3Small => "aufklarer/Qwen3-ASR-0.6B-MLX-4bit",
            Self::Qwen3Large => "aufklarer/Qwen3-ASR-1.7B-MLX-8bit",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::ParakeetStreaming => "Soniqo Parakeet Streaming",
            Self::OnnxParakeetStreaming => "Parakeet Streaming",
            Self::OnnxParakeetBatch => "Parakeet Batch",
            Self::ParakeetBatch => "Soniqo Parakeet Batch",
            Self::Omnilingual => "Soniqo Omnilingual",
            Self::Qwen3Small => "Soniqo Qwen3 0.6B",
            Self::Qwen3Large => "Soniqo Qwen3 1.7B",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::ParakeetStreaming | Self::OnnxParakeetStreaming => {
                "Realtime English transcription."
            }
            Self::ParakeetBatch | Self::OnnxParakeetBatch => {
                "Batch transcription for 25 European languages."
            }
            Self::Omnilingual => "Multilingual batch transcription.",
            Self::Qwen3Small => "Multilingual batch transcription.",
            Self::Qwen3Large => "Multilingual batch transcription.",
        }
    }

    pub const fn size_bytes(self) -> u64 {
        match self {
            Self::ParakeetStreaming => 120 * 1024 * 1024,
            Self::OnnxParakeetStreaming => 480708981,
            Self::OnnxParakeetBatch => 670479942,
            Self::ParakeetBatch => 600 * 1024 * 1024,
            Self::Omnilingual => 300 * 1024 * 1024,
            Self::Qwen3Small => 600 * 1024 * 1024,
            Self::Qwen3Large => 1_700 * 1024 * 1024,
        }
    }

    pub const fn supports_live(self) -> bool {
        matches!(self, Self::ParakeetStreaming | Self::OnnxParakeetStreaming)
    }

    pub const fn is_available_on_current_platform(self) -> bool {
        (cfg!(all(target_os = "macos", target_arch = "aarch64")) && !self.requires_macos_15())
            || (cfg!(all(
                target_os = "windows",
                any(target_arch = "x86_64", target_arch = "aarch64")
            )) && matches!(
                self,
                Self::ParakeetStreaming
                    | Self::ParakeetBatch
                    | Self::OnnxParakeetStreaming
                    | Self::OnnxParakeetBatch
            ))
    }

    pub const fn resolved(self) -> Self {
        match self {
            Self::ParakeetStreaming | Self::OnnxParakeetStreaming => {
                if cfg!(target_os = "windows") {
                    Self::OnnxParakeetStreaming
                } else {
                    Self::ParakeetStreaming
                }
            }
            Self::ParakeetBatch | Self::OnnxParakeetBatch => {
                if cfg!(target_os = "windows") {
                    Self::OnnxParakeetBatch
                } else {
                    Self::ParakeetBatch
                }
            }
            model => model,
        }
    }

    const fn requires_macos_15(self) -> bool {
        matches!(self, Self::Qwen3Small | Self::Qwen3Large)
    }

    pub const fn supports_live_on_current_platform(self) -> bool {
        self.supports_live() && self.is_available_on_current_platform()
    }

    pub const fn batch_model(self) -> Self {
        match self {
            Self::ParakeetStreaming => Self::ParakeetBatch,
            Self::OnnxParakeetStreaming => Self::OnnxParakeetBatch,
            model => model,
        }
    }

    pub fn supports_language(self, language: &hypr_language::Language) -> bool {
        match self {
            // Parakeet-EOU-120M uses an English-only BPE vocabulary; it decodes
            // other languages into English-shaped gibberish or nothing at all.
            Self::ParakeetStreaming | Self::OnnxParakeetStreaming => {
                hypr_language::is_parakeet_eou_language(language)
            }
            Self::ParakeetBatch | Self::OnnxParakeetBatch => {
                hypr_language::is_parakeet_tdt_v3_language(language)
            }
            Self::Omnilingual | Self::Qwen3Small | Self::Qwen3Large => true,
        }
    }

    pub fn supports_languages(self, languages: &[hypr_language::Language]) -> bool {
        languages
            .iter()
            .all(|language| self.supports_language(language))
    }
}

impl std::fmt::Display for SoniqoModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SoniqoModel {
    type Err = Error;

    fn from_str(value: &str) -> Result<Self> {
        Self::KNOWN
            .iter()
            .copied()
            .find(|model| value == model.as_str() || value == model.repo())
            .ok_or_else(|| Error::UnsupportedModel(value.to_string()))
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelDownloadState {
    pub status: String,
    pub current_file: Option<String>,
    pub progress_percent: Option<u8>,
    pub local_path: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTranscript {
    pub text: String,
    pub duration_seconds: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub chunks: Vec<FileTranscriptChunk>,
}

impl FileTranscript {
    pub fn new(text: String, duration_seconds: f64) -> Self {
        Self {
            text,
            duration_seconds,
            chunks: Vec::new(),
        }
    }

    pub fn from_chunks(chunks: Vec<FileTranscriptChunk>, duration_seconds: f64) -> Self {
        let text = chunks
            .iter()
            .map(|chunk| chunk.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");

        Self {
            text,
            duration_seconds,
            chunks,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTranscriptChunk {
    pub text: String,
    pub start_seconds: f64,
    pub duration_seconds: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub speech_spans: Vec<SpeechSpan>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechSpan {
    pub start_seconds: f64,
    pub end_seconds: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptSource {
    Microphone,
    System,
}

impl TranscriptSource {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Microphone => "microphone",
            Self::System => "system",
        }
    }

    pub const fn channel_index(self) -> i32 {
        match self {
            Self::Microphone => 0,
            Self::System => 1,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LivePartial {
    pub source: String,
    pub text: String,
    pub is_final: bool,
}

impl LivePartial {
    pub fn source(&self) -> TranscriptSource {
        match self.source.as_str() {
            "system" => TranscriptSource::System,
            _ => TranscriptSource::Microphone,
        }
    }

    pub fn into_stream_response(
        self,
        model: SoniqoModel,
        start: f64,
        duration: f64,
    ) -> stream::StreamResponse {
        let source = self.source();
        stream_response_from_text(
            model,
            self.text,
            start,
            duration,
            self.is_final,
            &[source.channel_index(), 2],
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported Soniqo model: {0}")]
    UnsupportedModel(String),
    #[error("On-device speech requires macOS Apple Silicon or Windows x64/ARM64")]
    UnsupportedPlatform,
    #[error("{} requires macOS 15 or newer.", .0.display_name())]
    RequiresMacOs15(SoniqoModel),
    #[error("Soniqo bridge failed: {0}")]
    Bridge(String),
    #[error("failed to parse Soniqo bridge response: {0}")]
    ResponseParse(#[from] serde_json::Error),
    #[error("failed to delete Soniqo model: {0}")]
    Delete(std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

fn ensure_supported_platform(model: SoniqoModel) -> Result<()> {
    if !model.is_available_on_current_platform()
        && !cfg!(all(target_os = "macos", target_arch = "aarch64"))
    {
        return Err(Error::UnsupportedPlatform);
    }

    if model.requires_macos_15() {
        return Err(Error::RequiresMacOs15(model));
    }

    Ok(())
}

pub fn model_cache_dir(model: SoniqoModel) -> Result<PathBuf> {
    ensure_supported_platform(model)?;
    platform::model_cache_dir(model.resolved())
}

pub fn model_download_state(model: SoniqoModel) -> Result<ModelDownloadState> {
    ensure_supported_platform(model)?;
    platform::model_download_state(model.resolved())
}

pub fn start_model_download(model: SoniqoModel) -> Result<()> {
    ensure_supported_platform(model)?;
    platform::start_model_download(model.resolved())
}

pub fn reset_model(model: SoniqoModel) -> Result<()> {
    ensure_supported_platform(model)?;
    platform::reset_model(model.resolved())
}

pub fn is_model_downloaded(model: SoniqoModel) -> Result<bool> {
    Ok(model_download_state(model)?.status == "ready")
}

pub fn is_model_downloading(model: SoniqoModel) -> Result<bool> {
    Ok(model_download_state(model)?.status == "downloading")
}

#[cfg(target_os = "windows")]
pub fn delete_model(model: SoniqoModel) -> Result<()> {
    ensure_supported_platform(model)?;
    platform::delete_model(model.resolved())
}

#[cfg(not(target_os = "windows"))]
pub fn delete_model(model: SoniqoModel) -> Result<()> {
    reset_model(model)?;

    let cache_dir = model_cache_dir(model)?;
    if cache_dir.exists() {
        std::fs::remove_dir_all(&cache_dir).map_err(Error::Delete)?;
    }

    Ok(())
}

pub fn transcribe_file(
    model: SoniqoModel,
    path: impl AsRef<Path>,
    language: Option<&str>,
) -> Result<FileTranscript> {
    ensure_supported_platform(model)?;

    let path = path.as_ref();
    let language = language.unwrap_or_default();
    let language_label = if language.is_empty() {
        "auto"
    } else {
        language
    };
    let file_extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default();
    let started_at = Instant::now();

    tracing::info!(
        fmtr.stt.provider.name = "soniqo",
        fmtr.stt.model = %model,
        fmtr.stt.language = %language_label,
        file.extension = %file_extension,
        "soniqo_native_file_transcription_start"
    );

    let result = platform::transcribe_file(model.resolved(), path, language);
    let elapsed_ms = started_at.elapsed().as_millis() as u64;

    match &result {
        Ok(transcript) => {
            tracing::info!(
                fmtr.stt.provider.name = "soniqo",
                fmtr.stt.model = %model,
                elapsed_ms,
                transcript.duration_seconds = transcript.duration_seconds,
                transcript.text_chars = transcript.text.chars().count(),
                "soniqo_native_file_transcription_completed"
            );
        }
        Err(error) => {
            tracing::error!(
                fmtr.stt.provider.name = "soniqo",
                fmtr.stt.model = %model,
                elapsed_ms,
                error = %error,
                "soniqo_native_file_transcription_failed"
            );
        }
    }

    result
}

pub struct LiveTranscriptionSession {
    model: SoniqoModel,
    stopped: bool,
}

impl LiveTranscriptionSession {
    pub fn start(model: SoniqoModel) -> Result<Self> {
        ensure_supported_platform(model)?;

        if !model.supports_live() {
            return Err(Error::Bridge(format!(
                "{} does not support realtime transcription",
                model.display_name()
            )));
        }

        platform::live_start(model.resolved())?;
        Ok(Self {
            model,
            stopped: false,
        })
    }

    pub fn append(
        &mut self,
        source: TranscriptSource,
        samples: &[f32],
    ) -> Result<Vec<LivePartial>> {
        platform::live_append(source, samples)
    }

    pub fn finalize(&mut self, source: TranscriptSource) -> Result<Vec<LivePartial>> {
        platform::live_finalize(source)
    }

    pub fn model(&self) -> SoniqoModel {
        self.model
    }

    pub fn stop(mut self) -> Result<()> {
        self.stopped = true;
        platform::live_stop()
    }
}

impl Drop for LiveTranscriptionSession {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = platform::live_stop();
        }
    }
}

pub fn stream_response_from_text(
    model: SoniqoModel,
    text: String,
    start: f64,
    duration: f64,
    is_final: bool,
    channel_index: &[i32],
) -> stream::StreamResponse {
    let text = normalize_transcript_text(&text);
    let duration = duration.max(MIN_SYNTHETIC_DURATION_SECONDS);
    let words = stream_words_from_text(&text, start, duration);

    stream::StreamResponse::TranscriptResponse {
        start,
        duration,
        is_final,
        speech_final: is_final,
        from_finalize: false,
        channel: stream::Channel {
            alternatives: vec![stream::Alternatives {
                transcript: text,
                words,
                confidence: 1.0,
                languages: vec![],
            }],
        },
        metadata: metadata(model),
        channel_index: channel_index.to_vec(),
    }
}

pub fn batch_response_from_text(
    model: SoniqoModel,
    text: String,
    duration_seconds: f64,
) -> batch::Response {
    batch_response_from_channels(model, vec![FileTranscript::new(text, duration_seconds)])
}

pub fn batch_response_from_channels(
    model: SoniqoModel,
    channels: Vec<FileTranscript>,
) -> batch::Response {
    let channels = if channels.is_empty() {
        vec![FileTranscript::new(String::new(), 0.05)]
    } else {
        channels
    };
    let duration_seconds = channels
        .iter()
        .map(|channel| channel.duration_seconds)
        .fold(0.05, f64::max);
    let has_speech_spans = channels
        .iter()
        .flat_map(|channel| channel.chunks.iter())
        .any(|chunk| !chunk.speech_spans.is_empty());
    let metadata = metadata_json(
        model,
        duration_seconds,
        channels.len() as u32,
        has_speech_spans,
    );

    batch::Response {
        metadata,
        results: batch::Results {
            channels: channels
                .into_iter()
                .enumerate()
                .map(|(channel_index, channel)| {
                    let transcript = normalize_transcript_text(&channel.text);
                    let words = if channel.chunks.is_empty() {
                        let synthetic_duration = synthetic_text_duration(&transcript);
                        batch_words_from_text(
                            &transcript,
                            0.0,
                            synthetic_duration,
                            channel_index as i32,
                        )
                    } else {
                        batch_words_from_chunks(&channel.chunks, channel_index as i32)
                    };
                    batch::Channel {
                        alternatives: vec![batch::Alternatives {
                            words,
                            transcript,
                            confidence: 1.0,
                        }],
                    }
                })
                .collect(),
        },
    }
}

fn metadata(model: SoniqoModel) -> stream::Metadata {
    stream::Metadata {
        model_info: stream::ModelInfo {
            name: model.as_str().to_string(),
            version: "0.0.9".to_string(),
            arch: "soniqo".to_string(),
        },
        extra: Some(stream::Extra::default().into()),
        ..Default::default()
    }
}

fn metadata_json(
    model: SoniqoModel,
    duration_seconds: f64,
    channels: u32,
    has_speech_spans: bool,
) -> serde_json::Value {
    let mut value = serde_json::to_value(metadata(model)).unwrap_or_else(|_| serde_json::json!({}));
    if let Some(object) = value.as_object_mut() {
        object.insert("duration".to_string(), serde_json::json!(duration_seconds));
        object.insert("channels".to_string(), serde_json::json!(channels));
        object.insert(
            "timing_source".to_string(),
            serde_json::json!(if has_speech_spans {
                "synthetic_speech"
            } else {
                "synthetic_text"
            }),
        );
    }
    value
}

fn stream_words_from_text(text: &str, start: f64, duration: f64) -> Vec<stream::Word> {
    let word_strs = split_words(text);
    let count = word_strs.len();

    if count == 0 {
        return Vec::new();
    }

    word_strs
        .into_iter()
        .enumerate()
        .map(|(index, word)| {
            let word_start = start + (index as f64 / count as f64) * duration;
            let word_end = if index + 1 == count {
                (start + duration - 0.05).max(word_start + 0.05)
            } else {
                start + ((index + 1) as f64 / count as f64) * duration
            };

            stream::Word {
                word: word.to_string(),
                start: word_start,
                end: word_end,
                confidence: 1.0,
                speaker: None,
                punctuated_word: Some(word.to_string()),
                language: None,
            }
        })
        .collect()
}

fn batch_words_from_chunks(chunks: &[FileTranscriptChunk], channel: i32) -> Vec<batch::Word> {
    chunks
        .iter()
        .flat_map(|chunk| {
            let text = normalize_transcript_text(&chunk.text);
            if !chunk.speech_spans.is_empty() {
                return batch_words_from_speech_spans(
                    &text,
                    chunk.start_seconds,
                    chunk.duration_seconds,
                    &chunk.speech_spans,
                    channel,
                );
            }

            let duration = synthetic_text_duration(&text)
                .min(chunk.duration_seconds.max(MIN_SYNTHETIC_DURATION_SECONDS));
            let start = chunk.start_seconds.max(0.0);
            batch_words_from_text(&text, start, duration, channel)
        })
        .collect()
}

fn batch_words_from_speech_spans(
    text: &str,
    start: f64,
    duration: f64,
    speech_spans: &[SpeechSpan],
    channel: i32,
) -> Vec<batch::Word> {
    let words = split_words(text);
    if words.is_empty() {
        return Vec::new();
    }

    let chunk_start = start.max(0.0);
    let chunk_end = chunk_start + duration.max(MIN_SYNTHETIC_DURATION_SECONDS);
    let mut spans = speech_spans
        .iter()
        .filter_map(|span| {
            let start = span.start_seconds.max(chunk_start);
            let end = span.end_seconds.min(chunk_end);
            (end > start).then_some((start, end))
        })
        .collect::<Vec<_>>();
    spans.sort_by(|left, right| {
        left.0
            .total_cmp(&right.0)
            .then_with(|| left.1.total_cmp(&right.1))
    });
    let mut non_overlapping = Vec::with_capacity(spans.len());
    let mut previous_end = chunk_start;
    for (start, end) in spans {
        let start = start.max(previous_end);
        if end > start {
            non_overlapping.push((start, end));
            previous_end = end;
        }
    }
    let spans = non_overlapping;

    let active_duration = spans.iter().map(|(start, end)| end - start).sum::<f64>();
    if active_duration <= 0.0 {
        let duration =
            synthetic_text_duration(text).min(duration.max(MIN_SYNTHETIC_DURATION_SECONDS));
        return batch_words_from_text(text, chunk_start, duration, channel);
    }

    let position = |offset: f64| {
        let mut remaining = offset.clamp(0.0, active_duration);
        for (index, (start, end)) in spans.iter().enumerate() {
            let span_duration = end - start;
            if remaining <= span_duration || index + 1 == spans.len() {
                return (start + remaining.min(span_duration), index);
            }
            remaining -= span_duration;
        }
        (chunk_end, spans.len() - 1)
    };
    let count = words.len();

    words
        .into_iter()
        .enumerate()
        .map(|(index, word)| {
            let start_offset = index as f64 / count as f64 * active_duration;
            let end_offset = (index + 1) as f64 / count as f64 * active_duration;
            let midpoint_offset = (start_offset + end_offset) / 2.0;
            let (mapped_start, _) = position(start_offset);
            let (mapped_end, _) = position(end_offset);
            let (midpoint, span_index) = position(midpoint_offset);
            let (span_start, span_end) = spans[span_index];
            let word_start = mapped_start.max(span_start).min(span_end);
            let word_end = mapped_end
                .min(span_end)
                .max((word_start + MIN_SYNTHETIC_DURATION_SECONDS).min(span_end));
            let (word_start, word_end) = if word_end > word_start {
                (word_start, word_end)
            } else {
                let half = MIN_SYNTHETIC_DURATION_SECONDS / 2.0;
                (
                    (midpoint - half).max(span_start),
                    (midpoint + half).min(span_end),
                )
            };

            batch::Word {
                word: word.to_string(),
                start: word_start,
                end: word_end.max(word_start + f64::EPSILON),
                confidence: 1.0,
                channel,
                speaker: None,
                punctuated_word: Some(word.to_string()),
            }
        })
        .collect()
}

fn batch_words_from_text(text: &str, start: f64, duration: f64, channel: i32) -> Vec<batch::Word> {
    let word_strs = split_words(text);
    let count = word_strs.len();

    if count == 0 {
        return Vec::new();
    }

    word_strs
        .into_iter()
        .enumerate()
        .map(|(index, word)| batch::Word {
            word: word.to_string(),
            start: start + (index as f64 / count as f64) * duration,
            end: start + ((index + 1) as f64 / count as f64) * duration,
            confidence: 1.0,
            channel,
            speaker: None,
            punctuated_word: Some(word.to_string()),
        })
        .collect()
}

fn split_words(text: &str) -> Vec<&str> {
    text.split_whitespace()
        .filter(|word| !word.is_empty())
        .collect()
}

fn normalize_transcript_text(text: &str) -> String {
    split_words(text).join(" ")
}

fn synthetic_text_duration(text: &str) -> f64 {
    let word_count = split_words(text).len();
    if word_count == 0 {
        MIN_SYNTHETIC_DURATION_SECONDS
    } else {
        (word_count as f64 * SYNTHETIC_BATCH_WORD_SECONDS).max(MIN_SYNTHETIC_DURATION_SECONDS)
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod platform {
    use super::*;
    use swift_rs::{Bool, SRData, SRString, swift};

    swift!(fn _soniqo_model_cache_dir(model_id: &SRString) -> SRString);
    swift!(fn _soniqo_model_download_state(model_id: &SRString) -> SRString);
    swift!(fn _soniqo_model_start_download(model_id: &SRString) -> Bool);
    swift!(fn _soniqo_model_reset(model_id: &SRString) -> Bool);
    swift!(fn _soniqo_transcribe_audio_file(
        model_id: &SRString,
        audio_path: &SRString,
        language: &SRString
    ) -> SRString);
    swift!(fn _soniqo_live_start(model_id: &SRString) -> SRString);
    swift!(fn _soniqo_live_append(source: &SRString, samples: &SRData) -> SRString);
    swift!(fn _soniqo_live_finalize(source: &SRString) -> SRString);
    swift!(fn _soniqo_live_stop() -> SRString);

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct FileTranscriptionPayload {
        text: String,
        duration_seconds: f64,
        error: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct LiveAppendPayload {
        partials: Vec<LivePartial>,
        error: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct StatusPayload {
        running: bool,
        error: Option<String>,
    }

    pub(super) fn model_cache_dir(model: SoniqoModel) -> Result<PathBuf> {
        let model_id = sr_string(model.as_str());
        let path = unsafe { _soniqo_model_cache_dir(&model_id) };
        let path = path.as_str().to_string();

        if path.is_empty() {
            return Err(Error::Bridge(format!(
                "cache path unavailable for {}",
                model.as_str()
            )));
        }

        Ok(PathBuf::from(path))
    }

    pub(super) fn model_download_state(model: SoniqoModel) -> Result<ModelDownloadState> {
        let model_id = sr_string(model.as_str());
        let payload = unsafe { _soniqo_model_download_state(&model_id) };
        let state: ModelDownloadState = serde_json::from_str(payload.as_str())?;

        Ok(state)
    }

    pub(super) fn start_model_download(model: SoniqoModel) -> Result<()> {
        let model_id = sr_string(model.as_str());
        if unsafe { _soniqo_model_start_download(&model_id) } {
            Ok(())
        } else {
            Err(Error::Bridge(format!(
                "failed to start Soniqo download for {}",
                model.as_str()
            )))
        }
    }

    pub(super) fn reset_model(model: SoniqoModel) -> Result<()> {
        let model_id = sr_string(model.as_str());
        if unsafe { _soniqo_model_reset(&model_id) } {
            Ok(())
        } else {
            Err(Error::Bridge(format!(
                "failed to reset Soniqo model {}",
                model.as_str()
            )))
        }
    }

    pub(super) fn transcribe_file(
        model: SoniqoModel,
        path: &Path,
        language: &str,
    ) -> Result<FileTranscript> {
        let model_id = sr_string(model.as_str());
        let audio_path = sr_string(&path.to_string_lossy());
        let language = sr_string(language);
        let payload = unsafe { _soniqo_transcribe_audio_file(&model_id, &audio_path, &language) };
        let result: FileTranscriptionPayload = serde_json::from_str(payload.as_str())?;

        if let Some(error) = result.error {
            return Err(Error::Bridge(error));
        }

        Ok(FileTranscript {
            text: result.text,
            duration_seconds: result.duration_seconds,
            chunks: Vec::new(),
        })
    }

    pub(super) fn live_start(model: SoniqoModel) -> Result<()> {
        let model_id = sr_string(model.as_str());
        let payload = unsafe { _soniqo_live_start(&model_id) };
        let result: StatusPayload = serde_json::from_str(payload.as_str())?;

        if result.running {
            Ok(())
        } else {
            Err(Error::Bridge(result.error.unwrap_or_else(|| {
                "failed to start Soniqo live session".to_string()
            })))
        }
    }

    pub(super) fn live_append(
        source: TranscriptSource,
        samples: &[f32],
    ) -> Result<Vec<LivePartial>> {
        let source = sr_string(source.as_str());
        let samples = floats_to_sr_data(samples);
        let payload = unsafe { _soniqo_live_append(&source, &samples) };
        let result: LiveAppendPayload = serde_json::from_str(payload.as_str())?;

        if let Some(error) = result.error {
            return Err(Error::Bridge(error));
        }

        Ok(result.partials)
    }

    pub(super) fn live_finalize(source: TranscriptSource) -> Result<Vec<LivePartial>> {
        let source = sr_string(source.as_str());
        let payload = unsafe { _soniqo_live_finalize(&source) };
        let result: LiveAppendPayload = serde_json::from_str(payload.as_str())?;

        if let Some(error) = result.error {
            return Err(Error::Bridge(error));
        }

        Ok(result.partials)
    }

    pub(super) fn live_stop() -> Result<()> {
        let payload = unsafe { _soniqo_live_stop() };
        let result: StatusPayload = serde_json::from_str(payload.as_str())?;

        if let Some(error) = result.error {
            return Err(Error::Bridge(error));
        }

        Ok(())
    }

    fn sr_string(value: &str) -> SRString {
        SRString::from(value)
    }

    fn floats_to_sr_data(samples: &[f32]) -> SRData {
        let bytes = samples
            .iter()
            .flat_map(|sample| sample.to_bits().to_le_bytes())
            .collect::<Vec<_>>();
        SRData::from(bytes.as_slice())
    }
}

#[cfg(target_os = "windows")]
#[path = "windows.rs"]
mod platform;

#[cfg(not(any(
    target_os = "windows",
    all(target_os = "macos", target_arch = "aarch64")
)))]
mod platform {
    use super::*;

    pub(super) fn model_cache_dir(_model: SoniqoModel) -> Result<PathBuf> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn model_download_state(_model: SoniqoModel) -> Result<ModelDownloadState> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn start_model_download(_model: SoniqoModel) -> Result<()> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn reset_model(_model: SoniqoModel) -> Result<()> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn transcribe_file(
        _model: SoniqoModel,
        _path: &Path,
        _language: &str,
    ) -> Result<FileTranscript> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn live_start(_model: SoniqoModel) -> Result<()> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn live_append(
        _source: TranscriptSource,
        _samples: &[f32],
    ) -> Result<Vec<LivePartial>> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn live_finalize(_source: TranscriptSource) -> Result<Vec<LivePartial>> {
        Err(Error::UnsupportedPlatform)
    }

    pub(super) fn live_stop() -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_model_ids() {
        assert_eq!(
            "soniqo-parakeet-streaming".parse::<SoniqoModel>().unwrap(),
            SoniqoModel::ParakeetStreaming
        );
        assert_eq!(
            "soniqo-qwen3-small".parse::<SoniqoModel>().unwrap(),
            SoniqoModel::Qwen3Small
        );
        assert_eq!(
            "soniqo-qwen3-large".parse::<SoniqoModel>().unwrap(),
            SoniqoModel::Qwen3Large
        );
    }

    #[test]
    fn all_includes_available_model_variants() {
        if cfg!(target_os = "windows") {
            assert_eq!(SoniqoModel::all(), SoniqoModel::selectable());
            return;
        }
        assert_eq!(
            SoniqoModel::all(),
            &[
                SoniqoModel::ParakeetStreaming,
                SoniqoModel::ParakeetBatch,
                SoniqoModel::Omnilingual,
            ]
        );
    }

    #[test]
    fn selectable_includes_advertised_models() {
        assert_eq!(
            SoniqoModel::selectable(),
            &[
                SoniqoModel::ParakeetStreaming.resolved(),
                SoniqoModel::ParakeetBatch.resolved()
            ]
        );
    }

    #[test]
    fn saved_models_resolve_without_changing_their_serialized_ids() {
        let mac = "soniqo-parakeet-batch".parse::<SoniqoModel>().unwrap();
        let windows = "onnx-parakeet-batch".parse::<SoniqoModel>().unwrap();
        assert_eq!(mac.resolved(), windows.resolved());
        assert_eq!(mac.as_str(), "soniqo-parakeet-batch");
        assert_eq!(windows.as_str(), "onnx-parakeet-batch");
        assert!(windows.supports_language(&"nl".parse().unwrap()));
        assert!(!windows.supports_language(&"ko".parse().unwrap()));
    }

    #[test]
    fn parakeet_batch_supports_documented_european_languages() {
        let english = "en-US".parse().unwrap();
        let french = "fr".parse().unwrap();
        let dutch = "nl".parse().unwrap();

        assert!(SoniqoModel::ParakeetBatch.supports_language(&english));
        assert!(SoniqoModel::ParakeetBatch.supports_language(&french));
        assert!(SoniqoModel::ParakeetBatch.supports_language(&dutch));
    }

    #[test]
    fn parakeet_streaming_is_english_only() {
        let english = "en-US".parse().unwrap();
        let french = "fr".parse().unwrap();
        let dutch = "nl".parse().unwrap();

        assert!(SoniqoModel::ParakeetStreaming.supports_language(&english));
        assert!(!SoniqoModel::ParakeetStreaming.supports_language(&french));
        assert!(!SoniqoModel::ParakeetStreaming.supports_language(&dutch));
        assert!(
            SoniqoModel::ParakeetStreaming
                .batch_model()
                .supports_language(&dutch)
        );
    }

    #[test]
    fn parakeet_models_reject_unsupported_languages() {
        let korean = "ko".parse().unwrap();

        assert!(!SoniqoModel::ParakeetStreaming.supports_language(&korean));
        assert!(!SoniqoModel::ParakeetBatch.supports_language(&korean));
    }

    #[test]
    fn multilingual_models_support_non_english_languages() {
        let french = "fr".parse().unwrap();

        assert!(SoniqoModel::Omnilingual.supports_language(&french));
        assert!(SoniqoModel::Qwen3Small.supports_language(&french));
        assert!(SoniqoModel::Qwen3Large.supports_language(&french));
    }

    #[test]
    fn live_support_is_gated_by_platform() {
        assert_eq!(
            SoniqoModel::ParakeetStreaming.supports_live_on_current_platform(),
            cfg!(any(
                target_os = "windows",
                all(target_os = "macos", target_arch = "aarch64")
            )),
        );
        assert!(!SoniqoModel::ParakeetBatch.supports_live_on_current_platform());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn qwen3_platform_error_mentions_macos_15() {
        let error = ensure_supported_platform(SoniqoModel::Qwen3Small).unwrap_err();

        assert_eq!(
            error.to_string(),
            "Soniqo Qwen3 0.6B requires macOS 15 or newer."
        );
    }

    #[test]
    fn streaming_model_uses_batch_model_for_file_transcription() {
        assert_eq!(
            SoniqoModel::ParakeetStreaming.batch_model(),
            SoniqoModel::ParakeetBatch
        );
        assert_eq!(
            SoniqoModel::ParakeetBatch.batch_model(),
            SoniqoModel::ParakeetBatch
        );
    }

    #[test]
    fn batch_response_has_deepgram_shape() {
        let response =
            batch_response_from_text(SoniqoModel::ParakeetBatch, "hello world".to_string(), 2.0);

        assert_eq!(
            response.results.channels[0].alternatives[0].transcript,
            "hello world"
        );
        assert_eq!(response.results.channels[0].alternatives[0].words.len(), 2);
        assert_eq!(response.metadata["model_info"]["arch"], "soniqo");
        assert_eq!(response.metadata["duration"], 2.0);
        assert_eq!(response.metadata["timing_source"], "synthetic_text");
    }

    #[test]
    fn batch_response_preserves_channel_indexes() {
        let response = batch_response_from_channels(
            SoniqoModel::Omnilingual,
            vec![
                FileTranscript::new("mic words".to_string(), 2.0),
                FileTranscript::new("speaker words".to_string(), 3.0),
            ],
        );

        assert_eq!(response.metadata["channels"], 2);
        assert_eq!(response.metadata["duration"], 3.0);
        assert_eq!(response.results.channels.len(), 2);
        assert_eq!(
            response.results.channels[0].alternatives[0].transcript,
            "mic words"
        );
        assert_eq!(
            response.results.channels[1].alternatives[0].transcript,
            "speaker words"
        );
        assert_eq!(
            response.results.channels[0].alternatives[0].words[0].channel,
            0
        );
        assert_eq!(
            response.results.channels[1].alternatives[0].words[0].channel,
            1
        );
    }

    #[test]
    fn batch_response_uses_compact_synthetic_word_timing() {
        let response = batch_response_from_text(
            SoniqoModel::ParakeetBatch,
            "one two three four".to_string(),
            120.0,
        );
        let words = &response.results.channels[0].alternatives[0].words;

        assert_eq!(words[0].start, 0.0);
        assert_eq!(words[0].end, SYNTHETIC_BATCH_WORD_SECONDS);
        assert_eq!(words[3].end, 4.0 * SYNTHETIC_BATCH_WORD_SECONDS);
        assert!(words[3].end < 3.0);
        assert_eq!(response.metadata["duration"], 120.0);
    }

    #[test]
    fn batch_response_offsets_synthetic_words_by_chunk_start() {
        let response = batch_response_from_channels(
            SoniqoModel::ParakeetBatch,
            vec![FileTranscript::from_chunks(
                vec![
                    FileTranscriptChunk {
                        text: "early words".to_string(),
                        start_seconds: 0.0,
                        duration_seconds: 29.5,
                        speech_spans: Vec::new(),
                    },
                    FileTranscriptChunk {
                        text: "later words".to_string(),
                        start_seconds: 29.5,
                        duration_seconds: 29.5,
                        speech_spans: Vec::new(),
                    },
                ],
                59.0,
            )],
        );
        let words = &response.results.channels[0].alternatives[0].words;

        assert_eq!(words[0].start, 0.0);
        assert_eq!(words[1].start, SYNTHETIC_BATCH_WORD_SECONDS);
        assert_eq!(words[2].start, 29.5);
        assert_eq!(words[3].start, 29.5 + SYNTHETIC_BATCH_WORD_SECONDS);
        assert_eq!(response.metadata["timing_source"], "synthetic_text");
    }

    #[test]
    fn batch_response_places_words_inside_speech_spans() {
        let response = batch_response_from_channels(
            SoniqoModel::ParakeetBatch,
            vec![FileTranscript::from_chunks(
                vec![FileTranscriptChunk {
                    text: "one two three four".to_string(),
                    start_seconds: 10.0,
                    duration_seconds: 29.5,
                    speech_spans: vec![
                        SpeechSpan {
                            start_seconds: 12.0,
                            end_seconds: 14.0,
                        },
                        SpeechSpan {
                            start_seconds: 30.0,
                            end_seconds: 32.0,
                        },
                    ],
                }],
                40.0,
            )],
        );
        let words = &response.results.channels[0].alternatives[0].words;

        assert_eq!(words.len(), 4);
        assert_eq!(words[0].start, 12.0);
        assert!(words[1].end <= 14.0);
        assert!(words[2].start >= 30.0);
        assert!(words[3].end <= 32.0);
        for pair in words.windows(2) {
            assert!(pair[0].start <= pair[1].start);
        }
        assert_eq!(response.metadata["timing_source"], "synthetic_speech");
    }

    #[test]
    fn batch_response_ignores_speech_spans_outside_chunk() {
        let response = batch_response_from_channels(
            SoniqoModel::ParakeetBatch,
            vec![FileTranscript::from_chunks(
                vec![FileTranscriptChunk {
                    text: "hello world".to_string(),
                    start_seconds: 10.0,
                    duration_seconds: 5.0,
                    speech_spans: vec![SpeechSpan {
                        start_seconds: 100.0,
                        end_seconds: 110.0,
                    }],
                }],
                15.0,
            )],
        );
        let words = &response.results.channels[0].alternatives[0].words;

        assert_eq!(words[0].start, 10.0);
        assert_eq!(words[1].end, 10.0 + 2.0 * SYNTHETIC_BATCH_WORD_SECONDS);
    }

    #[test]
    fn batch_response_normalizes_internal_whitespace() {
        let response = batch_response_from_text(
            SoniqoModel::ParakeetBatch,
            "eins zwei\n drei\tvier".to_string(),
            4.0,
        );
        let alternative = &response.results.channels[0].alternatives[0];

        assert_eq!(alternative.transcript, "eins zwei drei vier");
        assert_eq!(
            alternative
                .words
                .iter()
                .map(|word| word.word.as_str())
                .collect::<Vec<_>>(),
            vec!["eins", "zwei", "drei", "vier"]
        );
    }

    #[test]
    fn live_response_keeps_source_channel() {
        let partial = LivePartial {
            source: "system".to_string(),
            text: "hello".to_string(),
            is_final: true,
        };

        let response = partial.into_stream_response(SoniqoModel::ParakeetStreaming, 0.0, 0.5);
        let stream::StreamResponse::TranscriptResponse { channel_index, .. } = response else {
            panic!("expected transcript response");
        };

        assert_eq!(channel_index, vec![1, 2]);
    }
}
