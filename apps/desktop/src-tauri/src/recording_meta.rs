//! Stamps `_meta.json`'s `started_at`/`ended_at` from the capture lifecycle. Sessions are
//! created with both null and no frontend code ever patches them, so without this bridge
//! every real recording kept null timestamps forever. Riding the same
//! `CaptureLifecycleEvent` the frontend consumes keeps the file write on the Rust side:
//! `Started` fires when capture actually begins, `Stopped` when the session supervisor
//! winds down -- including after an abnormal supervisor exit, which also ends there.

use std::sync::Arc;

use tauri::{AppHandle, Manager};
use tauri_plugin_transcription::CaptureLifecycleEvent;
use tauri_specta::Event;

use crate::session_store::SessionStore;

pub fn spawn(app: AppHandle) {
    // One serialized worker applies the stamps in event order. Independent spawned
    // tasks could invert an instant Started/Stopped pair: the Stopped stamp's lease
    // clear would run first and the late Started stamp would re-insert a lease
    // nothing ever releases, blocking whole-vault relocation until the next recording or restart.
    let (stamps_tx, mut stamps_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, bool, String)>();
    let worker_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some((session_id, is_end, at)) = stamps_rx.recv().await {
            let store = worker_handle
                .try_state::<Arc<SessionStore>>()
                .map(|state| state.inner().clone());
            match &store {
                Some(store) => {
                    let result = if is_end {
                        store.mark_recording_ended(&session_id, &at).await
                    } else {
                        store.mark_recording_started(&session_id, &at).await
                    };
                    if let Err(error) = &result {
                        tracing::warn!(
                            %session_id,
                            %error,
                            "failed to stamp recording timestamp in _meta.json"
                        );
                    }
                }
                None => {
                    tracing::warn!(
                        %session_id,
                        "session store is not managed; recording timestamps will stay null"
                    );
                }
            };
        }
    });

    let handle = app.clone();
    CaptureLifecycleEvent::listen(&app, move |event| {
        let (session_id, is_end) = match event.payload {
            CaptureLifecycleEvent::Started { session_id, .. } => (session_id, false),
            CaptureLifecycleEvent::Stopped { session_id, .. } => (session_id, true),
            CaptureLifecycleEvent::Finalizing { .. } => return,
        };

        // Protect whole-vault relocation before the asynchronous stamp is queued.
        if !is_end && let Some(store) = handle.try_state::<Arc<SessionStore>>() {
            store.note_recording_active(&session_id);
        }

        // Stamped here rather than inside the worker: the event marks the actual
        // lifecycle moment, the store write merely persists it.
        let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let _ = stamps_tx.send((session_id, is_end, at));
    });
}
