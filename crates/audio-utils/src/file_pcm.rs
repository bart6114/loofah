use std::path::{Path, PathBuf};

use audioadapter_buffers::direct::SequentialSliceOfVecs;
use rubato::{
    Async, FixedAsync, Indexing, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

use crate::{Error, Source};

pub const PCM_SAMPLE_RATE: u32 = 16_000;
pub const PCM_BLOCK_FRAMES: usize = 1024;

#[derive(Clone, Debug)]
pub struct PcmDescriptor {
    pub path: PathBuf,
    pub frames: usize,
    pub channels: usize,
    pub sample_rate: u32,
}

impl PcmDescriptor {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let reader = hound::WavReader::open(path.as_ref())?;
        let spec = reader.spec();
        if spec.sample_rate != PCM_SAMPLE_RATE
            || spec.sample_format != hound::SampleFormat::Float
            || spec.bits_per_sample != 32
        {
            return Err(invalid("Expected 16 kHz float PCM"));
        }
        if reader.duration() == 0 || reader.len() % u32::from(spec.channels) != 0 {
            return Err(invalid("Empty or incomplete PCM frames"));
        }
        Ok(Self {
            path: path.as_ref().into(),
            frames: reader.duration() as usize,
            channels: spec.channels as usize,
            sample_rate: spec.sample_rate,
        })
    }

    pub fn duration(&self) -> f64 {
        self.frames as f64 / self.sample_rate as f64
    }

    pub fn reader(&self) -> Result<PcmReader, Error> {
        Ok(PcmReader {
            descriptor: self.clone(),
            next_frame: 0,
            reader: hound::WavReader::open(&self.path)?,
        })
    }
}

pub struct PcmReader {
    descriptor: PcmDescriptor,
    next_frame: usize,
    reader: hound::WavReader<std::io::BufReader<std::fs::File>>,
}

impl PcmReader {
    pub fn channel(
        &mut self,
        channel: usize,
        range: std::ops::Range<usize>,
    ) -> Result<Vec<f32>, Error> {
        if channel >= self.descriptor.channels
            || range.start > range.end
            || range.end > self.descriptor.frames
        {
            return Err(invalid("Invalid PCM channel or frame range"));
        }
        if self.next_frame != range.start {
            self.reader.seek(
                u32::try_from(range.start).map_err(|_| invalid("PCM offset is too large"))?,
            )?;
        }
        self.next_frame = range.end;
        let mut output = Vec::with_capacity(range.len());
        let mut samples = self.reader.samples::<f32>();
        for _ in range {
            for index in 0..self.descriptor.channels {
                let sample = samples.next().ok_or_else(|| invalid("Truncated PCM"))??;
                if !sample.is_finite() {
                    return Err(invalid("Audio contains non-finite samples"));
                }
                if index == channel {
                    output.push(sample);
                }
            }
        }
        Ok(output)
    }
}

pub fn pcm_spec(channels: usize) -> hound::WavSpec {
    hound::WavSpec {
        channels: channels as u16,
        sample_rate: PCM_SAMPLE_RATE,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    }
}

fn invalid(message: &str) -> Error {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
}

