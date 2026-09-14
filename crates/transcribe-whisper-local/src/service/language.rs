use hypr_audio_chunking::AudioChunk;
use hypr_whisper_local::{LanguageResolver, Whisper};

pub(super) struct EvidenceWindow {
    end: usize,
    channel: usize,
    spans: Vec<std::ops::Range<usize>>,
    speech_samples: usize,
}

impl EvidenceWindow {
    fn samples(&self, channels: &[Vec<f32>]) -> Vec<f32> {
        self.spans
            .iter()
            .flat_map(|r| channels[self.channel][r.clone()].iter().copied())
            .collect()
    }
}

pub(super) fn evidence_windows(
    channels: &[Vec<f32>],
    chunks: &[Vec<AudioChunk>],
) -> Vec<EvidenceWindow> {
    let duration = channels.iter().map(Vec::len).max().unwrap_or(0);
    let mut result = Vec::new();
    let mut indices = vec![0; channels.len()];
    let mut accumulated: Vec<Vec<std::ops::Range<usize>>> = vec![Vec::new(); channels.len()];
    for start in (0..duration).step_by(10 * 16000) {
        let end = (start + 10 * 16000).min(duration);
        let mut candidates = Vec::new();
        for (channel, (audio, chunks)) in channels.iter().zip(chunks).enumerate() {
            while indices[channel] < chunks.len() && chunks[indices[channel]].sample_end <= start {
                indices[channel] += 1;
            }
            let mut cursor = start;
            let spans = &mut accumulated[channel];
            for chunk in chunks[indices[channel]..]
                .iter()
                .take_while(|c| c.sample_start < end)
            {
                let a = chunk.sample_start.max(cursor);
                let b = chunk.sample_end.min(end).min(audio.len());
                if a < b {
                    spans.push(a..b);
                    cursor = b;
                }
            }
            candidates.push((channel, spans.iter().map(|r| r.len()).sum::<usize>()));
        }
        if let Some((channel, len)) = candidates.into_iter().max_by_key(|(_, len)| *len)
            && (len >= 10 * 16000 || end == duration)
        {
            if len >= 1600 {
                let mut remaining = 10 * 16000;
                let spans = std::mem::take(&mut accumulated[channel])
                    .into_iter()
                    .filter_map(|r| {
                        let count = r.len().min(remaining);
                        remaining -= count;
                        (count > 0).then_some(r.start..r.start + count)
                    })
                    .collect();
                result.push(EvidenceWindow {
                    end,
                    channel,
                    spans,
                    speech_samples: len,
                });
            }
            for spans in &mut accumulated {
                spans.clear();
            }
        }
    }
    result
}

pub(super) struct BatchLanguage<'a> {
    pub resolver: LanguageResolver,
    windows: std::collections::VecDeque<EvidenceWindow>,
    channels: &'a [Vec<f32>],
}

impl<'a> BatchLanguage<'a> {
    pub fn new(
        resolver: LanguageResolver,
        windows: Vec<EvidenceWindow>,
        channels: &'a [Vec<f32>],
    ) -> Self {
        Self {
            resolver,
            windows: windows.into(),
            channels,
        }
    }
    pub fn startup(&mut self, detector: &mut Whisper) -> Result<(), crate::Error> {
        while self.resolver.selected().is_none() {
            let Some(window) = self.windows.pop_front() else {
                break;
            };
            self.resolver.add_speech(window.speech_samples);
            self.resolver
                .observe(detector.detect_language(&window.samples(self.channels))?);
        }
        self.resolver.finish_startup();
        Ok(())
    }
    pub fn advance(&mut self, end: usize, detector: &mut Whisper) -> Result<(), crate::Error> {
        while self.windows.front().is_some_and(|w| w.end <= end) {
            let window = self.windows.pop_front().unwrap();
            self.resolver.add_speech(window.speech_samples);
            if self.resolver.needs_observation() {
                self.resolver
                    .observe(detector.detect_language(&window.samples(self.channels))?);
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
                vec![chunk(0, 160000), chunk(160000, 170000)],
                vec![chunk(0, 10000), chunk(160000, 300000)],
            ],
        );
        assert_eq!(windows.len(), 2);
        assert!(windows[0].samples(&audio).iter().all(|x| *x == 1.0));
        assert!(windows[1].samples(&audio).iter().all(|x| *x == 2.0));
    }
}
