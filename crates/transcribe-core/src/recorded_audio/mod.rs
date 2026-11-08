//! Existing desktop recording decode and channel repair for trusted file owners.
//! The preparation owns only temporary analysis audio; canonical playback is untouched.

use hypr_audio_utils::Source;
use hypr_fs_format::{AudioLayout, SessionAudio};
use owhisper_interface::batch;
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

pub struct PreparedAudio {
    pub path: PathBuf,
    pub audio: SessionAudio,
    pub pcm: hypr_audio_utils::PcmDescriptor,
    _temporary: tempfile::NamedTempFile,
}

impl PreparedAudio {
    pub fn from_path(
        path: &Path,
        mut audio: SessionAudio,
        cancelled: &AtomicBool,
    ) -> Result<Self, String> {
        let source = hypr_audio_utils::source_from_path(path).map_err(|e| e.to_string())?;
        let channels = usize::from(u16::from(source.channels()));
        let temporary = tempfile::Builder::new()
            .prefix("loofah-analysis-")
            .suffix(".wav")
            .tempfile()
            .map_err(|e| e.to_string())?;
        let mut energy = vec![0.0_f64; channels];
        let mut cross = 0.0_f64;
        let mut difference = 0.0_f64;
        let mut frames = 0_u64;
        let pcm = hypr_audio_utils::decode_to_pcm(
            source,
            temporary.path(),
            || cancelled.load(Ordering::Relaxed),
            |frame| {
                for (e, sample) in energy.iter_mut().zip(frame) {
                    *e += f64::from(*sample).powi(2);
                }
                if channels == 2 {
                    cross += f64::from(frame[0]) * f64::from(frame[1]);
                    difference += f64::from((frame[0] - frame[1]).abs());
                }
                frames += 1;
            },
        )
        .map_err(|e| e.to_string())?;
        if channels == 1 {
            audio.layout = AudioLayout::Mixed;
            return Ok(Self {
                path: pcm.path.clone(),
                pcm,
                audio,
                _temporary: temporary,
            });
        }
        let duplicate_native = audio.layout == AudioLayout::MicSystem
            && channels == 2
            && difference / (frames as f64) < 0.0005;
        if audio.layout == AudioLayout::MicSystem && !duplicate_native {
            if channels != 2 {
                return Err("Microphone/system audio must have two channels".into());
            }
            return Ok(Self {
                path: pcm.path.clone(),
                pcm,
                audio,
                _temporary: temporary,
            });
        }
        audio.layout = AudioLayout::Mixed;
        let inverted = channels == 2
            && energy[0] > 0.0
            && energy[1] > 0.0
            && cross / (energy[0] * energy[1]).sqrt() <= -0.95;
        let active = energy.iter().filter(|e| **e > 0.0).count().max(1) as f32;
        let mixed_temporary = tempfile::Builder::new()
            .prefix("loofah-analysis-mixed-")
            .suffix(".wav")
            .tempfile()
            .map_err(|e| e.to_string())?;
        let mut writer = hound::WavWriter::create(
            mixed_temporary.path(),
            hound::WavSpec {
                channels: 1,
                sample_rate: hypr_audio_utils::PCM_SAMPLE_RATE,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .map_err(|e| e.to_string())?;
        let mut source =
            hypr_audio_utils::source_from_path(&pcm.path).map_err(|e| e.to_string())?;
        let mut frame = vec![0.0; channels];
        let mut written = 0_u64;
        while read_frame(&mut source, &mut frame)? {
            if inverted {
                frame[1] = -frame[1];
            }
            let mixed = frame.iter().sum::<f32>() / active;
            if !mixed.is_finite() {
                return Err("Mixed audio contains non-finite samples".into());
            }
            writer.write_sample(mixed).map_err(|e| e.to_string())?;
            written += 1;
            if written % 8192 == 0 && cancelled.load(Ordering::Relaxed) {
                return Err("Audio preparation cancelled".into());
            }
        }
        writer.finalize().map_err(|e| e.to_string())?;
        tracing::info!(source_channels = channels, prepared_channels = 1, ?audio.layout, inverted, "batch_audio_prepared");
        Ok(Self {
            path: mixed_temporary.path().into(),
            pcm: hypr_audio_utils::PcmDescriptor::open(mixed_temporary.path())
                .map_err(|e| e.to_string())?,
            audio,
            _temporary: mixed_temporary,
        })
    }

    pub fn stamp_response(&self, response: &mut batch::Response) {
        if !response.metadata.is_object() {
            response.metadata = serde_json::json!({});
        }
        response.metadata["session_audio"] = serde_json::to_value(self.audio).unwrap();
        if self.audio.layout == AudioLayout::Mixed {
            for channel in &mut response.results.channels {
                for alternative in &mut channel.alternatives {
                    for word in &mut alternative.words {
                        word.channel = 2;
                    }
                }
            }
        }
    }
}

fn read_frame(source: &mut impl Iterator<Item = f32>, frame: &mut [f32]) -> Result<bool, String> {
    for (index, sample) in frame.iter_mut().enumerate() {
        match source.next() {
            Some(value) if value.is_finite() => *sample = value,
            Some(_) => return Err("Audio contains non-finite samples".into()),
            None if index == 0 => return Ok(false),
            None => return Err("Audio ends inside a channel frame".into()),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
