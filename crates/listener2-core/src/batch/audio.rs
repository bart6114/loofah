use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use hypr_audio_utils::Source;
use hypr_fs_format::{AudioLayout, SessionAudio};
use owhisper_interface::{batch, batch_stream::BatchStreamEvent, stream::StreamResponse};

use crate::{BatchEvent, BatchRuntime};

pub(super) struct PreparedAudio {
    pub path: PathBuf,
    pub audio: SessionAudio,
    _temporary: Option<tempfile::NamedTempFile>,
    _runtime: Option<Arc<dyn BatchRuntime>>,
}

struct CancelPreparation(Arc<AtomicBool>);
impl Drop for CancelPreparation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl PreparedAudio {
    pub async fn prepare(
        path: String,
        audio: SessionAudio,
        runtime: Option<Arc<dyn BatchRuntime>>,
    ) -> crate::Result<Arc<Self>> {
        let cancelled = CancelPreparation(Arc::new(AtomicBool::new(false)));
        let flag = cancelled.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut prepared = Self::from_path(Path::new(&path), audio, &flag)?;
            // Keep the session's audio lease until blocking consumers finish, even after cancellation.
            prepared._runtime = runtime;
            Ok::<_, String>(prepared)
        })
        .await
        .map_err(|e| crate::Error::BatchError(e.to_string()))?
        .map(Arc::new)
        .map_err(crate::Error::BatchError)
    }

    fn from_path(
        path: &Path,
        mut audio: SessionAudio,
        cancelled: &AtomicBool,
    ) -> Result<Self, String> {
        let mut source = hypr_audio_utils::source_from_path(path).map_err(|e| e.to_string())?;
        let channels = usize::from(u16::from(source.channels()));
        let sample_rate = u32::from(source.sample_rate());
        if channels == 1 {
            audio.layout = AudioLayout::Mixed;
            return Ok(Self {
                path: path.into(),
                audio,
                _temporary: None,
                _runtime: None,
            });
        }
        let mut energy = vec![0.0_f64; channels];
        let mut cross = 0.0_f64;
        let mut difference = 0.0_f64;
        let mut frames = 0_u64;
        let mut frame = vec![0.0_f32; channels];
        while read_frame(&mut source, &mut frame)? {
            for (e, sample) in energy.iter_mut().zip(&frame) {
                *e += f64::from(*sample).powi(2);
            }
            if channels == 2 {
                cross += f64::from(frame[0]) * f64::from(frame[1]);
                difference += f64::from((frame[0] - frame[1]).abs());
            }
            frames += 1;
            if frames % 8192 == 0 && cancelled.load(Ordering::Relaxed) {
                return Err("Audio preparation cancelled".into());
            }
        }
        if frames == 0 {
            return Err("Audio file is empty".into());
        }
        let duplicate_native = audio.layout == AudioLayout::MicSystem
            && channels == 2
            && difference / (frames as f64) < 0.0005;
        if audio.layout == AudioLayout::MicSystem && !duplicate_native {
            if channels != 2 {
                return Err("Microphone/system audio must have two channels".into());
            }
            return Ok(Self {
                path: path.into(),
                audio,
                _temporary: None,
                _runtime: None,
            });
        }
        audio.layout = AudioLayout::Mixed;
        let inverted = channels == 2
            && energy[0] > 0.0
            && energy[1] > 0.0
            && cross / (energy[0] * energy[1]).sqrt() <= -0.95;
        let active = energy.iter().filter(|e| **e > 0.0).count().max(1) as f32;
        let temporary = tempfile::Builder::new()
            .prefix("loofah-analysis-")
            .suffix(".wav")
            .tempfile()
            .map_err(|e| e.to_string())?;
        let mut writer = hound::WavWriter::create(
            temporary.path(),
            hound::WavSpec {
                channels: 1,
                sample_rate,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .map_err(|e| e.to_string())?;
        let mut source = hypr_audio_utils::source_from_path(path).map_err(|e| e.to_string())?;
        let mut written = 0_u64;
        while read_frame(&mut source, &mut frame)? {
            if inverted {
                frame[1] = -frame[1];
            }
            let mixed = frame.iter().sum::<f32>() / active;
            writer.write_sample(mixed).map_err(|e| e.to_string())?;
            written += 1;
            if written % 8192 == 0 && cancelled.load(Ordering::Relaxed) {
                return Err("Audio preparation cancelled".into());
            }
        }
        writer.finalize().map_err(|e| e.to_string())?;
        tracing::info!(source_channels = channels, prepared_channels = 1, ?audio.layout, inverted, "batch_audio_prepared");
        Ok(Self {
            path: temporary.path().into(),
            audio,
            _temporary: Some(temporary),
            _runtime: None,
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

    fn stamp_event(&self, event: &mut BatchStreamEvent) {
        match event {
            BatchStreamEvent::Result { response } => self.stamp_response(response),
            BatchStreamEvent::Segment {
                response: StreamResponse::TranscriptResponse { channel_index, .. },
                ..
            } if self.audio.layout == AudioLayout::Mixed => {
                if let Some(channel) = channel_index.first_mut() {
                    *channel = 2;
                }
            }
            _ => {}
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

pub(super) struct PreparedRuntime {
    pub inner: Arc<dyn BatchRuntime>,
    pub prepared: Arc<PreparedAudio>,
}
impl BatchRuntime for PreparedRuntime {
    fn emit(&self, mut event: BatchEvent) {
        if let BatchEvent::BatchResponseStreamed { event, .. } = &mut event {
            self.prepared.stamp_event(event);
        }
        self.inner.emit(event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypr_fs_format::AudioSource;

    fn fixture(frames: &[[f32; 2]]) -> tempfile::NamedTempFile {
        let file = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        let mut writer = hound::WavWriter::create(
            file.path(),
            hound::WavSpec {
                channels: 2,
                sample_rate: 16000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for frame in frames {
            for sample in frame {
                writer.write_sample(*sample).unwrap();
            }
        }
        writer.finalize().unwrap();
        file
    }

    #[test]
    fn mixed_stereo_is_one_stream_without_changing_playback() {
        let file = fixture(&[[0.2, 0.1], [0.0, 0.4], [-0.2, 0.0]]);
        let original = std::fs::read(file.path()).unwrap();
        let prepared = PreparedAudio::from_path(
            file.path(),
            SessionAudio::default(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let source = hypr_audio_utils::source_from_path(&prepared.path).unwrap();
        assert_eq!(u16::from(source.channels()), 1);
        assert_eq!(source.collect::<Vec<_>>(), vec![0.15, 0.2, -0.1]);
        assert_eq!(std::fs::read(file.path()).unwrap(), original);
        let temporary = prepared.path.clone();
        let owner = Arc::new(prepared);
        let worker = owner.clone();
        drop(owner);
        assert!(temporary.exists());
        drop(worker);
        assert!(!temporary.exists());
    }

    #[test]
    fn silent_and_inverted_channels_preserve_signal() {
        for frames in [
            vec![[0.2, 0.0], [-0.4, 0.0]],
            vec![[0.0, 0.2], [0.0, -0.4]],
            vec![[0.2, -0.2], [-0.4, 0.4]],
        ] {
            let file = fixture(&frames);
            let prepared = PreparedAudio::from_path(
                file.path(),
                SessionAudio::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert_eq!(
                hypr_audio_utils::source_from_path(&prepared.path)
                    .unwrap()
                    .collect::<Vec<_>>(),
                vec![0.2, -0.4]
            );
        }
    }

    #[test]
    fn native_tracks_stay_separate_but_legacy_duplicates_become_mixed() {
        let audio = SessionAudio {
            source: AudioSource::Recording,
            layout: AudioLayout::MicSystem,
        };
        for (frames, mixed) in [
            (vec![[0.2, 0.0], [0.0, 0.4]], false),
            (vec![[0.2, 0.2], [0.4, 0.4]], true),
        ] {
            let file = fixture(&frames);
            let prepared =
                PreparedAudio::from_path(file.path(), audio, &AtomicBool::new(false)).unwrap();
            assert_eq!(prepared.audio.layout == AudioLayout::Mixed, mixed);
            assert_eq!(
                u16::from(
                    hypr_audio_utils::source_from_path(&prepared.path)
                        .unwrap()
                        .channels()
                ),
                if mixed { 1 } else { 2 }
            );
        }
    }

    #[test]
    fn cancellation_does_not_change_the_source() {
        let file = fixture(&vec![[0.2, 0.1]; 16000]);
        let original = std::fs::read(file.path()).unwrap();
        assert!(
            PreparedAudio::from_path(file.path(), SessionAudio::default(), &AtomicBool::new(true))
                .is_err()
        );
        assert_eq!(std::fs::read(file.path()).unwrap(), original);
    }
}
