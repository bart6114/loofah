//! The same Whisper passthrough processor used by the desktop listener, without
//! its device/actor lifecycle. Callers retain deltas and own canonical storage.

use hypr_transcript::{TranscriptDelta, TranscriptProcessor};
use owhisper_interface::stream::StreamResponse;

pub struct LiveTranscriptMapper {
    processor: TranscriptProcessor,
    mono: bool,
}

impl LiveTranscriptMapper {
    pub fn new(channels: usize) -> Option<Self> {
        (1..=2).contains(&channels).then(|| Self {
            processor: TranscriptProcessor::new(),
            mono: channels == 1,
        })
    }

    pub fn process(&mut self, response: &StreamResponse) -> Option<TranscriptDelta> {
        let mut response = response.clone();
        if self.mono {
            response.remap_channel_index(0, 2);
        }
        self.processor.process(&response)
    }

    pub fn flush(&mut self) -> TranscriptDelta {
        self.processor.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use owhisper_interface::stream::{Alternatives, Channel, Metadata, Word};

    fn response(channel: i32, at: f64, text: &str) -> StreamResponse {
        StreamResponse::TranscriptResponse {
            start: at,
            duration: 1.0,
            is_final: true,
            speech_final: true,
            from_finalize: false,
            channel: Channel {
                alternatives: vec![Alternatives {
                    transcript: text.into(),
                    words: vec![Word {
                        word: text.into(),
                        start: at,
                        end: at + 1.0,
                        confidence: 0.9,
                        speaker: None,
                        punctuated_word: None,
                        language: Some("en".into()),
                    }],
                    confidence: 0.9,
                    languages: vec!["en".into()],
                }],
            },
            metadata: Metadata::default(),
            channel_index: vec![channel],
        }
    }

    #[test]
    fn live_mapper_matches_desktop_processor_for_dual_and_preserves_tail() {
        let mut mapper = LiveTranscriptMapper::new(2).unwrap();
        let mut desktop = TranscriptProcessor::new();
        let mut mapped = Vec::new();
        let mut expected = Vec::new();
        for event in [response(0, 1.0, "microphone"), response(1, 2.0, "system")] {
            if let Some(delta) = mapper.process(&event) {
                mapped.extend(delta.new_words);
            }
            if let Some(delta) = desktop.process(&event) {
                expected.extend(delta.new_words);
            }
        }
        mapped.extend(mapper.flush().new_words);
        expected.extend(desktop.flush().new_words);
        assert_eq!(mapped.len(), 2);
        for (a, b) in mapped.iter().zip(expected) {
            assert_eq!(
                (&a.text, a.channel, a.start_ms, a.end_ms),
                (&b.text, b.channel, b.start_ms, b.end_ms)
            );
        }
    }

    #[test]
    fn live_mapper_projects_any_mono_physical_channel_zero_to_mixed_domain_two() {
        let mut mapper = LiveTranscriptMapper::new(1).unwrap();
        let mut words = mapper
            .process(&response(0, 0.0, "mono"))
            .map(|delta| delta.new_words)
            .unwrap_or_default();
        words.extend(mapper.flush().new_words);
        assert_eq!(words.len(), 1);
        assert_eq!(words[0].channel, 2);
        assert_eq!((words[0].start_ms, words[0].end_ms), (0, 1000));
        assert!(LiveTranscriptMapper::new(3).is_none());
    }
}
