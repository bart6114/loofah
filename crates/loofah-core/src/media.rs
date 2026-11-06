use std::sync::Arc;

use hypr_loofah_protocol::{ApplicationError, AudioFileMetadata, ErrorCode};
use hypr_vault_write::media::{OpenedRecording, RecordingSnapshot};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn audio_metadata(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<Option<AudioFileMetadata>, ApplicationError> {
        self.read_recording(session_id, |source| {
            recording_with_metadata(source).map(|(_, metadata, _)| metadata)
        })
        .await
    }

    pub(crate) async fn read_recording<T, F>(
        self: &Arc<Self>,
        session_id: &str,
        read: F,
    ) -> Result<Option<T>, ApplicationError>
    where
        T: Send + 'static,
        F: FnOnce(OpenedRecording) -> Result<T, ApplicationError> + Send + 'static,
    {
        self.validate(session_id)?;
        let permit = self
            .media_readers
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                error(
                    ErrorCode::Unavailable,
                    "Recording readers are busy; retry shortly",
                )
            })?;
        let context = self.clone();
        let id = session_id.to_owned();
        tokio::spawn(async move {
            let source = context
                .store
                .open_recording(&id)
                .await
                .map_err(|cause| storage_error(cause, false))?
                .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))?;
            let source = match source {
                RecordingSnapshot::Absent => return Ok(None),
                RecordingSnapshot::Busy => {
                    return Err(error(
                        ErrorCode::Unavailable,
                        "Recording is not ready for playback",
                    ));
                }
                RecordingSnapshot::File(source) => source,
            };
            // Retain admission and vault ownership through blocking work after caller cancellation.
            tokio::task::spawn_blocking(move || {
                let _context = context;
                let _permit = permit;
                read(source).map(Some)
            })
            .await
            .map_err(|_| error(ErrorCode::Internal, "Recording reader stopped"))?
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Recording request stopped"))?
    }
}

pub(crate) fn recording_with_metadata(
    mut source: OpenedRecording,
) -> Result<(OpenedRecording, AudioFileMetadata, std::fs::Metadata), ApplicationError> {
    let before = source.file.metadata().map_err(media_error)?;
    let metadata =
        hypr_fs_sync_core::audio::metadata_from_reader(source.filename, &mut source.file)
            .map_err(media_error)?
            .ok_or_else(|| error(ErrorCode::Internal, "Unsupported recording format"))?;
    let after = source.file.metadata().map_err(media_error)?;
    if before.len() != after.len()
        || before.modified().ok() != after.modified().ok()
        || metadata.size_bytes != after.len()
    {
        return Err(error(
            ErrorCode::Unavailable,
            "Recording changed while its metadata was read",
        ));
    }
    Ok((source, metadata, after))
}

fn media_error(cause: std::io::Error) -> ApplicationError {
    tracing::error!(%cause, "recording metadata read failed");
    error(ErrorCode::Internal, "Cannot read recording metadata")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_vault_write::SessionStore;

    async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>) {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(root.path().into()));
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Title","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Notes remain unchanged")
            .await
            .unwrap();
        let context = Arc::new(VaultContext::new(
            "vault".into(),
            store,
            StartupState::new(Some(root.path())),
            Arc::new(ConfigState::load_or_default(root.path())),
            Arc::new(SearchService::default()),
        ));
        (root, context)
    }

    #[tokio::test]
    async fn shared_metadata_reuses_portable_hashing_and_keeps_paths_out_of_the_result() {
        let (root, context) = fixture().await;
        assert!(context.audio_metadata("note").await.unwrap().is_none());
        assert_eq!(
            context.audio_metadata("missing").await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            context.audio_metadata("../note").await.unwrap_err().code,
            ErrorCode::Validation
        );
        context.store.set_startup_pending(true);
        assert_eq!(
            context.audio_metadata("note").await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        context.store.set_startup_pending(false);
        let directory = root.path().join("sessions/note");
        std::fs::write(directory.join("audio.mp3"), b"Canonical recording bytes").unwrap();
        let metadata = context.audio_metadata("note").await.unwrap();
        assert_eq!(
            metadata,
            hypr_fs_sync_core::audio::metadata(&directory).unwrap()
        );
        let serialized = serde_json::to_string(&metadata).unwrap();
        assert!(!serialized.contains(&root.path().to_string_lossy().to_string()));
        std::fs::write(directory.join("audio.mp3"), b"A different recording").unwrap();
        assert_ne!(context.audio_metadata("note").await.unwrap(), metadata);
        context.store.note_recording_active("note");
        let busy = context.audio_metadata("note").await.unwrap_err();
        assert_eq!(busy.code, ErrorCode::Unavailable);
        assert!(busy.retryable);
        assert_eq!(
            std::fs::read_to_string(directory.join("notes.md")).unwrap(),
            "Notes remain unchanged"
        );
    }

    #[tokio::test]
    async fn cancelled_media_processing_retains_admission_and_ownership_but_releases_the_write_lock()
     {
        let (root, context) = fixture().await;
        std::fs::write(root.path().join("sessions/note/audio.mp3"), b"Recording").unwrap();
        let store = context.store.clone();
        let weak = Arc::downgrade(&context);
        let (started, waiting) = tokio::sync::oneshot::channel();
        let (finish, completed) = std::sync::mpsc::channel();
        let caller = context.clone();
        let request = tokio::spawn(async move {
            caller
                .read_recording("note", move |source| {
                    assert_eq!(source.filename, "audio.mp3");
                    started.send(()).unwrap();
                    completed
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                    Ok(())
                })
                .await
        });
        waiting.await.unwrap();
        assert_eq!(context.media_readers.available_permits(), 3);
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(context.media_readers.available_permits(), 3);
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.save_note("note", "Editing is responsive", None),
        )
        .await
        .unwrap()
        .unwrap();
        drop(context);
        assert!(
            weak.upgrade().is_some(),
            "cancelled work lost vault ownership before finishing"
        );
        finish.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn media_admission_is_bounded_and_busy_requests_do_not_queue_background_work() {
        let (root, context) = fixture().await;
        std::fs::write(root.path().join("sessions/note/audio.wav"), b"Recording").unwrap();
        let mut requests = Vec::new();
        let mut finishers = Vec::new();
        for _ in 0..4 {
            let (started, waiting) = tokio::sync::oneshot::channel();
            let (finish, completed) = std::sync::mpsc::channel();
            finishers.push(finish);
            let caller = context.clone();
            requests.push(tokio::spawn(async move {
                caller
                    .read_recording("note", move |_| {
                        started.send(()).unwrap();
                        completed
                            .recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap();
                        Ok(())
                    })
                    .await
            }));
            waiting.await.unwrap();
        }
        assert_eq!(context.media_readers.available_permits(), 0);
        let busy = context.audio_metadata("note").await.unwrap_err();
        assert_eq!(busy.code, ErrorCode::Unavailable);
        assert!(busy.retryable);
        for finish in finishers {
            finish.send(()).unwrap();
        }
        for request in requests {
            assert_eq!(request.await.unwrap().unwrap(), Some(()));
        }
        assert_eq!(context.media_readers.available_permits(), 4);
        assert!(context.audio_metadata("note").await.unwrap().is_some());
    }
}
