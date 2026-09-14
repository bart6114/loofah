// https://github.com/tazz4843/whisper-rs/blob/master/examples/audio_transcription.rs

use lazy_static::lazy_static;
use regex::Regex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use whisper_rs::{
    FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    WhisperTokenId,
};

use hypr_whisper::Language;

use crate::Segment;

lazy_static! {
    static ref TRAILING_DOTS: Regex = Regex::new(r"\.{2,}$").unwrap();
}

#[derive(Default)]
pub struct LoadedWhisperBuilder {
    model_path: Option<String>,
}

impl LoadedWhisperBuilder {
    pub fn model_path(mut self, model_path: impl Into<String>) -> Self {
        self.model_path = Some(model_path.into());
        self
    }

    pub fn build(self) -> Result<LoadedWhisper, crate::Error> {
        unsafe { Self::suppress_log() };

        let context_param = {
            let mut p = WhisperContextParameters {
                gpu_device: 0,
                use_gpu: true,
                flash_attn: std::env::var("LOOFAH_WHISPER_FLASH_ATTN").as_deref() == Ok("1"),
                ..Default::default()
            };
            p.dtw_parameters.mode = whisper_rs::DtwMode::None;
            p
        };

        let model_path = self.model_path.unwrap();
        if !std::path::Path::new(&model_path).exists() {
            return Err(crate::Error::ModelNotFound);
        }

        let start = std::time::Instant::now();
        tracing::info!(model = ?std::path::Path::new(&model_path).file_name(), engine = whisper_rs::get_whisper_version(), gpu = context_param.use_gpu, metal = cfg!(feature = "metal"), flash_attn = context_param.flash_attn, "whisper_model_loading");
        let ctx = WhisperContext::new_with_params(&model_path, context_param)?;
        tracing::info!(
            elapsed_ms = start.elapsed().as_millis(),
            "whisper_model_loaded"
        );
        let token_beg = ctx.token_beg();

        Ok(LoadedWhisper { ctx, token_beg })
    }

    unsafe fn suppress_log() {
        unsafe extern "C" fn noop_callback(
            _level: whisper_rs::whisper_rs_sys::ggml_log_level,
            _text: *const ::std::os::raw::c_char,
            _user_data: *mut ::std::os::raw::c_void,
        ) {
        }
        unsafe { whisper_rs::set_log_callback(Some(noop_callback), std::ptr::null_mut()) };
    }
}

#[derive(Default)]
pub struct WhisperBuilder {
    model_path: Option<String>,
    languages: Option<Vec<Language>>,
}

impl WhisperBuilder {
    pub fn model_path(mut self, model_path: impl Into<String>) -> Self {
        self.model_path = Some(model_path.into());
        self
    }

    pub fn languages(mut self, languages: Vec<Language>) -> Self {
        self.languages = Some(languages);
        self
    }

    pub fn build(self) -> Result<Whisper, crate::Error> {
        LoadedWhisper::builder()
            .model_path(self.model_path.unwrap())
            .build()?
            .session(self.languages.unwrap_or_default())
    }
}

pub struct LoadedWhisper {
    ctx: WhisperContext,
    token_beg: WhisperTokenId,
}

impl LoadedWhisper {
    pub fn builder() -> LoadedWhisperBuilder {
        LoadedWhisperBuilder::default()
    }

    pub fn session(&self, languages: Vec<Language>) -> Result<Whisper, crate::Error> {
        Ok(Whisper {
            id: uuid::Uuid::new_v4().to_string(),
            index: 0,
            languages,
            native_timestamps: false,
            selected_language: None,
            detection_calls: 0,
            inference_calls: 0,
            detection_threads: if self.ctx.model_n_audio_layer() == 12 {
                4
            } else {
                1
            },
            cancelled: Arc::new(AtomicBool::new(false)),
            dynamic_prompt: String::new(),
            initial_prompt: String::new(),
            state: self.ctx.create_state()?,
            token_beg: self.token_beg,
        })
    }
}

pub struct Whisper {
    #[allow(dead_code)]
    id: String,
    #[allow(dead_code)]
    index: usize,
    languages: Vec<Language>,
    native_timestamps: bool,
    selected_language: Option<String>,
    detection_calls: usize,
    inference_calls: usize,
    detection_threads: usize,
    cancelled: Arc<AtomicBool>,
    dynamic_prompt: String,
    initial_prompt: String,
    state: WhisperState,
    token_beg: WhisperTokenId,
}

impl Whisper {
    pub fn set_native_timestamps(&mut self, enabled: bool) {
        self.native_timestamps = enabled;
    }
    pub fn counters(&self) -> (usize, usize) {
        (self.detection_calls, self.inference_calls)
    }

    pub fn select_language(&mut self, language: Option<&str>) {
        if self.selected_language.as_deref() != language {
            self.dynamic_prompt.clear();
            self.selected_language = language.map(str::to_owned);
        }
    }

    pub fn set_cancellation(&mut self, cancelled: Arc<AtomicBool>) {
        self.cancelled = cancelled;
    }

