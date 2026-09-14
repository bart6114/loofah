use hypr_audio_chunking::AudioChunk;
use hypr_whisper_local::{LanguageResolver, Whisper};

pub(super) struct EvidenceWindow {
    pub end: usize,
    pub samples: Vec<f32>,
}

pub(super) fn evidence_windows(
    channels: &[Vec<f32>],
    chunks: &[Vec<AudioChunk>],
) -> Vec<EvidenceWindow> {
    let duration = channels.iter().map(Vec::len).max().unwrap_or(0);
    let mut result = Vec::new();
    for start in (0..duration).step_by(10 * 16000) {
        let end = (start + 10 * 16000).min(duration);
        let samples = channels
            .iter()
            .zip(chunks)
            .map(|(audio, chunks)| {
                let mut samples = Vec::new();
                let mut cursor = start;
                for chunk in chunks {
                    let a = chunk.sample_start.max(cursor);
                    let b = chunk.sample_end.min(end).min(audio.len());
                    if a < b {
                        samples.extend_from_slice(&audio[a..b]);
                        cursor = b;
                    }
                }
                samples
            })
            .max_by_key(Vec::len)
            .unwrap_or_default();
        if samples.len() >= 1600 {
            result.push(EvidenceWindow { end, samples });
        }
    }
    result
}

pub(super) struct BatchLanguage {
    pub resolver: LanguageResolver,
    windows: std::collections::VecDeque<EvidenceWindow>,
}

impl BatchLanguage {
    pub fn new(resolver: LanguageResolver, windows: Vec<EvidenceWindow>) -> Self {
        Self {
            resolver,
            windows: windows.into(),
        }
    }
    pub fn startup(&mut self, detector: &mut Whisper) -> Result<(), crate::Error> {
        while self.resolver.selected().is_none() {
            let Some(window) = self.windows.pop_front() else {
                break;
            };
            self.resolver.add_speech(window.samples.len());
            self.resolver
                .observe(detector.detect_language(&window.samples)?);
        }
        self.resolver.finish_startup();
        Ok(())
    }
    pub fn advance(&mut self, end: usize, detector: &mut Whisper) -> Result<(), crate::Error> {
        while self.windows.front().is_some_and(|w| w.end <= end) {
            let window = self.windows.pop_front().unwrap();
            self.resolver.add_speech(window.samples.len());
            if self.resolver.needs_observation() {
                self.resolver
                    .observe(detector.detect_language(&window.samples)?);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn chooses_one_active_channel_per_nonoverlapping_window() {
        let audio = vec![vec![1.0; 320000], vec![2.0; 320000]];
        let chunk = |a, b| AudioChunk {
            sample_start: a,
            sample_end: b,
            samples: vec![],
        };
        let windows = evidence_windows(
            &audio,
            &[
                vec![chunk(0, 100000), chunk(160000, 170000)],
                vec![chunk(0, 10000), chunk(160000, 300000)],
            ],
        );
        assert_eq!(windows.len(), 2);
        assert!(windows[0].samples.iter().all(|x| *x == 1.0));
        assert!(windows[1].samples.iter().all(|x| *x == 2.0));
    }
}
