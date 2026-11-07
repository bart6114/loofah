use std::{collections::VecDeque, fmt, sync::Arc};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::super::{ChannelMode, SAMPLE_RATE};

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct DeviceCaptureKey(String);

impl DeviceCaptureKey {
    pub fn new(fingerprint: String) -> Result<Self, DeviceSinkError> {
        if fingerprint.len() != 64
            || !fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(DeviceSinkError("Invalid device capture fingerprint".into()));
        }
        Ok(Self(fingerprint))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn actor_id(&self) -> String {
        format!("device:{}", self.0)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceRecordingEnd {
    Complete,
    Interrupted,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceAppendReceipt {
    pub sequence: u64,
    pub frames: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceFinishReceipt {
    pub chunk_count: u64,
    pub frames: u64,
    pub pcm_bytes: u64,
}

#[derive(Clone, Debug)]
pub struct DeviceSinkError(pub String);

impl fmt::Display for DeviceSinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for DeviceSinkError {}

#[ractor::async_trait]
/// Append receipts acknowledge durable storage. Retryable contention must retain the same
/// chunk; an interrupted finish flushes admitted audio without sealing a complete source.
pub trait DeviceRecordingSink: Send + Sync {
    async fn append(
        &self,
        sequence: u64,
        frames: u32,
        pcm: Arc<[u8]>,
    ) -> Result<DeviceAppendReceipt, DeviceSinkError>;
    async fn finish(&self, end: DeviceRecordingEnd)
    -> Result<DeviceFinishReceipt, DeviceSinkError>;
}

#[derive(Clone)]
pub struct DeviceIngress {
    frames: Arc<Semaphore>,
    messages: Arc<Semaphore>,
}

pub struct DeviceFrameCredit {
    frames: u32,
    _frames: OwnedSemaphorePermit,
    _message: OwnedSemaphorePermit,
}

impl Default for DeviceIngress {
    fn default() -> Self {
        Self {
            frames: Arc::new(Semaphore::new(SAMPLE_RATE as usize * 8)),
            messages: Arc::new(Semaphore::new(128)),
        }
    }
}

impl DeviceIngress {
    pub fn admit(&self, frames: usize) -> Result<DeviceFrameCredit, DeviceSinkError> {
        // Bound both payload and messages before entering the actors' unbounded mailboxes.
        if frames == 0 || frames > SAMPLE_RATE as usize {
            return Err(DeviceSinkError("Invalid device audio frame length".into()));
        }
        let message = self
            .messages
            .clone()
            .try_acquire_owned()
            .map_err(|_| DeviceSinkError("Device recording message backlog is full".into()))?;
        let frames = self
            .frames
            .clone()
            .try_acquire_many_owned(frames as u32)
            .map_err(|_| DeviceSinkError("Device recording storage backlog is full".into()))?;
        Ok(DeviceFrameCredit {
            frames: frames.num_permits() as u32,
            _frames: frames,
            _message: message,
        })
    }
}

pub(super) struct DeviceSink {
    sink: Arc<dyn DeviceRecordingSink>,
    mode: ChannelMode,
    pending: Vec<u8>,
    credits: VecDeque<(u32, DeviceFrameCredit)>,
    sequence: u64,
    frames: u64,
    pcm_bytes: u64,
    end: Option<DeviceRecordingEnd>,
}

impl DeviceSink {
    pub(super) fn new(sink: Arc<dyn DeviceRecordingSink>, mode: ChannelMode) -> Self {
        Self {
            sink,
            mode,
            pending: Vec::new(),
            credits: VecDeque::new(),
            sequence: 0,
            frames: 0,
            pcm_bytes: 0,
            end: None,
        }
    }

    fn frame_bytes(&self) -> usize {
        if self.mode == ChannelMode::MicAndSpeaker {
            8
        } else {
            4
        }
    }

    pub(super) async fn write(
        &mut self,
        mic: &[f32],
        speaker: &[f32],
        credit: DeviceFrameCredit,
    ) -> Result<(), DeviceSinkError> {
        if self.end.is_some() {
            return Err(DeviceSinkError("Device recording is closed".into()));
        }
        let frames = match self.mode {
            ChannelMode::MicOnly => mic.len(),
            ChannelMode::SpeakerOnly => speaker.len(),
            ChannelMode::MicAndSpeaker => mic.len().max(speaker.len()),
        };
        if frames == 0
            || frames > credit.frames as usize
            || frames > SAMPLE_RATE as usize
            || mic.iter().chain(speaker).any(|sample| !sample.is_finite())
        {
            return Err(DeviceSinkError("Invalid device PCM samples".into()));
        }
        for frame in 0..frames {
            match self.mode {
                ChannelMode::MicOnly => self.pending.extend_from_slice(&mic[frame].to_le_bytes()),
                ChannelMode::SpeakerOnly => self
                    .pending
                    .extend_from_slice(&speaker[frame].to_le_bytes()),
                ChannelMode::MicAndSpeaker => {
                    self.pending
                        .extend_from_slice(&mic.get(frame).copied().unwrap_or(0.0).to_le_bytes());
                    self.pending.extend_from_slice(
                        &speaker.get(frame).copied().unwrap_or(0.0).to_le_bytes(),
                    );
                }
            }
        }
        self.credits.push_back((frames as u32, credit));
        while self.pending.len() >= SAMPLE_RATE as usize * self.frame_bytes() {
            self.commit(SAMPLE_RATE).await?;
        }
        Ok(())
    }

    async fn commit(&mut self, frames: u32) -> Result<(), DeviceSinkError> {
        let bytes = frames as usize * self.frame_bytes();
        let receipt = self
            .sink
            .append(self.sequence, frames, Arc::from(&self.pending[..bytes]))
            .await?;
        if receipt
            != (DeviceAppendReceipt {
                sequence: self.sequence,
                frames,
            })
        {
            return Err(DeviceSinkError(
                "Device sink returned a mismatched append receipt".into(),
            ));
        }
        self.pending.drain(..bytes);
        self.sequence += 1;
        self.frames += u64::from(frames);
        self.pcm_bytes += bytes as u64;
        let mut confirmed = frames;
        while let Some((remaining, _)) = self.credits.front_mut() {
            if *remaining > confirmed {
                *remaining -= confirmed;
                break;
            }
            confirmed -= *remaining;
            self.credits.pop_front();
            if confirmed == 0 {
                break;
            }
        }
        Ok(())
    }

    pub(super) async fn finish(
        &mut self,
        end: DeviceRecordingEnd,
    ) -> Result<DeviceFinishReceipt, DeviceSinkError> {
        if let Some(previous) = self.end {
            if previous != end {
                return Err(DeviceSinkError(
                    "Device recording was already finalized with another outcome".into(),
                ));
            }
            return Ok(DeviceFinishReceipt {
                chunk_count: self.sequence,
                frames: self.frames,
                pcm_bytes: self.pcm_bytes,
            });
        }
        if !self.pending.is_empty() {
            self.commit((self.pending.len() / self.frame_bytes()) as u32)
                .await?;
        }
        let expected = DeviceFinishReceipt {
            chunk_count: self.sequence,
            frames: self.frames,
            pcm_bytes: self.pcm_bytes,
        };
        let receipt = self.sink.finish(end).await?;
        if receipt != expected {
            return Err(DeviceSinkError(
                "Device sink returned mismatched final totals".into(),
            ));
        }
        self.end = Some(end);
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };

    #[derive(Default)]
    struct MemorySink {
        chunks: Mutex<Vec<(u32, Vec<u8>)>>,
        end: Mutex<Option<DeviceRecordingEnd>>,
        malformed: AtomicBool,
        malformed_finish: AtomicBool,
    }
    #[ractor::async_trait]
    impl DeviceRecordingSink for MemorySink {
        async fn append(
            &self,
            sequence: u64,
            frames: u32,
            pcm: Arc<[u8]>,
        ) -> Result<DeviceAppendReceipt, DeviceSinkError> {
            self.chunks.lock().unwrap().push((frames, pcm.to_vec()));
            Ok(DeviceAppendReceipt {
                sequence: sequence + u64::from(self.malformed.load(Ordering::SeqCst)),
                frames,
            })
        }
        async fn finish(
            &self,
            end: DeviceRecordingEnd,
        ) -> Result<DeviceFinishReceipt, DeviceSinkError> {
            *self.end.lock().unwrap() = Some(end);
            let chunks = self.chunks.lock().unwrap();
            Ok(DeviceFinishReceipt {
                chunk_count: chunks.len() as u64,
                frames: chunks
                    .iter()
                    .map(|(frames, _)| u64::from(*frames))
                    .sum::<u64>()
                    + u64::from(self.malformed_finish.load(Ordering::SeqCst)),
                pcm_bytes: chunks.iter().map(|(_, bytes)| bytes.len() as u64).sum(),
            })
        }
    }

    #[test]
    fn ingress_bounds_frames_messages_and_identity() {
        assert!(DeviceCaptureKey::new("A".repeat(64)).is_err());
        assert!(DeviceCaptureKey::new("a".repeat(63)).is_err());
        assert!(
            DeviceCaptureKey::new("a".repeat(64))
                .unwrap()
                .actor_id()
                .starts_with("device:")
        );
        let ingress = DeviceIngress::default();
        assert!(ingress.admit(0).is_err());
        assert!(ingress.admit(16_001).is_err());
        let credits: Vec<_> = (0..8).map(|_| ingress.admit(16_000).unwrap()).collect();
        assert!(ingress.admit(1).is_err());
        drop(credits);
        let credits: Vec<_> = (0..128).map(|_| ingress.admit(1).unwrap()).collect();
        assert!(ingress.admit(1).is_err());
        drop(credits);
        assert!(ingress.admit(16_000).is_ok());
    }

    #[tokio::test]
    async fn stereo_retains_f32_layout_splits_chunks_and_flushes_interrupted_tail() {
        let memory = Arc::new(MemorySink::default());
        let ingress = DeviceIngress::default();
        let mut sink = DeviceSink::new(memory.clone(), ChannelMode::MicAndSpeaker);
        sink.write(
            &vec![-0.0; 12_000],
            &vec![0.75; 11_999],
            ingress.admit(12_000).unwrap(),
        )
        .await
        .unwrap();
        sink.write(
            &vec![-0.5; 4_321],
            &vec![0.25; 4_321],
            ingress.admit(4_321).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(sink.frames, 16_000);
        assert_eq!(sink.pending.len(), 321 * 8);
        let receipt = sink.finish(DeviceRecordingEnd::Interrupted).await.unwrap();
        assert_eq!(
            receipt,
            DeviceFinishReceipt {
                chunk_count: 2,
                frames: 16_321,
                pcm_bytes: 16_321 * 8
            }
        );
        assert_eq!(
            *memory.end.lock().unwrap(),
            Some(DeviceRecordingEnd::Interrupted)
        );
        let chunks = memory.chunks.lock().unwrap();
        assert_eq!(
            chunks.iter().map(|chunk| chunk.0).collect::<Vec<_>>(),
            [16_000, 321]
        );
        let pcm: Vec<_> = chunks
            .iter()
            .flat_map(|chunk| chunk.1.iter().copied())
            .collect();
        assert_eq!(&pcm[..4], &(-0.0_f32).to_le_bytes());
        assert_eq!(&pcm[4..8], &0.75_f32.to_le_bytes());
        assert_eq!(&pcm[11_999 * 8 + 4..12_000 * 8], &0.0_f32.to_le_bytes());
        assert_eq!(&pcm[12_000 * 8..12_000 * 8 + 4], &(-0.5_f32).to_le_bytes());
        assert_eq!(ingress.frames.available_permits(), 128_000);
        assert_eq!(
            sink.finish(DeviceRecordingEnd::Interrupted).await.unwrap(),
            receipt
        );
        assert!(sink.finish(DeviceRecordingEnd::Complete).await.is_err());
    }

    #[tokio::test]
    async fn malformed_receipts_never_confirm_credits_or_finalization() {
        let memory = Arc::new(MemorySink::default());
        memory.malformed.store(true, Ordering::SeqCst);
        let ingress = DeviceIngress::default();
        let mut sink = DeviceSink::new(memory.clone(), ChannelMode::MicOnly);
        assert!(
            sink.write(&vec![0.2; 16_000], &[], ingress.admit(16_000).unwrap())
                .await
                .is_err()
        );
        assert_eq!(sink.frames, 0);
        assert_eq!(sink.pending.len(), 64_000);
        assert_eq!(ingress.frames.available_permits(), 112_000);
        let memory = Arc::new(MemorySink::default());
        memory.malformed_finish.store(true, Ordering::SeqCst);
        let mut sink = DeviceSink::new(memory, ChannelMode::SpeakerOnly);
        sink.write(&[], &[0.1, -0.25], ingress.admit(2).unwrap())
            .await
            .unwrap();
        assert!(sink.finish(DeviceRecordingEnd::Complete).await.is_err());
        assert!(sink.end.is_none());
    }

    #[tokio::test]
    async fn invalid_pcm_or_uncredited_payload_fails_without_a_chunk() {
        let memory = Arc::new(MemorySink::default());
        let ingress = DeviceIngress::default();
        let mut sink = DeviceSink::new(memory.clone(), ChannelMode::MicOnly);
        assert!(
            sink.write(&[f32::NAN], &[], ingress.admit(1).unwrap())
                .await
                .is_err()
        );
        assert!(
            sink.write(&[0.1, 0.2], &[], ingress.admit(1).unwrap())
                .await
                .is_err()
        );
        assert!(memory.chunks.lock().unwrap().is_empty());
    }
}
