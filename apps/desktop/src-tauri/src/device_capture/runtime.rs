use std::{path::PathBuf, sync::Arc};

use tauri_plugin_transcription::{
    DeviceListenerRuntime, DeviceSessionDataEvent, DeviceSessionErrorEvent,
    DeviceSessionLifecycleEvent, DeviceSessionProgressEvent,
};

use super::{DeviceCaptureEvent, SpoolRecordingSink};

pub(super) struct DeviceRuntime {
    sink: Arc<SpoolRecordingSink>,
}
impl DeviceRuntime {
    pub(super) fn new(sink: Arc<SpoolRecordingSink>) -> Self {
        Self { sink }
    }
}

impl hypr_storage::StorageRuntime for DeviceRuntime {
    fn global_base(&self) -> Result<PathBuf, hypr_storage::Error> {
        Err(hypr_storage::Error::DataDirUnavailable)
    }
    fn vault_base(&self) -> Result<PathBuf, hypr_storage::Error> {
        Err(hypr_storage::Error::DataDirUnavailable)
    }
}

impl DeviceListenerRuntime for DeviceRuntime {
    fn emit_lifecycle(&self, event: DeviceSessionLifecycleEvent) {
        match event {
            DeviceSessionLifecycleEvent::Active { .. } => {}
            DeviceSessionLifecycleEvent::Finalizing { .. } => {
                self.sink.emit(DeviceCaptureEvent::Finalizing)
            }
            DeviceSessionLifecycleEvent::Inactive { error, .. } => self.sink.stopped(error),
        }
    }
    fn emit_progress(&self, event: DeviceSessionProgressEvent) {
        if let DeviceSessionProgressEvent::AudioReady { device, .. } = event {
            self.sink.emit(DeviceCaptureEvent::Capturing { device });
        }
    }
    fn emit_error(&self, event: DeviceSessionErrorEvent) {
        let (message, fatal) = match event {
            DeviceSessionErrorEvent::AudioError {
                error, is_fatal, ..
            } => (error, is_fatal),
            DeviceSessionErrorEvent::ConnectionError { error, .. } => (error, true),
        };
        if fatal {
            self.sink.interrupted(message);
        } else {
            self.sink.emit(DeviceCaptureEvent::Warning { message });
        }
    }
    fn emit_data(&self, event: DeviceSessionDataEvent) {
        if let DeviceSessionDataEvent::AudioAmplitude { mic, speaker, .. } = event {
            self.sink
                .emit(DeviceCaptureEvent::Amplitude { mic, speaker });
        }
    }
}
