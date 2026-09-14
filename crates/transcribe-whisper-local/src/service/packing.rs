use super::TARGET_SAMPLE_RATE;
use hypr_audio_chunking::AudioChunk;

pub(super) fn pack(samples: &[f32], chunks: &[AudioChunk], gap: Option<usize>) -> Vec<AudioChunk> {
    let max_span = 25 * TARGET_SAMPLE_RATE as usize;
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for chunk in chunks {
        let mut start = chunk.sample_start.min(samples.len());
        let end = chunk.sample_end.min(samples.len());
        while start < end {
            if let Some(last) = ranges.last_mut() {
                start = start.max(last.end);
                if start >= end {
                    break;
                }
                if end - last.start <= max_span
                    && gap.is_none_or(|gap| start.saturating_sub(last.end) <= gap)
                {
                    last.end = end;
                    break;
                }
            }
            let next = end.min(start + max_span);
            ranges.push(start..next);
            start = next;
        }
    }
    ranges
        .into_iter()
        .map(|range| AudioChunk {
            sample_start: range.start,
            sample_end: range.end,
            samples: samples[range].to_vec(),
        })
        .collect()
}

pub(super) fn gap_override() -> Option<usize> {
    match std::env::var("LOOFAH_WHISPER_PACK_GAP").as_deref() {
        Ok("1") => Some(TARGET_SAMPLE_RATE as usize),
        Ok("2") => Some(2 * TARGET_SAMPLE_RATE as usize),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn chunk(start: usize, end: usize) -> AudioChunk {
        AudioChunk {
            sample_start: start,
            sample_end: end,
            samples: vec![],
        }
    }
    #[test]
    fn preserves_original_gaps_and_offsets() {
        let audio: Vec<_> = (0..400_100).map(|i| i as f32).collect();
        let packed = pack(&audio, &[chunk(100, 1000), chunk(2000, 4000)], None);
        assert_eq!(packed.len(), 1);
        assert_eq!(packed[0].samples, audio[100..4000]);
        assert_eq!(
            pack(&audio, &[chunk(100, 1000), chunk(2000, 4000)], Some(500)).len(),
            2
        );
    }
    #[test]
    fn splits_continuous_speech_without_losing_partial_tail() {
        let audio = vec![1.0; 800_123];
        let packed = pack(&audio, &[chunk(0, audio.len())], None);
        assert_eq!(
            packed.iter().map(|c| c.samples.len()).collect::<Vec<_>>(),
            [400_000, 400_000, 123]
        );
        assert_eq!(packed[2].sample_start, 800_000);
        assert!(pack(&[], &[], None).is_empty());
    }
    #[test]
    fn prefers_existing_boundary_at_limit() {
        let audio = vec![1.0; 500_000];
        let packed = pack(&audio, &[chunk(0, 200_000), chunk(220_000, 500_000)], None);
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0].sample_end, 200_000);
        assert_eq!(packed[1].sample_start, 220_000);
    }
}
