use crate::{
    CaptureParams, DeviceCaptureRoot, DeviceCaptureRootMsg, ListenerRuntime, OwnedLocalCaptureArgs,
    SessionStateCache,
};
use ractor::{ActorRef, registry};
use std::{future::Future, pin::Pin, sync::Arc};
use tauri::Manager;

/// Registered by the Native owner; a webview cannot provide persistence callbacks.
pub trait LocalCaptureHost: Send + Sync + 'static {
    fn start(
        &self,
        params: CaptureParams,
        request_id: String,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;
}

pub fn local_capture_display_runtime(
    app: tauri::AppHandle,
) -> Result<Arc<dyn ListenerRuntime>, crate::Error> {
    let session_state_cache = app
        .try_state::<SessionStateCache>()
        .ok_or(crate::Error::StartSessionFailed)?
        .inner()
        .clone();
    Ok(Arc::new(crate::listener::TauriRuntime {
        app,
        session_state_cache,
    }))
}

pub async fn start_owned_local_capture(
    args: OwnedLocalCaptureArgs,
) -> Result<(), crate::DeviceCaptureStartError> {
    let cell = registry::where_is(DeviceCaptureRoot::name())
        .ok_or(crate::DeviceCaptureStartError::NotDispatched)?;
    let root: ActorRef<DeviceCaptureRootMsg> = cell.into();
    match ractor::call!(root, DeviceCaptureRootMsg::StartOwnedLocalCapture, args) {
        Ok(Ok(())) => Ok(()),
        Ok(Err(hypr_transcription_core::listener::StartSessionError::SessionAlreadyRunning)) => {
            Err(crate::DeviceCaptureStartError::Rejected)
        }
        _ => Err(crate::DeviceCaptureStartError::Unconfirmed),
    }
}

pub async fn stop_owned_local_capture(session_id: String, request_id: String) -> bool {
    let Some(cell) = registry::where_is(DeviceCaptureRoot::name()) else {
        return false;
    };
    let root: ActorRef<DeviceCaptureRootMsg> = cell.into();
    ractor::call!(root, |reply| DeviceCaptureRootMsg::StopOwnedLocalCapture {
        session_id,
        request_id,
        reply
    })
    .unwrap_or(false)
}