    pub fn detect_language(&mut self, audio: &[f32]) -> Result<crate::Observation, crate::Error> {
        if self.cancelled.load(Ordering::Acquire) {
            return Err(crate::Error::Cancelled);
        }
        let started = std::time::Instant::now();
        let threads = std::env::var("LOOFAH_WHISPER_DETECTION_THREADS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| [1, 2, 4].contains(n))
            .unwrap_or(self.detection_threads);
        self.detection_calls += 1;
        self.state.pcm_to_mel(audio, threads)?;
        if self.cancelled.load(Ordering::Acquire) {
            return Err(crate::Error::Cancelled);
        }
        let (_, probabilities) = self.state.lang_detect(0, threads)?;
        if self.cancelled.load(Ordering::Acquire) {
            return Err(crate::Error::Cancelled);
        }
        let scores = if self.languages.is_empty() {
            probabilities
                .iter()
                .enumerate()
                .filter_map(|(i, p)| {
                    whisper_rs::get_lang_str(i as i32).map(|lang| (lang.to_owned(), *p))
                })
                .collect()
        } else {
            self.languages
                .iter()
                .filter_map(|lang| {
                    probabilities
                        .get(lang.whisper_index())
                        .map(|p| (lang.to_string(), *p))
                })
                .collect()
        };
        tracing::debug!(elapsed_ms = started.elapsed().as_millis(), threads, samples = audio.len(), scores = ?scores, "whisper_language_detection");
        Ok(crate::Observation { scores })
    }

    pub fn set_initial_prompt(&mut self, prompt: String) {
        self.initial_prompt = prompt;
    }

    pub fn builder() -> WhisperBuilder {
        WhisperBuilder::default()
    }

    pub fn transcribe(&mut self, audio: &[f32]) -> Result<Vec<Segment>, crate::Error> {
        #[cfg(debug_assertions)]
        self.debug(audio);

        if self.cancelled.load(Ordering::Acquire) {
            return Err(crate::Error::Cancelled);
        }
        let started = std::time::Instant::now();
        let input_audio_length_sec = audio.len() as f32 / 16000.0;
        if input_audio_length_sec < 0.1 {
            tracing::warn!(input_audio_length_sec = ?input_audio_length_sec, "transcribe_skipped");
            return Ok(vec![]);
        }

        let token_beg = self.token_beg;
        let language = self.get_language(audio)?;

        let params = {
            let mut p = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });

            let parts = [self.dynamic_prompt.trim(), self.initial_prompt.trim()];
            let joined = parts.join("\n");
            let initial_prompt = joined.trim();

            tracing::info!(input_audio_length_sec = ?input_audio_length_sec, "transcribe_started");

            p.set_n_threads(4);
            // All text context is explicit, so a language change cannot retain decoder text.
            p.set_no_context(true);
            unsafe extern "C" fn abort(data: *mut std::ffi::c_void) -> bool {
                unsafe { (*(data as *const AtomicBool)).load(Ordering::Acquire) }
            }
            unsafe {
                p.set_abort_callback(Some(abort));
                p.set_abort_callback_user_data(
                    Arc::as_ptr(&self.cancelled) as *mut std::ffi::c_void
                );
            }
            p.set_translate(false);
            p.set_detect_language(false);
            p.set_language(language.as_deref());

            p.set_initial_prompt(initial_prompt);

            if !self.native_timestamps {
                unsafe {
                    Self::suppress_beg(&mut p, &token_beg);
                }
            }

            p.set_no_timestamps(!self.native_timestamps);
            p.set_token_timestamps(false);
            p.set_split_on_word(true);

            p.set_temperature(0.0);
            p.set_temperature_inc(0.2);

            p.set_single_segment(!self.native_timestamps);
            p.set_suppress_blank(true);
            p.set_suppress_nst(true);

            p.set_print_special(false);
            p.set_print_progress(false);
            p.set_print_realtime(false);
            p.set_print_timestamps(false);
            p
        };

        self.inference_calls += 1;
        self.state.full(params, audio)?;
        if self.cancelled.load(Ordering::Acquire) {
            return Err(crate::Error::Cancelled);
        }
        tracing::info!(
            elapsed_ms = started.elapsed().as_millis(),
            samples = audio.len(),
            "whisper_inference_completed"
        );
        let num_segments = self.state.full_n_segments();

        let mut segments = Vec::new();
        for i in 0..num_segments {
            let segment = match self.state.get_segment(i) {
                Some(seg) => seg,
                None => continue,
            };

            let (start, end) = (
                (segment.start_timestamp() as f64) / 100.0,
                (segment.end_timestamp() as f64) / 100.0,
            );

            let text = {
                let segment_text = segment.to_str_lossy()?;
                TRAILING_DOTS.replace(&segment_text, "").to_string()
            };

            segments.push(Segment {
                text,
                language: language.clone(),
                start,
                end,
                // https://github.com/ggml-org/whisper.cpp/pull/971/files#diff-2d3599a9fad195f2c3c60bd06691bc1815325b3560b5feda41a91fa71194e805R310-R327
                // We previously implemented it based on above, but after updating to v1.7.6, the API has changed, and we're still unable to figure it out. We're not using it anyway.
                confidence: 1.0,
                ..Default::default()
            });
        }

