/// Smoothed display level on a -60 dBFS to 0 dBFS scale; does not modify audio.
pub fn smoothed_audio_level(previous: f32, samples: &[f32]) -> f32 {
    let (sum_squares, count) = samples
        .iter()
        .filter(|sample| sample.is_finite())
        .fold((0.0_f32, 0_usize), |(sum, count), sample| {
            (sum + sample * sample, count + 1)
        });
    let level = if count == 0 || sum_squares == 0.0 {
        0.0
    } else {
        let rms = (sum_squares / count as f32).sqrt();
        ((20.0 * rms.log10() + 60.0) / 60.0).clamp(0.0, 1.0)
    };
    const SMOOTHING_ALPHA: f32 = 0.7;
    (1.0 - SMOOTHING_ALPHA) * previous + SMOOTHING_ALPHA * level
}

#[cfg(test)]
mod tests {
    use super::smoothed_audio_level;

    #[test]
    fn quiet_speech_uses_the_same_decibel_scale_as_loud_speech() {
        for (sample, expected) in [
            (0.001, 0.0),
            (0.01, 1.0 / 3.0),
            (0.1, 2.0 / 3.0),
            (1.0, 1.0),
        ] {
            let level = smoothed_audio_level(0.0, &[sample, -sample]);
            assert!((level - expected * 0.7).abs() < 0.00001);
        }
    }

    #[test]
    fn silence_stays_flat_and_speech_decays_smoothly() {
        assert_eq!(smoothed_audio_level(0.0, &[0.0; 16]), 0.0);
        let loud = smoothed_audio_level(0.0, &[1.0; 16]);
        let quiet = smoothed_audio_level(loud, &[0.0; 16]);
        assert!((quiet - 0.21).abs() < 0.00001);
        assert!(smoothed_audio_level(quiet, &[]) < quiet);
    }

    #[test]
    fn invalid_samples_do_not_poison_the_meter() {
        assert_eq!(smoothed_audio_level(0.0, &[f32::NAN, f32::INFINITY]), 0.0);
        assert_eq!(
            smoothed_audio_level(0.0, &[0.1, f32::NAN, f32::NEG_INFINITY]),
            smoothed_audio_level(0.0, &[0.1]),
        );
        assert!(smoothed_audio_level(1.0, &[2.0, -2.0]) <= 1.0);
    }
}
