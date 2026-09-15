use hypr_fs_sync_core::runtime::{AudioImportEvent, AudioImportRuntime};
use tauri_specta::Event;

pub struct TauriAudioImportRuntime<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> TauriAudioImportRuntime<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: tauri::Runtime> AudioImportRuntime for TauriAudioImportRuntime<R> {
    fn emit(&self, event: AudioImportEvent) {
        use tauri::Manager;
        if let AudioImportEvent::Completed { session_id, .. } = &event
            && let Some(store) = self
                .app
                .try_state::<std::sync::Arc<hypr_vault_write::SessionStore>>()
        {
            store.notify_artifacts_changed(session_id);
        }
        let _ = event.emit(&self.app);
    }
}