        let segments = Self::filter_segments(segments);

        let full_text = segments
            .iter()
            .map(|s| s.text())
            .collect::<Vec<&str>>()
            .join(" ");

        if !full_text.is_empty() {
            tracing::info!(text_length = full_text.len(), "transcribe_completed");
            self.dynamic_prompt = full_text;
        }

        Ok(segments)
    }

    fn get_language(&mut self, audio: &[f32]) -> Result<Option<String>, crate::Error> {
        if let Some(language) = &self.selected_language {
            return Ok(Some(language.clone()));
        }
        if self.languages.len() == 1 {
            return Ok(Some(self.languages[0].to_string()));
        }
        let observation = self.detect_language(audio)?;
        Ok(observation
            .scores
            .into_iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(lang, _)| lang))
    }

    fn filter_segments(segments: Vec<Segment>) -> Vec<Segment> {
        segments
            .into_iter()
            .filter(|s| {
                let t = s.text.trim().to_lowercase();

                !(s.confidence < 0.005
                    || t == "you"
                    || t == "thank you"
                    || t == "you."
                    || t == "thank you."
                    || t == "♪")
            })
            .collect()
    }

    unsafe fn suppress_beg(params: &mut FullParams, token_beg: &WhisperTokenId) {
        unsafe extern "C" fn logits_filter_callback(
            _ctx: *mut whisper_rs::whisper_rs_sys::whisper_context,
            _state: *mut whisper_rs::whisper_rs_sys::whisper_state,
            _tokens: *const whisper_rs::whisper_rs_sys::whisper_token_data,
            _n_tokens: std::os::raw::c_int,
            logits: *mut f32,
            user_data: *mut std::os::raw::c_void,
        ) {
            if logits.is_null() || user_data.is_null() {
                return;
            }

            unsafe {
                let token_beg_id = *(user_data as *const WhisperTokenId);
                *logits.offset(token_beg_id as isize) = f32::NEG_INFINITY;
            }
        }

        unsafe {
            params.set_filter_logits_callback(Some(logits_filter_callback));
            params.set_filter_logits_callback_user_data(
                token_beg as *const WhisperTokenId as *mut std::ffi::c_void,
            );
        }
    }

    #[cfg(debug_assertions)]
    fn debug(&mut self, audio: &[f32]) {
        if let Ok(v) = std::env::var("HYPR_WHISPER_DEBUG")
            && v == "1"
        {
            let mut writer = hound::WavWriter::create(
                format!("./whisper_{}_{}.wav", self.id, self.index),
                hound::WavSpec {
                    channels: 1,
                    sample_rate: 16000,
                    bits_per_sample: 32,
                    sample_format: hound::SampleFormat::Float,
                },
            )
            .unwrap();
            self.index += 1;

            for sample in audio {
                writer.write_sample(*sample).unwrap();
            }
            writer.finalize().unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linked_native_engine_matches_pinned_release() {
        assert_eq!(whisper_rs::get_whisper_version(), "1.9.4");
    }

    #[test]
    #[ignore = "requires LOOFAH_WHISPER_MODEL"]
    fn language_change_preserves_vocabulary_and_cancellation_skips_detection() {
        let loaded = LoadedWhisper::builder()
            .model_path(std::env::var("LOOFAH_WHISPER_MODEL").unwrap())
            .build()
            .unwrap();
        let mut model = loaded.session(vec![Language::En, Language::Nl]).unwrap();
        model.set_initial_prompt("Kubernetes, Loofah".into());
        model.select_language(Some("en"));
        model.dynamic_prompt = "previous English context".into();
        model.select_language(Some("en"));
        assert!(!model.dynamic_prompt.is_empty());
        model.select_language(Some("nl"));
        assert!(model.dynamic_prompt.is_empty());
        assert_eq!(model.initial_prompt, "Kubernetes, Loofah");
        model.set_cancellation(Arc::new(AtomicBool::new(true)));
        assert!(matches!(
            model.detect_language(&[0.0; 16000]),
            Err(crate::Error::Cancelled)
        ));
        assert!(matches!(
            model.transcribe(&[0.0; 16000]),
            Err(crate::Error::Cancelled)
        ));
        assert_eq!(model.counters(), (0, 0));
    }

    #[test]
    fn test_whisper() {
        let mut whisper = Whisper::builder()
            .model_path(concat!(env!("CARGO_MANIFEST_DIR"), "/model.bin"))
            .build()
            .unwrap();

        let audio: Vec<f32> = hypr_data::english_1::AUDIO
            .chunks_exact(2)
            .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]) as f32 / 32768.0)
            .collect();

        let start = std::time::Instant::now();
        let segments = whisper.transcribe(&audio).unwrap();
        let duration = start.elapsed();
        println!("segments: {:#?}", segments);
        println!("time: {:?}", duration);
        assert!(segments.len() > 0);
    }
}