pub fn decode_to_pcm<S: Source<Item = f32>>(
    mut source: S,
    path: impl AsRef<Path>,
    cancelled: impl Fn() -> bool,
    mut observe: impl FnMut(&[f32]),
) -> Result<PcmDescriptor, Error> {
    let channels = u16::from(source.channels()) as usize;
    let rate = u32::from(source.sample_rate());
    let mut writer = hound::WavWriter::create(path.as_ref(), pcm_spec(channels))?;
    let mut input = vec![vec![0.0; PCM_BLOCK_FRAMES]; channels];
    let mut frame = vec![0.0; channels];
    let mut resampler = if rate == PCM_SAMPLE_RATE {
        None
    } else {
        Some(Async::<f32>::new_sinc(
            PCM_SAMPLE_RATE as f64 / rate as f64,
            2.0,
            &SincInterpolationParameters {
                sinc_len: 256,
                f_cutoff: 0.95,
                interpolation: SincInterpolationType::Linear,
                oversampling_factor: 256,
                window: WindowFunction::BlackmanHarris2,
            },
            PCM_BLOCK_FRAMES,
            channels,
            FixedAsync::Input,
        )?)
    };
    let output_capacity = resampler
        .as_ref()
        .map_or(PCM_BLOCK_FRAMES, |r| r.output_frames_max());
    let buffer_bytes = (PCM_BLOCK_FRAMES + output_capacity)
        .saturating_mul(channels)
        .saturating_mul(std::mem::size_of::<f32>());
    if buffer_bytes > 64 * 1024 * 1024 {
        return Err(invalid("Audio format exceeds PCM buffer budget"));
    }
    tracing::info!(
        audio_buffer_bytes = buffer_bytes,
        "pcm_decode_buffer_budget"
    );
    let mut output = vec![vec![0.0; output_capacity]; channels];
    let mut trim = resampler.as_ref().map_or(0, |r| r.output_delay());
    let mut input_frames = 0usize;
    let mut written = 0usize;
    let mut eof = false;
    loop {
        if cancelled() {
            return Err(invalid("Audio preparation cancelled"));
        }
        let mut count = 0;
        if !eof {
            'read: for index in 0..PCM_BLOCK_FRAMES {
                for channel in 0..channels {
                    match source.next() {
                        Some(value) if value.is_finite() => frame[channel] = value,
                        Some(_) => return Err(invalid("Audio contains non-finite samples")),
                        None if channel == 0 => {
                            eof = true;
                            break 'read;
                        }
                        None => return Err(invalid("Audio ends inside a channel frame")),
                    }
                }
                observe(&frame);
                for channel in 0..channels {
                    input[channel][index] = frame[channel];
                }
                count += 1;
            }
        }
        input_frames += count;
        let expected = (input_frames as f64 * PCM_SAMPLE_RATE as f64 / rate as f64).ceil() as usize;
        if eof && written >= expected {
            break;
        }
        let produced = if let Some(resampler) = &mut resampler {
            let input_adapter =
                SequentialSliceOfVecs::new(&input, channels, PCM_BLOCK_FRAMES).unwrap();
            let mut output_adapter =
                SequentialSliceOfVecs::new_mut(&mut output, channels, output_capacity).unwrap();
            let indexing = Indexing {
                input_offset: 0,
                output_offset: 0,
                active_channels_mask: None,
                partial_len: (count < PCM_BLOCK_FRAMES).then_some(count),
            };
            resampler
                .process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))?
                .1
        } else {
            for channel in 0..channels {
                output[channel][..count].copy_from_slice(&input[channel][..count]);
            }
            count
        };
        let skip = trim.min(produced);
        trim -= skip;
        let end = if eof {
            produced.min(skip + expected.saturating_sub(written))
        } else {
            produced
        };
        for index in skip..end {
            for channel in &output {
                writer.write_sample(channel[index])?;
            }
            written += 1;
        }
    }
    if input_frames == 0 {
        return Err(invalid("Audio file is empty"));
    }
    writer.finalize()?;
    PcmDescriptor::open(path)
}

pub struct PcmFile {
    pub descriptor: PcmDescriptor,
    _temporary: Option<tempfile::NamedTempFile>,
}

