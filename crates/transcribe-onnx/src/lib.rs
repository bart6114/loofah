use std::{collections::VecDeque, path::Path};

use anyhow::{Context, Result};
use parakeet_rs::{
    ExecutionConfig as ModelConfig, ParakeetEOU, ParakeetEOUHandle, ParakeetTDT, TimestampMode,
    Transcriber,
};

pub mod diarize;
pub mod models;

const SAMPLE_RATE: usize = 16000;
const LIVE_CHUNK: usize = 2560;

fn execution_config() -> ModelConfig {
    let cores = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(2);
    ModelConfig::default()
        .with_intra_threads(cores.clamp(1, 4))
        .with_inter_threads(1)
}

pub struct BatchSession(ParakeetTDT);

pub struct Segment {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

impl BatchSession {
    pub fn load() -> Result<Self> {
        let path = models::Model::Batch.verify()?;
        Ok(Self(ParakeetTDT::from_pretrained(
            path,
            Some(execution_config()),
        )?))
    }

    pub fn transcribe_file(&mut self, path: &Path) -> Result<(f64, Vec<Segment>)> {
        let reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        let duration = reader.duration() as f64 / spec.sample_rate as f64;
        anyhow::ensure!(
            spec.sample_rate == 16000 && spec.channels == 1,
            "Speech input must be 16 kHz mono WAV"
        );
        drop(reader);
        let result = self.0.transcribe_file(path, Some(TimestampMode::Words))?;
        let segments = result
            .tokens
            .into_iter()
            .map(|token| Segment {
                text: token.text,
                start: token.start as f64,
                end: token.end as f64,
            })
            .collect();
        Ok((duration, segments))
    }
}

#[derive(Debug, PartialEq)]
pub struct Partial {
    pub text: String,
    pub is_final: bool,
}

#[derive(Default)]
struct Utterance(String);

impl Utterance {
    fn append(&mut self, delta: &str) -> Option<Partial> {
        let final_utterance = delta.contains("[EOU]");
        self.0
            .push_str(&delta.replace("[EOU]", "").replace('▁', " "));
        let text = self.0.trim().to_owned();
        if final_utterance {
            self.0.clear();
        }
        if text.is_empty() || (delta.is_empty() && !final_utterance) {
            return None;
        }
        Some(Partial {
            text,
            is_final: final_utterance,
        })
    }

    fn finalize(&mut self) -> Option<Partial> {
        let text = std::mem::take(&mut self.0).trim().to_owned();
        (!text.is_empty()).then_some(Partial {
            text,
            is_final: true,
        })
    }
}

pub struct LiveSession {
    engine: ParakeetEOU,
    pending: VecDeque<f32>,
    utterance: Utterance,
    finished: bool,
}

impl LiveSession {
    pub fn load() -> Result<Self> {
        Self::from_verified_path(&models::Model::Streaming.verify()?)
    }

    pub fn load_pair() -> Result<[Self; 2]> {
        let path = models::Model::Streaming.verify()?;
        let handle = ParakeetEOUHandle::from_pretrained(path, Some(execution_config()))?;
        Ok([
            Self::from_engine(ParakeetEOU::from_shared(&handle))?,
            Self::from_engine(ParakeetEOU::from_shared(&handle))?,
        ])
    }

    fn from_verified_path(path: &Path) -> Result<Self> {
        Self::from_engine(ParakeetEOU::from_pretrained(
            path,
            Some(execution_config()),
        )?)
    }

    fn from_engine(mut engine: ParakeetEOU) -> Result<Self> {
        // The decoder waits for a second of feature context. Prime with silence so
        // the first second of real speech is decoded instead of used only as context.
        engine.transcribe(&vec![0.0; SAMPLE_RATE], false)?;
        Ok(Self {
            engine,
            pending: VecDeque::new(),
            utterance: Utterance::default(),
            finished: false,
        })
    }

    pub fn append(&mut self, samples: &[f32]) -> Result<Vec<Partial>> {
        anyhow::ensure!(!self.finished, "Live stream already finalized");
        anyhow::ensure!(
            samples.iter().all(|s| s.is_finite()),
            "Invalid audio samples"
        );
        self.pending.extend(samples);
        let mut partials = Vec::new();
        while self.pending.len() >= LIVE_CHUNK {
            let chunk: Vec<_> = self.pending.drain(..LIVE_CHUNK).collect();
            let delta = self
                .engine
                .transcribe(&chunk, true)
                .context("Live speech decoding failed")?;
            if let Some(partial) = self.utterance.append(&delta) {
                partials.push(partial);
            }
        }
        Ok(partials)
    }

    pub fn finalize(&mut self) -> Result<Vec<Partial>> {
        if self.finished {
            return Ok(Vec::new());
        }
        let padding = LIVE_CHUNK - self.pending.len() % LIVE_CHUNK;
        let mut partials = self.append(&vec![0.0; padding + SAMPLE_RATE * 2])?;
        if let Some(partial) = self.utterance.finalize() {
            partials.push(partial);
        }
        self.finished = true;
        Ok(partials)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_deltas_accumulate_and_finalization_does_not_duplicate() {
        let mut utterance = Utterance::default();
        assert_eq!(
            utterance.append(" Hello"),
            Some(Partial {
                text: "Hello".into(),
                is_final: false
            })
        );
        assert_eq!(
            utterance.append(" world [EOU]"),
            Some(Partial {
                text: "Hello world".into(),
                is_final: true
            })
        );
        assert_eq!(utterance.finalize(), None);
        assert_eq!(
            utterance.append(" Next"),
            Some(Partial {
                text: "Next".into(),
                is_final: false
            })
        );
        assert_eq!(
            utterance.finalize(),
            Some(Partial {
                text: "Next".into(),
                is_final: true
            })
        );
        assert_eq!(utterance.finalize(), None);
    }

    #[test]
    fn silence_does_not_emit_empty_transcripts() {
        let mut utterance = Utterance::default();
        assert_eq!(utterance.append(""), None);
        assert_eq!(utterance.append(" [EOU]"), None);
        assert_eq!(utterance.finalize(), None);
    }
}
