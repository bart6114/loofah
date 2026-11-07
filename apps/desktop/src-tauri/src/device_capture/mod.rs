//! Device-only capture. No server admission, canonical vault writes or uploader lives here.

mod runtime;
mod sink;
#[cfg(test)]
mod tests;

use std::sync::{Arc, OnceLock};

use hypr_loofah_client::capture_spool::{
    CaptureChannel, CaptureFormat, CaptureIdentity, CaptureSeal, CaptureSpool,
};
use tauri_plugin_transcription::{
    DeviceCaptureArgs, DeviceCaptureKey, DeviceChannelMode, DeviceSinkError,
};

pub use sink::SpoolRecordingSink;

#[derive(Clone, Debug)]
pub struct ScopedDeviceCaptureEvent {
    pub identity: CaptureIdentity,
    pub event: DeviceCaptureEvent,
}

#[derive(Clone, Debug)]
pub enum DeviceCaptureEvent {
    Preparing,
    Capturing {
        device: Option<String>,
    },
    Finalizing,
    Durable {
        chunk_count: u64,
        frames: u64,
        pcm_bytes: u64,
    },
    Sealed(CaptureSeal),
    Interrupted {
        reason: String,
    },
    Warning {
        message: String,
    },
    Amplitude {
        mic: u16,
        speaker: u16,
    },
}

pub trait DeviceCaptureObserver: Send + Sync {
    fn emit(&self, event: ScopedDeviceCaptureEvent);
}

pub struct PreparedDeviceCapture {
    pub key: DeviceCaptureKey,
    args: DeviceCaptureArgs,
    pub sink: Arc<SpoolRecordingSink>,
}

impl PreparedDeviceCapture {
    pub async fn prepare(
        spool: Arc<CaptureSpool>,
        identity: CaptureIdentity,
        format: CaptureFormat,
        observer: Arc<dyn DeviceCaptureObserver>,
    ) -> Result<Self, DeviceSinkError> {
        let mode = match format.channels.as_slice() {
            [CaptureChannel::Microphone, CaptureChannel::SystemAudio] => {
                DeviceChannelMode::MicAndSpeaker
            }
            [CaptureChannel::Microphone] => DeviceChannelMode::MicOnly,
            [CaptureChannel::SystemAudio] => DeviceChannelMode::SpeakerOnly,
            _ => {
                return Err(DeviceSinkError(
                    "Device capture requires an explicit native channel layout".into(),
                ));
            }
        };
        static ADMISSION: OnceLock<(Arc<tokio::sync::Semaphore>, Arc<tokio::sync::Semaphore>)> =
            OnceLock::new();
        let (pending, serial) = ADMISSION.get_or_init(|| {
            (
                Arc::new(tokio::sync::Semaphore::new(8)),
                Arc::new(tokio::sync::Semaphore::new(1)),
            )
        });
        let _pending = pending
            .clone()
            .try_acquire_owned()
            .map_err(|_| DeviceSinkError("Too many device recordings are being prepared".into()))?;
        let _admission = serial
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| DeviceSinkError("Device recording admission is unavailable".into()))?;
        // Even a zero-chunk abandoned manifest may have had pre-crash audio buffered in memory.
        if spool
            .inventory(identity.scope.clone())
            .await
            .map_err(|error| DeviceSinkError(error.to_string()))?
            .iter()
            .any(|snapshot| snapshot.identity.capture_id == identity.capture_id)
        {
            return Err(DeviceSinkError(
                "Recover the previous capture instead of restarting its identity".into(),
            ));
        }
        let writer = spool
            .begin(identity, format)
            .await
            .map_err(|error| DeviceSinkError(error.to_string()))?;
        let snapshot = writer
            .snapshot()
            .await
            .map_err(|error| DeviceSinkError(error.to_string()))?;
        if snapshot.chunk_count != 0
            || snapshot.seal.is_some()
            || snapshot.acknowledged.is_some()
            || snapshot.publication.is_some()
        {
            return Err(DeviceSinkError(
                "An existing recording cannot resume capture across a device gap".into(),
            ));
        }
        let key = DeviceCaptureKey::new(snapshot.capture_fingerprint.clone())?;
        let sink = Arc::new(SpoolRecordingSink::new(writer, snapshot, observer));
        let runtime = Arc::new(runtime::DeviceRuntime::new(sink.clone()));
        sink.emit(DeviceCaptureEvent::Preparing);
        Ok(Self {
            key: key.clone(),
            args: DeviceCaptureArgs {
                key,
                sink: sink.clone(),
                mode,
                runtime,
            },
            sink,
        })
    }

    pub fn into_args(self) -> DeviceCaptureArgs {
        self.args
    }

    pub async fn start(self) -> Result<DeviceCaptureKey, DeviceSinkError> {
        let key = self.key.clone();
        let sink = self.sink.clone();
        tauri_plugin_transcription::start_device_capture(self.args)
            .await
            .map_err(|error| {
                sink.interrupted(format!("Device capture failed to start: {error}"));
                DeviceSinkError(error.to_string())
            })?;
        Ok(key)
    }
}