impl PcmFile {
    pub fn prepare(path: impl AsRef<Path>, cancelled: impl Fn() -> bool) -> Result<Self, Error> {
        if let Ok(descriptor) = PcmDescriptor::open(&path) {
            return Ok(Self {
                descriptor,
                _temporary: None,
            });
        }
        let temporary = tempfile::Builder::new()
            .prefix("loofah-pcm-")
            .suffix(".wav")
            .tempfile()?;
        let descriptor = decode_to_pcm(
            crate::source_from_path(path)?,
            temporary.path(),
            cancelled,
            |_| {},
        )?;
        Ok(Self {
            descriptor,
            _temporary: Some(temporary),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resampler_preserves_channel_phase_duration_and_tail() {
        for rate in [8000, 16000, 44100, 48000] {
            for frames in [200, 1024, 15301] {
                let data: Vec<_> = (0..frames)
                    .flat_map(|i| {
                        let s = (i as f32 * 0.04).sin();
                        [s, -s]
                    })
                    .collect();
                let source = || {
                    rodio::buffer::SamplesBuffer::new(
                        std::num::NonZeroU16::new(2).unwrap(),
                        std::num::NonZeroU32::new(rate).unwrap(),
                        data.clone(),
                    )
                };
                let file = tempfile::NamedTempFile::new().unwrap();
                let pcm = decode_to_pcm(source(), file.path(), || false, |_| {}).unwrap();
                assert_eq!(
                    pcm.frames,
                    (frames as f64 * 16000.0 / rate as f64).ceil() as usize
                );
                let mut reader = pcm.reader().unwrap();
                let left = reader.channel(0, 0..pcm.frames).unwrap();
                let right = reader.channel(1, 0..pcm.frames).unwrap();
                assert!(left.iter().zip(right).all(|(a, b)| (a + b).abs() < 1e-6));
                if frames > 1024 {
                    let old = crate::resample_audio(source(), 16000).unwrap();
                    let mismatches: Vec<_> = left
                        .iter()
                        .zip(old.chunks_exact(2))
                        .enumerate()
                        // The legacy whole-file helper copies only part of the first delayed block.
                        .skip(
                            (2.0 * PCM_BLOCK_FRAMES as f64 * 16000.0 / rate as f64).ceil() as usize,
                        )
                        .filter(|(_, (a, b))| (*a - b[0]).abs() >= 1e-5)
                        .map(|(i, _)| i)
                        .collect();
                    assert!(
                        mismatches.is_empty(),
                        "rate={rate} frames={frames} mismatches={:?}",
                        &mismatches[..mismatches.len().min(8)]
                    );
                }
            }
        }
    }
    #[test]
    fn supported_formats_decode_to_bounded_pcm() {
        for path in [
            hypr_data::english_1::AUDIO_PATH,
            hypr_data::english_1::AUDIO_MP3_PATH,
            hypr_data::english_1::AUDIO_MP4_PATH,
            hypr_data::english_1::AUDIO_M4A_PATH,
            hypr_data::english_1::AUDIO_OGG_PATH,
            hypr_data::english_1::AUDIO_FLAC_PATH,
            hypr_data::english_1::AUDIO_AAC_PATH,
            hypr_data::english_1::AUDIO_AIFF_PATH,
        ] {
            let source = crate::source_from_path(path).unwrap();
            let source_rate = u32::from(source.sample_rate());
            let channels = u16::from(source.channels()) as usize;
            let input_samples = source.count();
            let pcm = PcmFile::prepare(path, || false).unwrap();
            assert_eq!(pcm.descriptor.channels, channels);
            assert_eq!(
                pcm.descriptor.frames,
                ((input_samples / channels) as f64 * 16000.0 / source_rate as f64).ceil() as usize
            );
            assert!(
                pcm.descriptor
                    .reader()
                    .unwrap()
                    .channel(0, 0..pcm.descriptor.frames.min(16000))
                    .unwrap()
                    .iter()
                    .all(|s| s.is_finite())
            );
        }
    }

    #[test]
    fn pcm_ranges_are_channel_exact_and_reject_invalid_reads() {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = hound::WavWriter::create(file.path(), pcm_spec(2)).unwrap();
        for sample in [0.1_f32, -0.1, 0.2, -0.2, 0.3, -0.3] {
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
        let descriptor = PcmDescriptor::open(file.path()).unwrap();
        let mut reader = descriptor.reader().unwrap();
        assert_eq!(reader.channel(0, 0..1).unwrap(), vec![0.1]);
        assert_eq!(reader.channel(0, 1..3).unwrap(), vec![0.2, 0.3]);
        assert_eq!(reader.channel(1, 1..3).unwrap(), vec![-0.2, -0.3]);
        assert!(reader.channel(2, 0..1).is_err());
        assert!(reader.channel(0, 0..4).is_err());
    }

    #[test]
    fn decoded_artifact_is_removed_and_input_is_untouched() {
        let original = std::fs::read(hypr_data::english_1::AUDIO_MP3_PATH).unwrap();
        let pcm = PcmFile::prepare(hypr_data::english_1::AUDIO_MP3_PATH, || false).unwrap();
        let path = pcm.descriptor.path.clone();
        assert!(path.exists());
        drop(pcm);
        assert!(!path.exists());
        assert_eq!(
            std::fs::read(hypr_data::english_1::AUDIO_MP3_PATH).unwrap(),
            original
        );
    }

    #[test]
    fn rejects_nonfinite_and_cancellation() {
        for (sample, cancel) in [(f32::NAN, false), (0.0, true)] {
            let source = rodio::buffer::SamplesBuffer::new(
                std::num::NonZeroU16::new(1).unwrap(),
                std::num::NonZeroU32::new(16000).unwrap(),
                vec![sample],
            );
            let file = tempfile::NamedTempFile::new().unwrap();
            assert!(decode_to_pcm(source, file.path(), || cancel, |_| {}).is_err());
        }
    }
}
