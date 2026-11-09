use std::{
    fs::File,
    io::{BufWriter, Read, Seek, Write},
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
    File::create_new(decoded_path).map_err(io)?;
    let descriptor = decode_to_pcm(decoder, decoded_path, || false, |_| {}).map_err(io)?;
    let output_channels = if layout == AudioLayout::MicSystem {
        2
    } else {
        1
    };
    let mut output_file = File::create_new(normalized_path).map_err(io)?;
    write_pcm(&mut output_file, output_channels, |output| {
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
        Ok(())
    })?;
    // Windows requires a writable handle for FlushFileBuffers. Retained source
    // verification stays read-only; only this original output owner flushes it.
    output_file.sync_all().map_err(io)?;
    drop(output_file);
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

const PCM_OUTPUT_BUFFER_BYTES: usize = 64 * 1024;

fn write_pcm<W: Write + Seek>(
    output: &mut W,
    channels: usize,
    samples: impl FnOnce(&mut hound::WavWriter<&mut BufWriter<&mut W>>) -> Result<(), ApplicationError>,
) -> Result<(), ApplicationError> {
    let mut buffered = BufWriter::with_capacity(PCM_OUTPUT_BUFFER_BYTES, output);
    let mut wave = hound::WavWriter::new(&mut buffered, pcm_spec(channels)).map_err(io)?;
    samples(&mut wave)?;
    wave.finalize().map_err(io)?;
    buffered.flush().map_err(io)
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
    let mut output_file = File::create_new(output_path).map_err(io)?;
    let mut output =
        hound::WavWriter::new(&mut output_file, pcm_spec(prefix.format.channels.len()))
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
    // Windows requires a writable handle for FlushFileBuffers. Retained source
    // verification stays read-only; only this original output owner flushes it.
    output_file.sync_all().map_err(io)?;
    drop(output_file);
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

pub(crate) fn verify_file_sha256(path: &Path, expected: &str) -> Result<(), ApplicationError> {
    let metadata = path.symlink_metadata().map_err(io)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(conflict("Append input is no longer a regular file"));
    }
    if file_hash(&mut File::open(path).map_err(io)?)? != expected {
        return Err(conflict("Retained append inference input changed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct CountedOutput {
        bytes: std::io::Cursor<Vec<u8>>,
        writes: usize,
        maximum_write: usize,
        flushes: usize,
        fail_write: bool,
        fail_flush: bool,
    }
    impl Write for CountedOutput {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.writes += 1;
            self.maximum_write = self.maximum_write.max(bytes.len());
            if self.fail_write {
                return Err(std::io::Error::other("injected output write failure"));
            }
            self.bytes.write(bytes)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            self.flushes += 1;
            if self.fail_flush {
                return Err(std::io::Error::other("injected output flush failure"));
            }
            Ok(())
        }
    }
    impl Seek for CountedOutput {
        fn seek(&mut self, position: std::io::SeekFrom) -> std::io::Result<u64> {
            self.bytes.seek(position)
        }
    }

    #[test]
    fn buffered_pcm_output_preserves_exact_wav_bytes_with_bounded_write_calls() {
        let values: Vec<_> = (0..40_002)
            .map(|index| match index % 6 {
                0 => -0.0_f32,
                1 => f32::from_bits(1),
                2 => f32::MIN_POSITIVE,
                3 => f32::MAX,
                4 => -0.25,
                _ => 0.125,
            })
            .collect();
        let mut original = std::io::Cursor::new(Vec::new());
        let mut legacy = hound::WavWriter::new(&mut original, pcm_spec(2)).unwrap();
        for &sample in &values {
            legacy.write_sample(sample).unwrap();
        }
        legacy.finalize().unwrap();
        let mut output = CountedOutput::default();
        write_pcm(&mut output, 2, |writer| {
            for &sample in &values {
                writer.write_sample(sample).map_err(io)?;
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(output.bytes.get_ref(), original.get_ref());
        assert!(
            output.writes <= (values.len() * 4).div_ceil(PCM_OUTPUT_BUFFER_BYTES) + 4,
            "{} writes",
            output.writes
        );
        assert!(output.maximum_write <= PCM_OUTPUT_BUFFER_BYTES);
        assert!(output.flushes >= 1);
        let samples: Vec<_> =
            hound::WavReader::new(std::io::Cursor::new(output.bytes.into_inner()))
                .unwrap()
                .samples::<f32>()
                .map(|sample| sample.unwrap().to_bits())
                .collect();
        assert_eq!(
            samples,
            values.into_iter().map(f32::to_bits).collect::<Vec<_>>()
        );
    }

    #[test]
    fn buffered_pcm_output_propagates_underlying_write_and_final_flush_errors() {
        for fail_write in [true, false] {
            let mut output = CountedOutput {
                fail_write,
                fail_flush: !fail_write,
                ..Default::default()
            };
            let failure = write_pcm(&mut output, 1, |writer| {
                for _ in 0..20_000 {
                    writer.write_sample(0.25_f32).map_err(io)?;
                }
                Ok(())
            })
            .unwrap_err();
            assert_eq!(failure.code, ErrorCode::Unavailable);
            assert!(output.writes > 0);
            if !fail_write {
                assert!(output.flushes > 0);
            }
        }
    }

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
    fn concatenation_reads_immutable_inputs_and_syncs_only_its_owned_output() {
        let root = tempfile::tempdir().unwrap();
        let prefix_path = root.path().join("prefix.wav");
        let suffix_path = root.path().join("suffix.wav");
        wave(&prefix_path, 16_000, 2, 17);
        wave(&suffix_path, 16_000, 2, 19);
        let prefix = describe(&prefix_path, CaptureFormat::stereo()).unwrap();
        let suffix = describe(&suffix_path, CaptureFormat::stereo()).unwrap();
        let inputs = [&prefix_path, &suffix_path].map(|path| {
            let original = path.metadata().unwrap().permissions();
            let mut readonly = original.clone();
            readonly.set_readonly(true);
            std::fs::set_permissions(path, readonly).unwrap();
            original
        });
        let output = root.path().join("combined.wav");
        let combined = concatenate(&prefix_path, &prefix, &suffix_path, &suffix, &output);
        for (path, permissions) in [&prefix_path, &suffix_path].into_iter().zip(inputs) {
            std::fs::set_permissions(path, permissions).unwrap();
        }
        let combined = combined.unwrap();
        assert_eq!(combined.frames, 36);
        assert_eq!(
            file_hash(&mut File::open(prefix_path).unwrap()).unwrap(),
            prefix.sha256
        );
        assert_eq!(
            file_hash(&mut File::open(suffix_path).unwrap()).unwrap(),
            suffix.sha256
        );
        assert_eq!(
            file_hash(&mut File::open(output).unwrap()).unwrap(),
            combined.sha256
        );
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
