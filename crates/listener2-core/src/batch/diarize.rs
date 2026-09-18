use std::collections::BTreeMap;
use std::sync::Arc;

use hypr_audio_utils::Source;
use hypr_transcribe_core::{TARGET_SAMPLE_RATE, split_resampled_channels};
use hypr_transcribe_soniqo::diarize::DiarizeSegment;
use owhisper_interface::batch_stream::BatchStreamEvent;
use owhisper_interface::stream::StreamResponse;

pub(super) type ChannelSegments = BTreeMap<i32, Vec<DiarizeSegment>>;

pub(super) trait Diarizer: Send + Sync + 'static {
    fn is_ready(&self) -> bool;
    fn diarize(&self, samples: &[f32], sample_rate_hz: u32) -> Result<Vec<DiarizeSegment>, String>;
}

pub(super) struct SoniqoDiarizer;

impl Diarizer for SoniqoDiarizer {
    fn is_ready(&self) -> bool {
        hypr_transcribe_soniqo::diarize::is_ready()
    }

    fn diarize(&self, samples: &[f32], sample_rate_hz: u32) -> Result<Vec<DiarizeSegment>, String> {
        hypr_transcribe_soniqo::diarize::diarize_samples(samples, sample_rate_hz)
            .map_err(|error| error.to_string())
    }
}

// Runs alongside ASR; every stamp point awaits the same task once and then
// reuses the memoized result, including failures so a retry keeps its recording.
#[derive(Clone)]
pub(super) struct SharedDiarization(Arc<tokio::sync::Mutex<DiarizationState>>);

enum DiarizationState {
    Pending(tokio::task::JoinHandle<Result<ChannelSegments, String>>),
    Ready(Result<Arc<ChannelSegments>, String>),
}

impl SharedDiarization {
    pub(super) fn for_file(diarizer: Arc<dyn Diarizer>, file_path: String) -> Self {
        if !diarizer.is_ready() {
            return Self::disabled();
        }

        let handle = tokio::task::spawn_blocking(move || file_diarization(&*diarizer, &file_path));
        Self(Arc::new(tokio::sync::Mutex::new(
            DiarizationState::Pending(handle),
        )))
    }

    pub(super) fn disabled() -> Self {
        Self(Arc::new(tokio::sync::Mutex::new(DiarizationState::Ready(
            Ok(Arc::new(ChannelSegments::new())),
        ))))
    }

    pub(super) async fn segments(&self) -> Result<Arc<ChannelSegments>, crate::BatchFailure> {
        let mut state = self.0.lock().await;

        if let DiarizationState::Pending(handle) = &mut *state {
            let segments = match handle.await {
                Ok(segments) => segments,
                Err(error) => {
                    tracing::warn!(error = %format!("{error:?}"), "diarization_task_join_failed");
                    Err(format!("Speaker detection task failed: {error}"))
                }
            };
            *state = DiarizationState::Ready(segments.map(Arc::new));
        }

        match &*state {
            DiarizationState::Ready(segments) => segments
                .clone()
                .map_err(|message| crate::BatchFailure::DiarizationFailed { message }),
            DiarizationState::Pending(_) => unreachable!(),
        }
    }
}

fn file_diarization(diarizer: &dyn Diarizer, file_path: &str) -> Result<ChannelSegments, String> {
    let source = match hypr_audio_utils::source_from_path(file_path) {
        Ok(source) => source,
        Err(error) => {
            tracing::warn!(error = %error, "diarization_audio_decode_failed");
            return Err(format!(
                "Could not read audio for speaker detection: {error}"
            ));
        }
    };
    let channel_count = u16::from(source.channels()).max(1) as usize;

    let samples = match hypr_audio_utils::resample_audio(source, TARGET_SAMPLE_RATE) {
        Ok(samples) => samples,
        Err(error) => {
            tracing::warn!(error = %error, "diarization_audio_resample_failed");
            return Err(format!(
                "Could not resample audio for speaker detection: {error}"
            ));
        }
    };

    let channels = split_resampled_channels(&samples, channel_count);
    diarize_channels(diarizer, &channels)
}

fn diarize_channels(
    diarizer: &dyn Diarizer,
    channels: &[Vec<f32>],
) -> Result<ChannelSegments, String> {
    let mut segments = ChannelSegments::new();

    for channel_index in channels_to_diarize(channels.len()) {
        match diarizer.diarize(&channels[channel_index], TARGET_SAMPLE_RATE) {
            Ok(channel_segments) if !channel_segments.is_empty() => {
                tracing::info!(
                    channel.index = channel_index,
                    segment.count = channel_segments.len(),
                    "diarization_channel_completed"
                );
                segments.insert(channel_index as i32, channel_segments);
            }
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(
                    channel.index = channel_index,
                    error = %error,
                    "diarization_channel_failed"
                );
                return Err(error);
            }
        }
    }

    Ok(segments)
}

