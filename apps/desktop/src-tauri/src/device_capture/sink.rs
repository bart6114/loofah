use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use hypr_loofah_client::capture_spool::{CaptureSeal, CaptureSnapshot, CaptureWriter};
use hypr_loofah_protocol::{ApplicationError, ErrorCode, MutationOutcome};
use tauri_plugin_transcription::{
    DeviceAppendReceipt, DeviceFinishReceipt, DeviceRecordingEnd, DeviceRecordingSink,
    DeviceSinkError,
};

use super::{DeviceCaptureEvent, DeviceCaptureObserver, ScopedDeviceCaptureEvent};

pub struct SpoolRecordingSink {
    writer: CaptureWriter,
    state: Mutex<SinkState>,
    operations: tokio::sync::Mutex<()>,
    observer: Arc<dyn DeviceCaptureObserver>,
}

struct SinkState {
    snapshot: CaptureSnapshot,
    end: Option<DeviceRecordingEnd>,
    lifecycle_closed: bool,
}

impl SpoolRecordingSink {
    pub(super) fn new(
        writer: CaptureWriter,
        snapshot: CaptureSnapshot,
        observer: Arc<dyn DeviceCaptureObserver>,
    ) -> Self {
        Self {
            writer,
            state: Mutex::new(SinkState {
                snapshot,
                end: None,
                lifecycle_closed: false,
            }),
            operations: tokio::sync::Mutex::new(()),
            observer,
        }
    }

    pub(super) fn emit(&self, event: DeviceCaptureEvent) {
        self.observer.emit(ScopedDeviceCaptureEvent {
            identity: self.writer.identity().clone(),
            event,
        });
    }

    pub fn seal(&self) -> Option<CaptureSeal> {
        self.state
            .lock()
            .ok()
            .and_then(|state| state.snapshot.seal.clone())
    }

    pub(super) fn interrupted(&self, reason: String) {
        if let Ok(mut state) = self.state.lock() {
            if state.lifecycle_closed {
                return;
            }
            state.lifecycle_closed = true;
        }
        self.emit(DeviceCaptureEvent::Interrupted { reason });
    }

    pub(super) fn stopped(&self, error: Option<String>) {
        let sealed = self.state.lock().ok().and_then(|mut state| {
            if state.lifecycle_closed {
                return None;
            }
            if error.is_none()
                && state.end == Some(DeviceRecordingEnd::Complete)
                && let Some(seal) = state.snapshot.seal.clone()
            {
                state.lifecycle_closed = true;
                return Some(seal);
            }
            None
        });
        if let Some(seal) = sealed {
            self.emit(DeviceCaptureEvent::Sealed(seal));
        } else {
            self.interrupted(error.unwrap_or_else(|| {
                "Device capture stopped without a complete durable seal".into()
            }));
        }
    }

    pub(super) async fn retry<T: Send, F, Fut>(&self, mut action: F) -> Result<T, DeviceSinkError>
    where
        F: FnMut() -> Fut + Send,
        Fut: std::future::Future<Output = Result<T, ApplicationError>> + Send,
    {
        let started = Instant::now();
        let mut delay = Duration::from_millis(10);
        let mut warned = false;
        loop {
            match action().await {
                Ok(result) => return Ok(result),
                Err(error)
                    if error.retryable
                        && error.code == ErrorCode::Unavailable
                        && error.mutation_outcome == Some(MutationOutcome::NotCommitted) =>
                {
                    if !warned && started.elapsed() >= Duration::from_secs(1) {
                        warned = true;
                        self.emit(DeviceCaptureEvent::Warning {
                            message:
                                "Recording storage is slow; audio remains queued on this device"
                                    .into(),
                        });
                    }
                    tokio::time::sleep(delay).await;
                    delay = (delay * 2).min(Duration::from_millis(100));
                }
                Err(error) => return Err(DeviceSinkError(error.to_string())),
            }
        }
    }
}

#[ractor::async_trait]
impl DeviceRecordingSink for SpoolRecordingSink {
    async fn append(
        &self,
        sequence: u64,
        frames: u32,
        pcm: Arc<[u8]>,
    ) -> Result<DeviceAppendReceipt, DeviceSinkError> {
        let _operation = self.operations.lock().await;
        let (expected_sequence, expected_frame, expected_offset, channels, fingerprint, ended) = {
            let state = self
                .state
                .lock()
                .map_err(|_| DeviceSinkError("Device capture state is unavailable".into()))?;
            (
                state.snapshot.chunk_count,
                state.snapshot.frames,
                state.snapshot.pcm_bytes,
                state.snapshot.format.channels.len(),
                state.snapshot.capture_fingerprint.clone(),
                state.end.is_some(),
            )
        };
        if ended
            || sequence != expected_sequence
            || frames == 0
            || pcm.len() != frames as usize * channels * 4
        {
            return Err(DeviceSinkError(
                "Device capture chunk does not match its durable prefix".into(),
            ));
        }
        let descriptor = self
            .retry(|| self.writer.append_chunk(sequence, frames, pcm.to_vec()))
            .await?;
        if descriptor.capture_fingerprint != fingerprint
            || descriptor.sequence != sequence
            || descriptor.first_frame != expected_frame
            || descriptor.pcm_offset != expected_offset
            || descriptor.frames != frames
            || descriptor.bytes as usize != pcm.len()
        {
            return Err(DeviceSinkError(
                "Device spool returned an inconsistent chunk identity".into(),
            ));
        }
        let event = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| DeviceSinkError("Device capture state is unavailable".into()))?;
            state.snapshot.chunk_count += 1;
            state.snapshot.frames += u64::from(frames);
            state.snapshot.pcm_bytes += pcm.len() as u64;
            state.snapshot.chain_digest = descriptor.digest;
            DeviceCaptureEvent::Durable {
                chunk_count: state.snapshot.chunk_count,
                frames: state.snapshot.frames,
                pcm_bytes: state.snapshot.pcm_bytes,
            }
        };
        self.emit(event);
        Ok(DeviceAppendReceipt { sequence, frames })
    }

    async fn finish(
        &self,
        end: DeviceRecordingEnd,
    ) -> Result<DeviceFinishReceipt, DeviceSinkError> {
        let _operation = self.operations.lock().await;
        let expected = {
            let state = self
                .state
                .lock()
                .map_err(|_| DeviceSinkError("Device capture state is unavailable".into()))?;
            if state.end.is_some() {
                return Err(DeviceSinkError("Device sink is already finalized".into()));
            }
            DeviceFinishReceipt {
                chunk_count: state.snapshot.chunk_count,
                frames: state.snapshot.frames,
                pcm_bytes: state.snapshot.pcm_bytes,
            }
        };
        let seal = if end == DeviceRecordingEnd::Complete {
            let seal = self.retry(|| self.writer.seal()).await?;
            if seal.chunk_count != expected.chunk_count
                || seal.frames != expected.frames
                || seal.pcm_bytes != expected.pcm_bytes
            {
                return Err(DeviceSinkError(
                    "Device seal does not match confirmed capture bytes".into(),
                ));
            }
            Some(seal)
        } else {
            None
        };
        let mut state = self
            .state
            .lock()
            .map_err(|_| DeviceSinkError("Device capture state is unavailable".into()))?;
        state.snapshot.seal = seal;
        state.end = Some(end);
        Ok(expected)
    }
}
