use std::{
    fs::File,
    io::{Read, Seek},
    path::Path,
};

use hypr_audio_utils::{PCM_BLOCK_FRAMES, PcmDescriptor, Source, decode_to_pcm, pcm_spec};
use hypr_loofah_protocol::{
    ApplicationError, CaptureChannel, CaptureEncoding, CaptureFormat, ErrorCode,
};
use hypr_vault_read::AudioLayout;
use sha2::{Digest, Sha256};

use crate::vault::error;

#[derive(Clone, Debug)]
pub struct NormalizedAppendAudio {
    pub frames: u64,
    pub bytes: u64,
    pub sha256: String,
    pub format: CaptureFormat,
}

/// Both paths must be new private files inside the immutable admission journal.
/// The decoder's actual output count (including its resampler delay trim), not
/// container duration metadata, defines the suffix's canonical frame offset.
pub fn normalize_baseline(
    mut source: File,
    expected_source_sha256: &str,
    layout: AudioLayout,
    decoded_path: &Path,
    normalized_path: &Path,
) -> Result<NormalizedAppendAudio, ApplicationError> {
    if file_hash(&mut source)? != expected_source_sha256 {
        return Err(conflict("Append recording source changed"));
    }
    source.rewind().map_err(io)?;
    let size = source.metadata().map_err(io)?.len();
    let decoder = hypr_audio_utils::source_from_reader(source, size).map_err(io)?;
    let channels = u16::from(decoder.channels()) as usize;
    if channels == 0 || channels > 2 || layout == AudioLayout::MicSystem && channels != 2 {
        return Err(conflict(
            "Append source does not match its authoritative channel layout",
        ));
    }
    let descriptor = decode_to_pcm(decoder, decoded_path, || false, |_| {}).map_err(io)?;
    let output_channels = if layout == AudioLayout::MicSystem {
        2
    } else {
        1
    };
    let mut output =
        hound::WavWriter::create(normalized_path, pcm_spec(output_channels)).map_err(io)?;
    let mut input = descriptor.reader().map_err(io)?;
    for start in (0..descriptor.frames).step_by(PCM_BLOCK_FRAMES) {
        let end = (start + PCM_BLOCK_FRAMES).min(descriptor.frames);
        let first = input.channel(0, start..end).map_err(io)?;
        let second = if channels == 2 {
            Some(input.channel(1, start..end).map_err(io)?)
        } else {
            None
        };
        for (index, first) in first.into_iter().enumerate() {
            if output_channels == 2 {
                output.write_sample(first).map_err(io)?;
                output
                    .write_sample(second.as_ref().unwrap()[index])
                    .map_err(io)?;
            } else {
                let sample = second
                    .as_ref()
                    .map_or(first, |second| first / 2.0 + second[index] / 2.0);
                output.write_sample(sample).map_err(io)?;
            }
        }
    }
    output.finalize().map_err(io)?;
    describe(
        normalized_path,
        CaptureFormat {
            encoding: CaptureEncoding::PcmF32Le,
            sample_rate: 16_000,
            channels: if output_channels == 2 {
                vec![CaptureChannel::Microphone, CaptureChannel::SystemAudio]
            } else {
                vec![CaptureChannel::Mixed]
            },
        },
    )
}

/// Concatenate only the admitted normalized prefix and verified suffix. The
/// exact retained prefix is reused, so neither a decoder upgrade nor recovery
/// changes the time at which the suffix starts.
pub fn concatenate(
    prefix_path: &Path,
    prefix: &NormalizedAppendAudio,
    suffix_path: &Path,
    suffix: &NormalizedAppendAudio,
    output_path: &Path,
) -> Result<NormalizedAppendAudio, ApplicationError> {
    if prefix.format != suffix.format {
        return Err(conflict(
            "Append suffix channel layout differs from its admission",
        ));
    }
    let frames = prefix
        .frames
        .checked_add(suffix.frames)
        .ok_or_else(|| conflict("Append recording is too large"))?;
    let pcm_bytes = frames
        .checked_mul(prefix.format.channels.len() as u64 * 4)
        .ok_or_else(|| conflict("Append recording is too large"))?;
    if pcm_bytes > u32::MAX as u64 - 128 {
        return Err(conflict("Append recording exceeds the WAV format limit"));
    }
    let mut output = hound::WavWriter::create(output_path, pcm_spec(prefix.format.channels.len()))
        .map_err(io)?;
    for (path, expected) in [(prefix_path, prefix), (suffix_path, suffix)] {
        let actual = describe(path, expected.format.clone())?;
        if actual.frames != expected.frames
            || actual.sha256 != expected.sha256
            || actual.bytes != expected.bytes
        {
            return Err(conflict("Retained append artifact changed"));
        }
        let mut source = hound::WavReader::open(path).map_err(io)?;
        for sample in source.samples::<f32>() {
            let sample = sample.map_err(io)?;
            if !sample.is_finite() {
                return Err(conflict("Append contains non-finite PCM"));
            }
            output.write_sample(sample).map_err(io)?;
        }
    }
    output.finalize().map_err(io)?;
    let result = describe(output_path, prefix.format.clone())?;
    if result.frames != frames {
        return Err(conflict("Append frame count changed"));
    }
    Ok(result)
}

