use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use hypr_fs_format::{AudioLayout, SessionAudio};
use owhisper_interface::{batch_stream::BatchStreamEvent, stream::StreamResponse};

use crate::{BatchEvent, BatchRuntime};

pub(super) struct PreparedAudio {
    prepared: hypr_transcribe_core::recorded_audio::PreparedAudio,
    _runtime: Option<Arc<dyn BatchRuntime>>,
}

impl std::ops::Deref for PreparedAudio {
    type Target = hypr_transcribe_core::recorded_audio::PreparedAudio;
    fn deref(&self) -> &Self::Target {
        &self.prepared
    }
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

    fn from_path(path: &Path, audio: SessionAudio, cancelled: &AtomicBool) -> Result<Self, String> {
        Ok(Self {
            prepared: hypr_transcribe_core::recorded_audio::PreparedAudio::from_path(
                path, audio, cancelled,
            )?,
            _runtime: None,
        })
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

pub(super) struct PreparedRuntime {
    pub inner: Arc<dyn BatchRuntime>,
    pub prepared: Arc<PreparedAudio>,
}
impl BatchRuntime for PreparedRuntime {
    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
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
    use hypr_audio_utils::Source;
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
