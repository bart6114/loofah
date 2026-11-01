use std::collections::VecDeque;

use hypr_vad::earshot::VoiceActivityDetector;

const FRAME_SAMPLES: usize = 320;
const PRE_ROLL_SAMPLES: usize = 3_200;
const SILENCE_SAMPLES: usize = 12_800;
const MAX_SEGMENT_SAMPLES: usize = 480_000;

pub(super) struct Action {
    pub samples: Vec<f32>,
    pub start_sample: usize,
    pub end_sample: usize,
    pub is_final: bool,
}

pub(super) struct Endpoint {
    vad: VoiceActivityDetector,
    pending: Vec<f32>,
    pre_roll: VecDeque<f32>,
    cursor: usize,
    segment_start: Option<usize>,
    trailing_silence: usize,
}

impl Endpoint {
    pub fn new() -> Self {
        Self {
            vad: VoiceActivityDetector::new(),
            pending: Vec::new(),
            pre_roll: VecDeque::new(),
            cursor: 0,
            segment_start: None,
            trailing_silence: 0,
        }
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<Action> {
        self.pending.extend_from_slice(samples);
        let buffered = std::mem::take(&mut self.pending);
        let mut frames = buffered.chunks_exact(FRAME_SAMPLES);
        let mut actions = Vec::new();
        for frame in &mut frames {
            let speech = self.is_speech(frame);
            self.process_frame(frame, speech, &mut actions);
        }
        self.pending.extend_from_slice(frames.remainder());
        actions
    }

    pub fn finish(&mut self) -> Vec<Action> {
        let mut actions = Vec::new();
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            let speech = self.is_speech(&pending);
            self.process_frame(&pending, speech, &mut actions);
        }
        self.finalize(&mut actions);
        actions
    }

    fn is_speech(&mut self, samples: &[f32]) -> bool {
        let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
            / samples.len() as f32)
            .sqrt();
        if rms < 0.0005 {
            return false;
        }
        let mut frame = [0i16; FRAME_SAMPLES];
        for (dest, sample) in frame.iter_mut().zip(samples) {
            *dest = (sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        }
        self.vad.predict_16khz(&frame).unwrap_or(true)
    }

    fn process_frame(&mut self, frame: &[f32], speech: bool, actions: &mut Vec<Action>) {
        let frame_start = self.cursor;
        self.cursor += frame.len();
        if self.segment_start.is_none() && speech {
            self.segment_start = Some(frame_start - self.pre_roll.len());
            let samples = self.pre_roll.drain(..).collect();
            actions.push(Action {
                samples,
                start_sample: self.segment_start.unwrap(),
                end_sample: frame_start,
                is_final: false,
            });
        }

        if let Some(start) = self.segment_start {
            if let Some(previous) = actions.last_mut().filter(|action| !action.is_final) {
                previous.samples.extend_from_slice(frame);
                previous.end_sample = self.cursor;
            } else {
                actions.push(Action {
                    samples: frame.to_vec(),
                    start_sample: start,
                    end_sample: self.cursor,
                    is_final: false,
                });
            }
            self.trailing_silence = if speech {
                0
            } else {
                self.trailing_silence + frame.len()
            };
            if self.trailing_silence >= SILENCE_SAMPLES
                || self.cursor - start >= MAX_SEGMENT_SAMPLES
            {
                self.finalize(actions);
            }
        } else {
            self.pre_roll.extend(frame);
            while self.pre_roll.len() > PRE_ROLL_SAMPLES {
                self.pre_roll.pop_front();
            }
        }
    }

    fn finalize(&mut self, actions: &mut Vec<Action>) {
        if let Some(start) = self.segment_start.take() {
            actions.push(Action {
                samples: Vec::new(),
                start_sample: start,
                end_sample: self.cursor,
                is_final: true,
            });
        }
        self.trailing_silence = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(endpoint: &mut Endpoint, speech: bool, count: usize) -> Vec<Action> {
        let mut actions = Vec::new();
        for _ in 0..count {
            endpoint.process_frame(&[0.1; FRAME_SAMPLES], speech, &mut actions);
        }
        actions
    }

    #[test]
    fn silence_is_bounded_and_does_not_create_segments() {
        let mut endpoint = Endpoint::new();
        assert!(endpoint.push(&vec![0.0; 160_117]).is_empty());
        assert!(endpoint.finish().is_empty());
        assert_eq!(endpoint.cursor, 160_117);
        assert!(endpoint.pre_roll.len() <= PRE_ROLL_SAMPLES);
    }

    #[test]
    fn endpoints_include_pre_roll_and_wait_for_eight_hundred_ms_of_silence() {
        let mut endpoint = Endpoint::new();
        assert!(frames(&mut endpoint, false, 50).is_empty());
        let first = frames(&mut endpoint, true, 5);
        assert_eq!(first[0].start_sample, 16_000 - PRE_ROLL_SAMPLES);
        assert!(
            !frames(&mut endpoint, false, 39)
                .iter()
                .any(|action| action.is_final)
        );
        let finals = frames(&mut endpoint, false, 1);
        assert_eq!(finals.iter().filter(|action| action.is_final).count(), 1);
        assert!(endpoint.finish().is_empty());
        assert!(frames(&mut endpoint, false, 50).is_empty());
        let next = frames(&mut endpoint, true, 1);
        assert!(next[0].start_sample >= finals.last().unwrap().end_sample);
    }

    #[test]
    fn speech_across_push_boundaries_keeps_offsets_and_flushes_the_tail_once() {
        let mut endpoint = Endpoint::new();
        frames(&mut endpoint, true, 1);
        assert!(endpoint.push(&[0.0; 117]).is_empty());
        let tail = endpoint.finish();
        assert_eq!(tail[0].samples.len(), 117);
        assert_eq!(tail.last().unwrap().end_sample, FRAME_SAMPLES + 117);
        assert_eq!(tail.iter().filter(|action| action.is_final).count(), 1);
        assert!(endpoint.finish().is_empty());
    }

    #[test]
    fn continuous_speech_is_split_without_losing_audio() {
        let mut endpoint = Endpoint::new();
        let actions = frames(&mut endpoint, true, MAX_SEGMENT_SAMPLES / FRAME_SAMPLES + 1);
        assert_eq!(actions.iter().filter(|action| action.is_final).count(), 1);
        assert_eq!(
            actions
                .iter()
                .map(|action| action.samples.len())
                .sum::<usize>(),
            MAX_SEGMENT_SAMPLES + FRAME_SAMPLES
        );
        assert_eq!(actions.last().unwrap().start_sample, MAX_SEGMENT_SAMPLES);
    }

    #[test]
    fn channel_boundaries_are_independent() {
        let mut mic = Endpoint::new();
        let mut system = Endpoint::new();
        frames(&mut mic, true, 5);
        frames(&mut system, true, 5);
        assert!(frames(&mut mic, false, 40).last().unwrap().is_final);
        assert!(!frames(&mut system, true, 40).last().unwrap().is_final);
        assert!(system.finish().last().unwrap().is_final);
    }
}