fn describe(path: &Path, format: CaptureFormat) -> Result<NormalizedAppendAudio, ApplicationError> {
    hypr_loofah_protocol::validate_capture_format(&format).map_err(conflict)?;
    let descriptor = PcmDescriptor::open(path).map_err(io)?;
    if descriptor.channels != format.channels.len() {
        return Err(conflict("Append layout does not match PCM"));
    }
    let mut file = File::open(path).map_err(io)?;
    file.sync_all().map_err(io)?;
    let bytes = file.metadata().map_err(io)?.len();
    if bytes > u32::MAX as u64 {
        return Err(conflict("Append recording exceeds the WAV format limit"));
    }
    Ok(NormalizedAppendAudio {
        frames: descriptor.frames as u64,
        bytes,
        sha256: file_hash(&mut file)?,
        format,
    })
}

fn file_hash(file: &mut File) -> Result<String, ApplicationError> {
    file.rewind().map_err(io)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(io)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
fn conflict(message: &str) -> ApplicationError {
    error(ErrorCode::Conflict, message)
}
fn io(cause: impl std::fmt::Display) -> ApplicationError {
    error(
        ErrorCode::Unavailable,
        &format!("Append audio preparation failed: {cause}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wave(path: &Path, rate: u32, channels: u16, frames: usize) -> String {
        let mut spec = pcm_spec(channels as usize);
        spec.sample_rate = rate;
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        for index in 0..frames {
            for channel in 0..channels {
                writer
                    .write_sample(if channel == 0 {
                        index as f32 / 1000.0
                    } else {
                        -0.25
                    })
                    .unwrap();
            }
        }
        writer.finalize().unwrap();
        file_hash(&mut File::open(path).unwrap()).unwrap()
    }

    #[test]
    fn resampled_mixed_stereo_uses_actual_frames_and_downmixes_before_append() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original.wav");
        let hash = wave(&original, 44_100, 2, 442);
        let prefix_path = root.path().join("normalized.wav");
        let prefix = normalize_baseline(
            File::open(original).unwrap(),
            &hash,
            AudioLayout::Mixed,
            &root.path().join("decode.wav"),
            &prefix_path,
        )
        .unwrap();
        assert_eq!(prefix.frames, 161);
        assert_eq!(prefix.format.channels, [CaptureChannel::Mixed]);
        let suffix_path = root.path().join("suffix.wav");
        wave(&suffix_path, 16_000, 1, 19);
        let suffix = describe(&suffix_path, prefix.format.clone()).unwrap();
        let output = root.path().join("combined.wav");
        let combined = concatenate(&prefix_path, &prefix, &suffix_path, &suffix, &output).unwrap();
        assert_eq!(combined.frames, 180);
        let actual: Vec<_> = hound::WavReader::open(output)
            .unwrap()
            .samples::<f32>()
            .map(Result::unwrap)
            .collect();
        let expected: Vec<_> = [prefix_path, suffix_path]
            .into_iter()
            .flat_map(|path| {
                hound::WavReader::open(path)
                    .unwrap()
                    .samples::<f32>()
                    .map(Result::unwrap)
                    .collect::<Vec<_>>()
            })
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn mic_system_preserves_channels_and_rejects_changed_artifacts() {
        let root = tempfile::tempdir().unwrap();
        let original = root.path().join("original.wav");
        let hash = wave(&original, 16_000, 2, 17);
        let prefix_path = root.path().join("normalized.wav");
        let prefix = normalize_baseline(
            File::open(&original).unwrap(),
            &hash,
            AudioLayout::MicSystem,
            &root.path().join("decoded.wav"),
            &prefix_path,
        )
        .unwrap();
        assert_eq!(prefix.frames, 17);
        assert_eq!(prefix.format, CaptureFormat::stereo());
        assert!(
            normalize_baseline(
                File::open(&original).unwrap(),
                &"0".repeat(64),
                AudioLayout::MicSystem,
                &root.path().join("bad-decode.wav"),
                &root.path().join("bad.wav")
            )
            .is_err()
        );
        let suffix_path = root.path().join("suffix.wav");
        wave(&suffix_path, 16_000, 2, 19);
        let suffix = describe(&suffix_path, prefix.format.clone()).unwrap();
        wave(&suffix_path, 16_000, 2, 20);
        assert!(
            concatenate(
                &prefix_path,
                &prefix,
                &suffix_path,
                &suffix,
                &root.path().join("bad-combined.wav")
            )
            .is_err()
        );
    }
}