fn channels_to_diarize(channel_count: usize) -> Vec<usize> {
    match channel_count {
        0 => Vec::new(),
        // Recordings follow the mic=0/system=1 convention; leaving the mic
        // channel alone protects the "You" channel from false speaker splits.
        2 => vec![1],
        count => (0..count).collect(),
    }
}

pub(super) fn stamp_batch_response(
    response: &mut owhisper_interface::batch::Response,
    segments: &ChannelSegments,
) {
    if segments.is_empty() {
        return;
    }

    for channel in &mut response.results.channels {
        for alternative in &mut channel.alternatives {
            for word in &mut alternative.words {
                if word.speaker.is_some() {
                    continue;
                }

                word.speaker = segments
                    .get(&word.channel)
                    .and_then(|segments| speaker_at(segments, midpoint_ms(word.start, word.end)))
                    .and_then(|index| usize::try_from(index).ok());
            }
        }
    }
}

pub(super) fn stamp_stream_event(event: &mut BatchStreamEvent, segments: &ChannelSegments) {
    if segments.is_empty() {
        return;
    }

    match event {
        BatchStreamEvent::Segment { response, .. } => {
            let StreamResponse::TranscriptResponse {
                channel,
                channel_index,
                ..
            } = response
            else {
                return;
            };

            let channel_id = channel_index.first().copied().unwrap_or(0);
            let Some(channel_segments) = segments.get(&channel_id) else {
                return;
            };

            for alternative in &mut channel.alternatives {
                for word in &mut alternative.words {
                    if word.speaker.is_some() {
                        continue;
                    }

                    word.speaker = speaker_at(channel_segments, midpoint_ms(word.start, word.end));
                }
            }
        }
        BatchStreamEvent::Result { response } => stamp_batch_response(response, segments),
        BatchStreamEvent::Progress { .. }
        | BatchStreamEvent::Terminal { .. }
        | BatchStreamEvent::Error { .. } => {}
    }
}

fn midpoint_ms(start_secs: f64, end_secs: f64) -> i64 {
    ((start_secs + end_secs) * 500.0).round() as i64
}

fn speaker_at(segments: &[DiarizeSegment], midpoint_ms: i64) -> Option<i32> {
    let mut nearest: Option<(i64, i32)> = None;

    for segment in segments {
        if midpoint_ms >= segment.start_ms && midpoint_ms < segment.end_ms {
            return Some(segment.speaker_index);
        }

        let distance = if midpoint_ms < segment.start_ms {
            segment.start_ms - midpoint_ms
        } else {
            midpoint_ms - segment.end_ms
        };

        if nearest.is_none_or(|(best, _)| distance < best) {
            nearest = Some((distance, segment.speaker_index));
        }
    }

    nearest.map(|(_, speaker_index)| speaker_index)
}

#[cfg(test)]
mod tests {
    use owhisper_interface::stream;

    use super::*;

    struct FakeDiarizer {
        ready: bool,
        segments: Vec<DiarizeSegment>,
        diarized_first_samples: std::sync::Mutex<Vec<f32>>,
    }

