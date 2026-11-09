use std::sync::{Arc, Weak};

use hypr_loofah_core::{
    LocalSessionLifecycleHost, VaultContext,
    local_session_work::{LocalSessionWorkObserver, LocalSessionWorkRegistry},
};
use hypr_loofah_protocol::{ApplicationError, ErrorCode};
use tauri::Manager;
use tauri_plugin_transcription::{BatchSessionRegistry, CaptureMaintenanceGate};

struct NativeLifecycleHost {
    context: Weak<VaultContext>,
    work: LocalSessionWorkObserver,
    capture: CaptureMaintenanceGate,
    batches: Arc<BatchSessionRegistry>,
}

pub(crate) fn configure_lifecycle<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    context: &Arc<VaultContext>,
) -> Result<(), ApplicationError> {
    let work = app
        .try_state::<LocalSessionWorkRegistry>()
        .ok_or_else(unavailable)?
        .downgrade();
    let capture = app
        .try_state::<CaptureMaintenanceGate>()
        .ok_or_else(unavailable)?
        .inner()
        .clone();
    let batches = app
        .try_state::<Arc<BatchSessionRegistry>>()
        .ok_or_else(unavailable)?
        .inner()
        .clone();
    context.configure_local_session_lifecycle_host(Arc::new(NativeLifecycleHost {
        context: Arc::downgrade(context),
        work,
        capture,
        batches,
    }))
}

impl LocalSessionLifecycleHost for NativeLifecycleHost {
    fn admit<'a>(
        &'a self,
        context: &'a Arc<VaultContext>,
        session: &'a str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<Output = Result<Box<dyn Send + Sync>, ApplicationError>>
                + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            if !self
                .context
                .upgrade()
                .is_some_and(|original| Arc::ptr_eq(&original, context))
            {
                return Err(unavailable());
            }
            self.work.ensure_idle_for(context, session)?;
            let batches = self
                .batches
                .admit_session_maintenance()
                .map_err(|_| unavailable())?;
            let capture =
                tauri_plugin_transcription::admit_session_maintenance(self.capture.clone())
                    .await
                    .map_err(|_| unavailable())?;
            self.work.ensure_idle_for(context, session)?;
            Ok(Box::new((batches, capture)) as Box<dyn Send + Sync>)
        })
    }
}

fn unavailable() -> ApplicationError {
    hypr_loofah_client::failure(
        ErrorCode::Conflict,
        "Local capture and persistence could not be confirmed idle",
    )
}
