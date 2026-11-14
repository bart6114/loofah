use super::TARGET_SAMPLE_RATE;
use hypr_audio_chunking::AudioChunk;

pub(super) fn pack_ranges<E>(
    frame_count: usize,
    chunks: &[AudioChunk],
    gap: Option<usize>,
    mut read: impl FnMut(std::ops::Range<usize>) -> Result<Vec<f32>, E>,
) -> Result<Vec<AudioChunk>, E> {
    let cap = 25 * TARGET_SAMPLE_RATE as usize;
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for chunk in chunks {
        let mut start = chunk.sample_start.min(frame_count);
        let end = chunk.sample_end.min(frame_count);
        if let Some(last) = ranges.last_mut() {
            start = start.max(last.end);
            if start < end
                && end - last.start <= cap
                && gap.is_none_or(|gap| start.saturating_sub(last.end) <= gap)
            {
                last.end = end;
                continue;
            }
        }
        ranges.extend(hypr_audio_chunking::split_audio_range(
            start..end,
            cap,
            TARGET_SAMPLE_RATE,
            &mut read,
        )?);
    }
    Ok(ranges
        .into_iter()
        .map(|range| AudioChunk {
            sample_start: range.start,
            sample_end: range.end,
            samples: Vec::new(),
        })
        .collect())
}

pub(super) fn gap_override() -> Option<usize> {
    match std::env::var("LOOFAH_WHISPER_PACK_GAP").as_deref() {
        Ok("1") => Some(TARGET_SAMPLE_RATE as usize),
        Ok("2") => Some(2 * TARGET_SAMPLE_RATE as usize),
        _ => None,
    }
}

#[cfg(test)]
fn pack(samples: &[f32], chunks: &[AudioChunk]) -> Vec<AudioChunk> {
    pack_ranges(samples.len(), chunks, None, |range| {
        Ok::<_, std::convert::Infallible>(samples[range].to_vec())
    })
    .unwrap()
    .into_iter()
    .map(|mut chunk| {
        chunk.samples = samples[chunk.sample_start..chunk.sample_end].to_vec();
        chunk
    })
    .collect()
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
    fn preserves_original_gaps_without_concatenating_separate_sample_buffers() {
        let audio: Vec<_> = (0..400_100).map(|i| i as f32).collect();
        let packed = pack(&audio, &[chunk(100, 1000), chunk(2000, 4000)]);
        assert_eq!(packed.len(), 1);
        assert_eq!(packed[0].samples, audio[100..4000]);
    }

    #[test]
    fn rebalances_short_tails_without_losing_or_duplicating_audio() {
        let audio = vec![1.0; 800_123];
        let packed = pack(&audio, &[chunk(0, audio.len())]);
        assert_eq!(
            packed.iter().map(|c| c.samples.len()).collect::<Vec<_>>(),
            [400_000, 392_123, 8000]
        );
        let reconstructed: Vec<_> = packed
            .iter()
            .flat_map(|c| c.samples.iter().copied())
            .collect();
        assert_eq!(reconstructed, audio);
        assert!(pack(&[], &[]).is_empty());
    }

    #[test]
    fn retains_existing_boundaries_when_the_next_span_exceeds_the_cap() {
        let audio = vec![1.0; 500_000];
        let packed = pack(&audio, &[chunk(0, 200_000), chunk(220_000, 500_000)]);
        assert_eq!(packed.len(), 2);
        assert_eq!(packed[0].sample_end, 200_000);
        assert_eq!(packed[1].sample_start, 220_000);
        assert_eq!(
            pack(&audio, &[chunk(0, 200_000), chunk(200_000, 200_100)]).len(),
            1
        );
    }

    #[test]
    fn packing_uses_the_shared_pause_selector() {
        let mut audio = vec![1.0; 600_000];
        audio[350_000..354_800].fill(0.0);
        let packed = pack(&audio, &[chunk(0, audio.len())]);
        assert_eq!(packed[0].sample_end, 352_400);
        assert_eq!(packed[1].sample_start, 352_400);
    }
}
