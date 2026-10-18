use hypr_fs_sync_core::runtime::{AudioImportEvent, AudioImportRuntime};
use tauri_specta::Event;

pub struct TauriAudioImportRuntime<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
    activity: std::sync::Mutex<
        Option<(
            std::sync::Arc<hypr_vault_access::connection::ConnectionState>,
            String,
        )>,
    >,
}

impl<R: tauri::Runtime> TauriAudioImportRuntime<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self {
            app,
            activity: std::sync::Mutex::new(None),
        }
    }
}

impl<R: tauri::Runtime> Drop for TauriAudioImportRuntime<R> {
    fn drop(&mut self) {
        if let Some((connection, id)) = self.activity.get_mut().unwrap().take() {
            connection.end_processing(&id);
        }
    }
}

impl<R: tauri::Runtime> AudioImportRuntime for TauriAudioImportRuntime<R> {
    fn prepare_commit(&self, session_id: &str) -> std::io::Result<()> {
        use tauri::Manager;
        if let Some(connection) = crate::company::connection(&self.app) {
            let id = tauri::async_runtime::block_on(connection.begin_processing());
            *self.activity.lock().unwrap() = Some((connection.clone(), id));
            return tauri::async_runtime::block_on(connection.call(
                "session_begin_audio_import",
                serde_json::json!({"session_id":session_id}),
                false,
            ))
            .map(|_| ())
            .map_err(|error| std::io::Error::other(error.to_string()));
        }
        let store = self
            .app
            .state::<std::sync::Arc<hypr_vault_write::SessionStore>>();
        tauri::async_runtime::block_on(store.begin_audio_import(session_id))
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

    fn finish_commit(&self, session_id: &str) -> std::io::Result<()> {
        use tauri::Manager;
        if let Some(connection) = crate::company::connection(&self.app) {
            let root = connection
                .scratch_base()
                .ok_or_else(|| std::io::Error::other("company vault changed"))?;
            let audio = root.join("sessions").join(session_id).join("audio.mp3");
            connection
                .enqueue_upload(session_id, &audio, "audio.mp3", true)
                .map_err(|error| std::io::Error::other(error.to_string()))?;
            return tauri::async_runtime::block_on(connection.call(
                "session_finish_audio_import",
                serde_json::json!({"session_id":session_id}),
                false,
            ))
            .map(|_| ())
            .map_err(|error| std::io::Error::other(error.to_string()));
        }

        let store = self
            .app
            .state::<std::sync::Arc<hypr_vault_write::SessionStore>>();
        tauri::async_runtime::block_on(store.finish_audio_import(session_id))
            .map_err(|e| std::io::Error::other(e.to_string()))
    }

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
