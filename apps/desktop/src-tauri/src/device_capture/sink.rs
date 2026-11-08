use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant, SystemTime},
};

use hypr_loofah_client::capture_spool::{CaptureSeal, CaptureSnapshot, CaptureWriter};
use hypr_loofah_protocol::{ApplicationError, ErrorCode, MutationOutcome};
use tauri_plugin_transcription::{
    DeviceAppendReceipt, DeviceFinishReceipt, DeviceRecordingEnd, DeviceRecordingSink,
    DeviceSinkError,
};

use super::{
    DeviceCaptureEvent, DeviceCaptureObserver, ScopedDeviceCaptureEvent,
    manager::DeviceObservationDurability,
};

pub struct SpoolRecordingSink {
    writer: CaptureWriter,
    state: Mutex<SinkState>,
    operations: tokio::sync::Mutex<()>,
    observer: Arc<dyn DeviceCaptureObserver>,
    completion: tokio::sync::watch::Sender<SinkCompletion>,
    observation: tokio::sync::watch::Sender<DeviceObservationDurability>,
    #[cfg(test)]
    pub(super) observation_test: Mutex<ObservationTest>,
    #[cfg(test)]
    pub(super) observation_attempts: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct ObservationTest {
    pub gate: Option<Arc<tokio::sync::Semaphore>>,
    pub before: Option<ApplicationError>,
    pub after: Option<ApplicationError>,
}

#[derive(Clone, Debug)]
pub(super) enum SinkCompletion {
    Pending,
    Finished {
        end: DeviceRecordingEnd,
        receipt: DeviceFinishReceipt,
    },
    Failed {
        reason: String,
        durable_prefix: Option<DeviceFinishReceipt>,
    },
}

struct SinkState {
    snapshot: CaptureSnapshot,
    end: Option<DeviceRecordingEnd>,
    lifecycle_closed: bool,
    lifecycle_stopped: bool,
    stop_error: Option<String>,
    observation_required: bool,
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
                lifecycle_stopped: false,
                stop_error: None,
                observation_required: false,
            }),
            operations: tokio::sync::Mutex::new(()),
            observer,
            completion: tokio::sync::watch::channel(SinkCompletion::Pending).0,
            observation: tokio::sync::watch::channel(DeviceObservationDurability::NotObserved).0,
            #[cfg(test)]
            observation_test: Mutex::new(ObservationTest::default()),
            #[cfg(test)]
            observation_attempts: std::sync::atomic::AtomicUsize::new(0),
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

    pub(super) fn completion(&self) -> tokio::sync::watch::Receiver<SinkCompletion> {
        self.completion.subscribe()
    }

    pub(super) fn require_observation(&self) {
        if let Ok(mut state) = self.state.lock() {
            state.observation_required = true;
        }
    }

    pub(super) fn observation(&self) -> tokio::sync::watch::Receiver<DeviceObservationDurability> {
        self.observation.subscribe()
    }

    pub(super) fn observation_failed(&self, ms: Option<u64>, reason: String) {
        self.observation
            .send_replace(DeviceObservationDurability::Failed {
                observed_at_ms: ms,
                reason,
            });
        self.close_lifecycle();
    }

    pub(super) async fn record_observation(&self, at: SystemTime) {
        let ms = match observation_ms(at) {
            Ok(ms) => ms,
            Err(error) => {
                self.observation_failed(None, error.to_string());
                return;
            }
        };
        self.observation
            .send_replace(DeviceObservationDurability::Pending { observed_at_ms: ms });
        #[cfg(test)]
        {
            let gate = self.observation_test.lock().unwrap().gate.clone();
            if let Some(gate) = gate {
                let _permit = gate.acquire().await.unwrap();
            }
        }
        let mut delay = Duration::from_millis(10);
        let mut reconfirmed = false;
        // Metadata contention must not hold the audio append/finish mutex.
        loop {
            match self.write_observation(ms).await {
                Ok(snapshot) => {
                    let result = self.state.lock().map_err(|_| "Device capture state is unavailable".to_owned()).and_then(|mut state| {
                        if snapshot.identity != *self.writer.identity()
                            || snapshot.capture_fingerprint != state.snapshot.capture_fingerprint
                            || snapshot.format != state.snapshot.format
                            || snapshot.first_admitted_audio_observed_at_ms != Some(ms) {
                            return Err("Capture timing receipt does not match its immutable identity and observation".into());
                        }
                        state.snapshot.first_admitted_audio_observed_at_ms = Some(ms);
                        Ok(())
                    });
                    if let Err(reason) = result {
                        self.observation_failed(Some(ms), reason);
                        return;
                    }
                    self.observation
                        .send_replace(DeviceObservationDurability::Durable { observed_at_ms: ms });
                    self.close_lifecycle();
                    return;
                }
                Err(error)
                    if error.retryable
                        && error.code == ErrorCode::Unavailable
                        && error.mutation_outcome == Some(MutationOutcome::NotCommitted) => {}
                Err(error)
                    if error.mutation_outcome == Some(MutationOutcome::Unknown) && !reconfirmed =>
                {
                    reconfirmed = true;
                }
                Err(error) => {
                    self.observation_failed(
                        Some(ms),
                        format!("Capture timing was not durably confirmed: {error}"),
                    );
                    return;
                }
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_millis(100));
        }
    }

    async fn write_observation(&self, ms: u64) -> Result<CaptureSnapshot, ApplicationError> {
        #[cfg(test)]
        {
            self.observation_attempts
                .fetch_add(1, std::sync::atomic::Ordering::Release);
            let before = self.observation_test.lock().unwrap().before.take();
            if let Some(error) = before {
                return Err(error);
            }
        }
        let snapshot = self.writer.record_first_audio_observation(ms).await?;
        #[cfg(test)]
        {
            let after = self.observation_test.lock().unwrap().after.take();
            if let Some(error) = after {
                return Err(error);
            }
        }
        Ok(snapshot)
    }

    pub(super) fn durable_prefix(&self) -> Option<DeviceFinishReceipt> {
        self.state.lock().ok().map(|state| DeviceFinishReceipt {
            chunk_count: state.snapshot.chunk_count,
            frames: state.snapshot.frames,
            pcm_bytes: state.snapshot.pcm_bytes,
        })
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
        if let Ok(mut state) = self.state.lock() {
            state.lifecycle_stopped = true;
            state.stop_error = error;
        }
        self.close_lifecycle();
    }

    fn close_lifecycle(&self) {
        let event = self.state.lock().ok().and_then(|mut state| {
            if state.lifecycle_closed || !state.lifecycle_stopped {
                return None;
            }
            if state.stop_error.is_none()
                && state.end == Some(DeviceRecordingEnd::Complete)
                && let Some(seal) = state.snapshot.seal.clone()
            {
                if state.observation_required && state.snapshot.frames != 0 {
                    match self.observation.borrow().clone() {
                        DeviceObservationDurability::Durable { observed_at_ms }
                            if state.snapshot.first_admitted_audio_observed_at_ms
                                == Some(observed_at_ms) => {}
                        DeviceObservationDurability::Failed { reason, .. } => {
                            state.lifecycle_closed = true;
                            return Some(DeviceCaptureEvent::Interrupted { reason });
                        }
                        _ => return None,
                    }
                }
                state.lifecycle_closed = true;
                return Some(DeviceCaptureEvent::Sealed(seal));
            }
            state.lifecycle_closed = true;
            Some(DeviceCaptureEvent::Interrupted {
                reason: state.stop_error.clone().unwrap_or_else(|| {
                    "Device capture stopped without a complete durable seal".into()
                }),
            })
        });
        if let Some(event) = event {
            self.emit(event);
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

pub(super) fn observation_ms(at: SystemTime) -> Result<u64, DeviceSinkError> {
    at.duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| DeviceSinkError("First audio observation precedes the Unix epoch".into()))?
        .as_millis()
        .try_into()
        .map_err(|_| {
            DeviceSinkError(
                "First audio observation cannot be represented in UTC milliseconds".into(),
            )
        })
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
        let result = self.finish_durable(end).await;
        let completion = match &result {
            Ok(receipt) => SinkCompletion::Finished {
                end,
                receipt: *receipt,
            },
            Err(error) => SinkCompletion::Failed {
                reason: error.to_string(),
                durable_prefix: self.durable_prefix(),
            },
        };
        self.completion.send_if_modified(|state| {
            if !matches!(state, SinkCompletion::Pending) {
                return false;
            }
            *state = completion;
            true
        });
        result
    }
}

impl SpoolRecordingSink {
    async fn finish_durable(
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
