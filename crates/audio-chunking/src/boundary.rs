use std::ops::Range;

pub fn pause_boundary(
    samples: &[f32],
    sample_start: usize,
    proposed: usize,
    earliest: usize,
    sample_rate: u32,
) -> usize {
    let rate = sample_rate as usize;
    let frame = rate.div_ceil(100).max(1);
    let quiet_min = rate.div_ceil(10).max(1);
    let end = proposed.saturating_sub(sample_start).min(samples.len());
    let begin = end.saturating_sub(rate * 4);
    let reference_start = end.saturating_sub(quiet_min);
    if reference_start == end {
        return proposed;
    }
    let energy = |audio: &[f32]| {
        audio
            .iter()
            .map(|sample| f64::from(*sample).powi(2))
            .sum::<f64>()
            / audio.len().max(1) as f64
    };
    let reference = energy(&samples[reference_start..end]);
    if !reference.is_finite() || reference <= 0.0 {
        return proposed;
    }
    let mut best = None;
    let mut consider = |score: u64, start: usize, end: usize| {
        let candidate = (sample_start + start + (end - start) / 2).max(earliest);
        if candidate > sample_start + end - quiet_min / 2 || candidate > proposed {
            return;
        }
        if best.is_none_or(|(previous, cut)| {
            score < previous || (score == previous && candidate > cut)
        }) {
            best = Some((score, candidate));
        }
    };
    let frames: Vec<_> = (begin..end)
        .step_by(frame)
        .map(|start| {
            let next = (start + frame).min(end);
            (start, next, energy(&samples[start..next]))
        })
        .collect();
    let mut plateau: Option<(u64, usize, usize)> = None;
    for window in frames.windows(quiet_min.div_ceil(frame)) {
        let start = window.first().unwrap().0;
        let end = window.last().unwrap().1;
        let candidate = if end - start >= quiet_min
            && window.iter().all(|(_, _, level)| *level < reference * 0.5)
        {
            let average = window
                .iter()
                .map(|(a, b, level)| level * (b - a) as f64)
                .sum::<f64>()
                / (end - start) as f64;
            // Ignore inaudible resampling noise when comparing otherwise equivalent pauses.
            let score = (average / reference * 1_000_000.0).round() as u64;
            Some((score, start, end))
        } else {
            None
        };
        if let Some((score, start, end)) = candidate
            && let Some((previous, _, previous_end)) = &mut plateau
            && *previous == score
            && start <= *previous_end
        {
            *previous_end = end;
            continue;
        }
        if let Some((score, start, end)) = plateau.take() {
            consider(score, start, end);
        }
        plateau = candidate;
    }
    if let Some((score, start, end)) = plateau {
        consider(score, start, end);
    }

    best.map_or(proposed, |(_, cut)| cut)
}

pub fn split_audio_range<E>(
    range: Range<usize>,
    max_samples: usize,
    sample_rate: u32,
    mut read: impl FnMut(Range<usize>) -> Result<Vec<f32>, E>,
) -> Result<Vec<Range<usize>>, E> {
    assert!(max_samples > 0);
    let minimum = (sample_rate as usize / 2)
        .max(1)
        .min(max_samples / 2)
        .max(1);
    let mut start = range.start;
    let mut output = Vec::new();
    while range.end.saturating_sub(start) > max_samples {
        let count = (range.end - start).div_ceil(max_samples);
        let earliest = (start + minimum).max(range.end.saturating_sub((count - 1) * max_samples));
        let proposed = (start + max_samples).min(range.end - minimum);
        let window_start = start.max(proposed.saturating_sub(4 * sample_rate as usize));
        let window = read(window_start..proposed)?;
        let cut = pause_boundary(&window, window_start, proposed, earliest, sample_rate);
        output.push(start..cut);
        start = cut;
    }
    if start < range.end {
        output.push(start..range.end);
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    const RATE: u32 = 16000;
    const CAP: usize = 25 * RATE as usize;

    fn split(audio: &[f32], range: Range<usize>) -> Vec<Range<usize>> {
        split_audio_range(range, CAP, RATE, |window| {
            assert!(window.len() <= 4 * RATE as usize);
            Ok::<_, std::convert::Infallible>(audio[window].to_vec())
        })
        .unwrap()
    }

    #[test]
    fn selects_midpoint_of_quietest_stretch_in_preceding_four_seconds() {
        let mut audio = vec![1.0; 40 * RATE as usize];
        audio[22 * 16000..23 * 16000].fill(0.1);
        audio[23 * 16000..24 * 16000].fill(0.2);
        let ranges = split(&audio, 0..audio.len());
        assert_eq!(
            ranges,
            vec![0..22 * 16000 + 8000, 22 * 16000 + 8000..audio.len()]
        );
    }

    #[test]
    fn inaudible_noise_does_not_prefer_a_much_earlier_pause() {
        let mut audio = vec![1.0; 40 * 16000];
        audio[22 * 16000..22 * 16000 + 3200].fill(0.0);
        audio[24 * 16000..24 * 16000 + 3200].fill(1e-6);
        assert_eq!(split(&audio, 0..audio.len())[0].end, 24 * 16000 + 1600);
    }

    #[test]
    fn short_dips_and_weak_energy_changes_use_the_limit() {
        for (length, amplitude) in [(80, 0.0), (200, 0.8)] {
            let mut audio = vec![1.0; 40 * 16000];
            audio[23 * 16000..23 * 16000 + length * 16].fill(amplitude);
            assert_eq!(split(&audio, 0..audio.len())[0], 0..CAP);
        }
    }

    #[test]
    fn boundaries_cannot_add_chunks_even_when_a_pause_is_quieter() {
        let mut audio = vec![1.0; 50 * 16000];
        audio[22 * 16000..23 * 16000].fill(0.0);
        assert_eq!(split(&audio, 0..audio.len()), vec![0..CAP, CAP..2 * CAP]);
    }

    #[test]
    fn uses_the_valid_part_of_a_pause_when_its_midpoint_would_add_a_chunk() {
        let mut audio = vec![1.0; 49 * 16000];
        audio[22 * 16000..24 * 16000 + 8000].fill(0.0);
        assert_eq!(split(&audio, 0..audio.len())[0].end, 24 * 16000);
    }

    #[test]
    fn covers_short_tails_once_and_respects_caps_and_absolute_offsets() {
        for tail_ms in [1, 99, 100, 499, 500, 501, 999] {
            let end = 123 + 2 * CAP + tail_ms * 16;
            let audio = vec![1.0; end];
            let ranges = split(&audio, 123..end);
            assert_eq!(ranges.len(), 3);
            let mut cursor = 123;
            for range in &ranges {
                assert_eq!(range.start, cursor);
                assert!(range.len() >= 8000 && range.len() <= CAP);
                cursor = range.end;
            }
            assert_eq!(cursor, end);
        }
    }

    #[test]
    fn keeps_short_natural_spans_and_does_not_read_unneeded_audio() {
        let result = split_audio_range(700..2300, CAP, RATE, |_| -> Result<Vec<f32>, ()> {
            panic!("no forced cut")
        })
        .unwrap();
        assert_eq!(result, vec![700..2300]);
    }
}
