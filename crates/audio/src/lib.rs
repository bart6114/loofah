use std::pin::Pin;
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};
use std::task::{Context, Poll};

use futures_util::Stream;

#[derive(thiserror::Error, Debug, Clone, PartialEq, Eq)]
pub enum Error {
    #[error("no input device found")]
    NoInputDevice,
    #[error("mic_open_failed")]
    MicOpenFailed,
    #[error("mic_stream_setup_failed")]
    MicStreamSetupFailed,
    #[error(
        "System audio recording permission is not granted. Enable \"System Audio Recording Only\" for this app in System Settings > Privacy & Security > Screen & System Audio Recording, then try again."
    )]
    SystemAudioPermissionDenied,
    #[error("speaker_tap_failed: {0}")]
    SpeakerTapFailed(String),
    #[error("speaker_aggregate_device_failed: {0}")]
    SpeakerAggregateDeviceFailed(String),
    #[error("speaker_stream_setup_failed: {0}")]
    SpeakerStreamSetupFailed(String),
    #[error("mic_resample_failed")]
    MicResampleFailed,
    #[error("speaker_resample_failed")]
    SpeakerResampleFailed,
    #[error("mic_stream_ended")]
    MicStreamEnded,
    #[error("speaker_stream_ended")]
    SpeakerStreamEnded,
    #[error("Audio device recovery: microphone={microphone}, system audio={speaker}")]
    CaptureRecovering { microphone: bool, speaker: bool },
}

#[derive(Debug, Clone)]
pub struct CaptureConfig {
    pub sample_rate: u32,
    pub chunk_size: usize,
    pub mic_device: Option<String>,
    pub enable_aec: bool,
}

#[derive(Debug, Clone)]
pub struct CaptureFrame {
    pub raw_mic: Arc<[f32]>,
    pub raw_speaker: Arc<[f32]>,
    pub aec_mic: Option<Arc<[f32]>>,
}

impl CaptureFrame {
    pub fn preferred_mic(&self) -> Arc<[f32]> {
        self.aec_mic
            .as_ref()
            .map(Arc::clone)
            .unwrap_or_else(|| Arc::clone(&self.raw_mic))
    }

    pub fn raw_dual(&self) -> (Arc<[f32]>, Arc<[f32]>) {
        (Arc::clone(&self.raw_mic), Arc::clone(&self.raw_speaker))
    }

    pub fn aec_dual(&self) -> (Arc<[f32]>, Arc<[f32]>) {
        (self.preferred_mic(), Arc::clone(&self.raw_speaker))
    }
}

/// Per-stream evidence of known input discontinuity. Recovery cannot clear earlier loss.
/// Clear evidence alone does not prove that native buffers were completely drained.
#[derive(Debug, Default)]
pub struct CaptureIntegrity {
    lost_channels: AtomicU8,
}

impl CaptureIntegrity {
    pub fn mark_discontinuity(&self, microphone: bool, speaker: bool) {
        let bits = u8::from(microphone) | (u8::from(speaker) << 1);
        self.lost_channels.fetch_or(bits, Ordering::Release);
    }
    pub fn discontinuous(&self) -> bool {
        self.lost_channels.load(Ordering::Acquire) != 0
    }
    pub fn channels(&self) -> (bool, bool) {
        let bits = self.lost_channels.load(Ordering::Acquire);
        (bits & 1 != 0, bits & 2 != 0)
    }
}

pub struct CaptureStream {
    stream: Pin<Box<dyn Stream<Item = Result<CaptureFrame, Error>> + Send>>,
    integrity: Arc<CaptureIntegrity>,
}

impl CaptureStream {
    pub fn new(stream: impl Stream<Item = Result<CaptureFrame, Error>> + Send + 'static) -> Self {
        Self {
            stream: Box::pin(stream),
            integrity: Arc::new(CaptureIntegrity::default()),
        }
    }
    pub fn with_integrity(mut self, integrity: Arc<CaptureIntegrity>) -> Self {
        self.integrity = integrity;
        self
    }
    pub fn integrity(&self) -> Arc<CaptureIntegrity> {
        self.integrity.clone()
    }
}

impl Stream for CaptureStream {
    type Item = Result<CaptureFrame, Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.stream.as_mut().poll_next(cx)
    }
}

pub trait AudioProvider: Send + Sync {
    fn open_capture(&self, config: CaptureConfig) -> Result<CaptureStream, Error>;
    fn open_speaker_capture(
        &self,
        sample_rate: u32,
        chunk_size: usize,
    ) -> Result<CaptureStream, Error>;
    fn open_mic_capture(
        &self,
        device: Option<String>,
        sample_rate: u32,
        chunk_size: usize,
    ) -> Result<CaptureStream, Error>;

    fn default_device_name(&self) -> String;
    fn list_mic_devices(&self) -> Vec<String>;

    fn play_silence(&self) -> std::sync::mpsc::Sender<()>;
    fn play_bytes(&self, bytes: &'static [u8]) -> std::sync::mpsc::Sender<()>;

    fn probe_mic(&self, device: Option<String>) -> Result<(), Error>;
    fn probe_speaker(&self) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_frame_exposes_raw_and_aec_views() {
        let frame = CaptureFrame {
            raw_mic: Arc::from([0.1_f32, 0.2]),
            raw_speaker: Arc::from([0.3_f32, 0.4]),
            aec_mic: Some(Arc::from([0.9_f32, 1.0])),
        };

        let (raw_mic, raw_speaker) = frame.raw_dual();
        assert_eq!(&*raw_mic, &[0.1, 0.2]);
        assert_eq!(&*raw_speaker, &[0.3, 0.4]);

        let (aec_mic, aec_speaker) = frame.aec_dual();
        assert_eq!(&*aec_mic, &[0.9, 1.0]);
        assert_eq!(&*aec_speaker, &[0.3, 0.4]);
    }
    #[test]
    fn capture_integrity_is_sticky_and_distinct_for_each_new_stream() {
        let first = CaptureStream::new(futures_util::stream::empty());
        let evidence = first.integrity();
        evidence.mark_discontinuity(true, false);
        evidence.mark_discontinuity(false, false);
        assert!(first.integrity().discontinuous());
        assert_eq!(evidence.channels(), (true, false));
        evidence.mark_discontinuity(false, true);
        assert_eq!(evidence.channels(), (true, true));
        let second = CaptureStream::new(futures_util::stream::empty());
        assert!(!second.integrity().discontinuous());
        let wrapped = second.with_integrity(evidence.clone());
        assert!(Arc::ptr_eq(&wrapped.integrity(), &evidence));
    }
}