    impl FakeDiarizer {
        fn ready_with(segments: Vec<DiarizeSegment>) -> Self {
            Self {
                ready: true,
                segments,
                diarized_first_samples: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn not_ready() -> Self {
            Self {
                ready: false,
                segments: Vec::new(),
                diarized_first_samples: std::sync::Mutex::new(Vec::new()),
            }
        }
    }

    impl Diarizer for FakeDiarizer {
        fn is_ready(&self) -> bool {
            self.ready
        }

        fn diarize(
            &self,
            samples: &[f32],
            _sample_rate_hz: u32,
        ) -> Result<Vec<DiarizeSegment>, String> {
            self.diarized_first_samples
                .lock()
                .unwrap()
                .push(samples.first().copied().unwrap_or_default());
            Ok(self.segments.clone())
        }
    }

    fn segment(start_ms: i64, end_ms: i64, speaker_index: i32) -> DiarizeSegment {
        DiarizeSegment {
            start_ms,
            end_ms,
            speaker_index,
        }
    }

    fn batch_word(start: f64, end: f64, channel: i32) -> owhisper_interface::batch::Word {
        owhisper_interface::batch::Word {
            word: "hello".to_string(),
            start,
            end,
            confidence: 0.9,
            channel,
            speaker: None,
            punctuated_word: Some("hello".to_string()),
        }
    }

    fn batch_response(
        channels: Vec<Vec<owhisper_interface::batch::Word>>,
    ) -> owhisper_interface::batch::Response {
        owhisper_interface::batch::Response {
            metadata: serde_json::json!({}),
            results: owhisper_interface::batch::Results {
                channels: channels
                    .into_iter()
                    .map(|words| owhisper_interface::batch::Channel {
                        alternatives: vec![owhisper_interface::batch::Alternatives {
                            transcript: "hello".to_string(),
                            confidence: 0.9,
                            words,
                        }],
                    })
                    .collect(),
            },
        }
    }

    fn segment_event(words: Vec<stream::Word>, channel_index: Vec<i32>) -> BatchStreamEvent {
        BatchStreamEvent::Segment {
            response: stream::StreamResponse::TranscriptResponse {
                start: 0.0,
                duration: 10.0,
                is_final: true,
                speech_final: true,
                from_finalize: false,
                channel: stream::Channel {
                    alternatives: vec![stream::Alternatives {
                        transcript: "hello".to_string(),
                        words,
                        confidence: 0.9,
                        languages: Vec::new(),
                    }],
                },
                metadata: stream::Metadata {
                    request_id: "r".to_string(),
                    model_info: stream::ModelInfo {
                        name: String::new(),
                        version: String::new(),
                        arch: String::new(),
                    },
                    model_uuid: "m".to_string(),
                    extra: None,
                },
                channel_index,
            },
            percentage: 0.5,
        }
    }

    fn stream_word(start: f64, end: f64, speaker: Option<i32>) -> stream::Word {
        stream::Word {
            word: "hello".to_string(),
            start,
            end,
            confidence: 0.9,
            speaker,
            punctuated_word: Some("hello".to_string()),
            language: None,
        }
    }

    #[test]
    fn stamps_by_overlap_at_boundaries() {
        let segments =
            ChannelSegments::from([(0, vec![segment(0, 1000, 0), segment(1000, 2000, 1)])]);

        let mut response = batch_response(vec![vec![
            // spans the boundary, midpoint 950ms -> first segment
            batch_word(0.8, 1.1, 0),
            // spans the boundary, midpoint 1100ms -> second segment
            batch_word(0.9, 1.3, 0),
            // midpoint exactly on the boundary -> the segment starting there
            batch_word(0.9, 1.1, 0),
        ]]);
        stamp_batch_response(&mut response, &segments);

        let words = &response.results.channels[0].alternatives[0].words;
        assert_eq!(words[0].speaker, Some(0));
        assert_eq!(words[1].speaker, Some(1));
        assert_eq!(words[2].speaker, Some(1));

        let mut event = segment_event(
            vec![stream_word(0.8, 1.1, None), stream_word(0.9, 1.3, None)],
            vec![0, 1],
        );
        stamp_stream_event(&mut event, &segments);

        let BatchStreamEvent::Segment {
            response: stream::StreamResponse::TranscriptResponse { channel, .. },
            ..
        } = &event
        else {
            panic!("expected transcript segment");
        };
        assert_eq!(channel.alternatives[0].words[0].speaker, Some(0));
        assert_eq!(channel.alternatives[0].words[1].speaker, Some(1));
    }

    #[test]
    fn gap_words_get_nearest_segment_on_diarized_channel() {
        let segments =
            ChannelSegments::from([(0, vec![segment(0, 1000, 0), segment(3000, 4000, 1)])]);

        let mut response = batch_response(vec![vec![
            // midpoint 1400ms: 400ms after first segment, 1600ms before second
            batch_word(1.2, 1.6, 0),
            // midpoint 2600ms: closer to the second segment
            batch_word(2.4, 2.8, 0),
            // midpoint 5000ms: after the last segment
            batch_word(4.8, 5.2, 0),
        ]]);
        stamp_batch_response(&mut response, &segments);

        let words = &response.results.channels[0].alternatives[0].words;
        assert_eq!(words[0].speaker, Some(0));
        assert_eq!(words[1].speaker, Some(1));
        assert_eq!(words[2].speaker, Some(1));
        assert!(words.iter().all(|word| word.speaker.is_some()));
    }

    #[tokio::test]
    async fn decode_failure_is_preserved_for_every_consumer() {
        let diarization = SharedDiarization::for_file(
            Arc::new(FakeDiarizer::ready_with(Vec::new())),
            "/nonexistent/audio.wav".to_string(),
        );
        for _ in 0..2 {
            let error = diarization.segments().await.unwrap_err();
            assert_eq!(error.code(), crate::BatchErrorCode::DiarizationFailed);
            assert!(error.to_string().contains("available to retry"));
        }
    }

    #[test]
    fn inference_failure_does_not_become_an_empty_success() {
        struct FailingDiarizer;
        impl Diarizer for FailingDiarizer {
            fn is_ready(&self) -> bool {
                true
            }
            fn diarize(&self, _: &[f32], _: u32) -> Result<Vec<DiarizeSegment>, String> {
                Err("model inference failed".to_string())
            }
        }
        assert_eq!(
            diarize_channels(&FailingDiarizer, &[vec![0.0; 16000]]).unwrap_err(),
            "model inference failed"
        );
    }

    #[tokio::test]
    async fn model_missing_yields_todays_output() {
        let diarization = SharedDiarization::for_file(
            Arc::new(FakeDiarizer::not_ready()),
            "/nonexistent/audio.wav".to_string(),
        );
        let segments = diarization.segments().await.unwrap();
        assert!(segments.is_empty());

        let original = batch_response(vec![vec![batch_word(0.0, 1.0, 0)]]);
        let mut stamped = original.clone();
        stamp_batch_response(&mut stamped, &segments);
        assert_eq!(stamped, original);
        assert_eq!(
            serde_json::to_string(&stamped).unwrap(),
            serde_json::to_string(&original).unwrap()
        );

        let original_event = segment_event(vec![stream_word(0.0, 1.0, None)], vec![0, 1]);
        let mut stamped_event = original_event.clone();
        stamp_stream_event(&mut stamped_event, &segments);
        assert_eq!(stamped_event, original_event);
    }

    #[test]
    fn existing_indexes_not_overwritten() {
        let segments = ChannelSegments::from([(0, vec![segment(0, 2000, 1)])]);

        let mut word = batch_word(0.0, 1.0, 0);
        word.speaker = Some(7);
        let mut response = batch_response(vec![vec![word]]);
        stamp_batch_response(&mut response, &segments);
        assert_eq!(
            response.results.channels[0].alternatives[0].words[0].speaker,
            Some(7)
        );

        let mut event = segment_event(vec![stream_word(0.0, 1.0, Some(7))], vec![0, 1]);
        stamp_stream_event(&mut event, &segments);
        let BatchStreamEvent::Segment {
            response: stream::StreamResponse::TranscriptResponse { channel, .. },
            ..
        } = &event
        else {
            panic!("expected transcript segment");
        };
        assert_eq!(channel.alternatives[0].words[0].speaker, Some(7));
    }

    #[test]
    fn two_channel_audio_diarizes_only_system_channel() {
        let diarizer = FakeDiarizer::ready_with(vec![segment(0, 2000, 0)]);
        let channels = vec![vec![1.0f32; 16_000], vec![2.0f32; 16_000]];

        let segments = diarize_channels(&diarizer, &channels).unwrap();

        assert_eq!(segments.keys().copied().collect::<Vec<_>>(), vec![1]);
        assert_eq!(
            diarizer.diarized_first_samples.lock().unwrap().clone(),
            vec![2.0]
        );

        let mut response = batch_response(vec![
            vec![batch_word(0.0, 1.0, 0)],
            vec![batch_word(0.0, 1.0, 1)],
        ]);
        stamp_batch_response(&mut response, &segments);

        assert_eq!(
            response.results.channels[0].alternatives[0].words[0].speaker,
            None
        );
        assert_eq!(
            response.results.channels[1].alternatives[0].words[0].speaker,
            Some(0)
        );
    }

    #[test]
    fn mono_audio_diarizes_its_only_channel() {
        assert_eq!(channels_to_diarize(1), vec![0]);
        assert_eq!(channels_to_diarize(2), vec![1]);
        assert_eq!(channels_to_diarize(3), vec![0, 1, 2]);
        assert!(channels_to_diarize(0).is_empty());
    }

    #[test]
    fn negative_speaker_indexes_are_not_stamped_on_batch_words() {
        let segments = ChannelSegments::from([(0, vec![segment(0, 2000, -1)])]);

        let mut response = batch_response(vec![vec![batch_word(0.0, 1.0, 0)]]);
        stamp_batch_response(&mut response, &segments);

        assert_eq!(
            response.results.channels[0].alternatives[0].words[0].speaker,
            None
        );
    }
}
