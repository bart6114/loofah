use std::{
    collections::HashMap,
    fs::File,
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use hypr_fs_sync_core::attachment::{StagedAttachment, sanitize_filename};

use crate::{SessionStore, StoreError, attachments::SavedAttachment};

pub struct SessionAttachmentStage {
    pub(crate) staged: StagedAttachment,
    pub(crate) _reservation: Reservation,
    pub(crate) store: SessionStore,
    pub(crate) session_id: String,
    pub(crate) filename: String,
    pub(crate) relative_directory: PathBuf,
}

pub(crate) struct Reservation {
    active: Arc<Mutex<HashMap<String, usize>>>,
    session: String,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut active = self.active.lock().unwrap();
        if let Some(count) = active.get_mut(&self.session) {
            *count -= 1;
            if *count == 0 {
                active.remove(&self.session);
            }
        }
    }
}

impl SessionStore {
    pub async fn stage_attachment(
        &self,
        session_id: &str,
        filename: &str,
        write: impl FnOnce(&mut File) -> io::Result<()> + Send + 'static,
    ) -> Result<Option<SessionAttachmentStage>, StoreError> {
        crate::validate_session_id(session_id)?;
        let filename = sanitize_filename(filename).map_err(io_error)?;
        let store = self.clone();
        let session_id = session_id.to_owned();
        tokio::spawn(async move {
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            if store
                .metadata_snapshot_locked(&snapshot._guard, &session_id)
                .await?
                .is_none()
            {
                return Ok(None);
            }
            let relative_directory = store
                .session_dir_locked(&snapshot._guard, &session_id)
                .await?
                .join("attachments");
            let directory = store.vault_base.join(&relative_directory);
            *store
                .attachment_stages
                .lock()
                .unwrap()
                .entry(session_id.clone())
                .or_default() += 1;
            let reservation = Reservation {
                active: store.attachment_stages.clone(),
                session: session_id.clone(),
            };
            drop(snapshot);
            // The reservation belongs to actual I/O, even after its caller abandons the future.
            tokio::task::spawn_blocking(move || {
                let staged = StagedAttachment::prepare(&directory, write).map_err(io_error)?;
                Ok(Some(SessionAttachmentStage {
                    staged,
                    _reservation: reservation,
                    store,
                    session_id,
                    filename,
                    relative_directory,
                }))
            })
            .await
            .map_err(worker_error)?
        })
        .await
        .map_err(worker_error)?
    }
}

impl SessionAttachmentStage {
    pub fn path(&self) -> &Path {
        self.staged.path()
    }

    pub async fn publish(self) -> Result<SavedAttachment, StoreError> {
        tokio::spawn(async move {
            let store = self.store.clone();
            let session = self.session_id.clone();
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            if store
                .metadata_snapshot_locked(&snapshot._guard, &session)
                .await?
                .is_none()
            {
                return Err(StoreError::Conflict("Session no longer exists".into()));
            }
            let relative_directory = store
                .session_dir_locked(&snapshot._guard, &session)
                .await?
                .join("attachments");
            if relative_directory != self.relative_directory {
                return Err(StoreError::Conflict(
                    "Session attachment directory changed".into(),
                ));
            }
            let filename = tokio::task::spawn_blocking(move || {
                let _snapshot = snapshot;
                let _reservation = self._reservation;
                self.staged
                    .publish_unique(&self.filename)
                    .map(|(_, name)| name)
                    .map_err(io_error)
            })
            .await
            .map_err(worker_error)??;
            store.notify_attachment_changed(&session, Some(&filename));
            Ok(SavedAttachment {
                relative_path: relative_directory.join(&filename),
                attachment_id: filename,
            })
        })
        .await
        .map_err(worker_error)?
    }
}

fn io_error(cause: io::Error) -> StoreError {
    StoreError::Io(cause.to_string())
}
fn worker_error(cause: tokio::task::JoinError) -> StoreError {
    StoreError::Io(format!("Attachment staging worker stopped: {cause}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Write, time::Duration};

    async fn fixture() -> (tempfile::TempDir, SessionStore) {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().into());
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Attachments","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Canonical note")
            .await
            .unwrap();
        (root, store)
    }

    #[tokio::test]
    async fn completed_stages_publish_original_bytes_without_overwriting_existing_attachments_or_notes()
     {
        let (root, store) = fixture().await;
        store
            .save_attachment("note", "image 73.png", b"Existing".to_vec())
            .await
            .unwrap();
        let stage = store
            .stage_attachment("note", "image 73.png", |file| {
                file.write_all(b"New complete file")
            })
            .await
            .unwrap()
            .unwrap();
        assert!(stage.path().exists());
        assert!(
            !root
                .path()
                .join("sessions/note/attachments/image 73 1.png")
                .exists()
        );
        let saved = stage.publish().await.unwrap();
        assert_eq!(saved.attachment_id, "image 73 1.png");
        assert_eq!(
            std::fs::read(root.path().join(saved.relative_path)).unwrap(),
            b"New complete file"
        );
        assert_eq!(
            std::fs::read(root.path().join("sessions/note/attachments/image 73.png")).unwrap(),
            b"Existing"
        );
        assert_eq!(
            std::fs::read(root.path().join("sessions/note/notes.md")).unwrap(),
            b"Canonical note"
        );
        assert!(store.attachment_stages.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn copying_does_not_hold_the_vault_write_lock_but_cancellation_keeps_session_and_relocation_reservations()
     {
        let (root, store) = fixture().await;
        let (entered, started) = tokio::sync::oneshot::channel();
        let (release, paused) = std::sync::mpsc::channel();
        let writer = store.clone();
        let request = tokio::spawn(async move {
            writer
                .stage_attachment("note", "long.pdf", move |file| {
                    file.write_all(b"first")?;
                    let _ = entered.send(());
                    paused.recv().unwrap();
                    file.write_all(b" last")
                })
                .await
        });
        started.await.unwrap();
        let snapshot = tokio::time::timeout(Duration::from_secs(1), store.lock_snapshot())
            .await
            .unwrap();
        drop(snapshot);
        assert!(matches!(
            store.freeze_for_vault_move().await,
            Err(StoreError::Conflict(_))
        ));
        assert!(matches!(
            store.delete_session("note").await,
            Err(StoreError::Conflict(_))
        ));
        request.abort();
        assert!(matches!(request.await, Err(cause) if cause.is_cancelled()));
        assert!(matches!(
            store.delete_session("note").await,
            Err(StoreError::Conflict(_))
        ));
        release.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !store.attachment_stages.lock().unwrap().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(
            std::fs::read_dir(root.path().join("sessions/note/attachments"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(store.freeze_for_vault_move().await.is_ok());
        assert!(store.delete_session("note").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn dropped_or_failed_stages_release_admission_and_remove_only_their_unpublished_bytes() {
        let (root, store) = fixture().await;
        let stage = store
            .stage_attachment("note", "a.pdf", |file| file.write_all(b"complete"))
            .await
            .unwrap()
            .unwrap();
        let path = stage.path().to_owned();
        drop(stage);
        assert!(!path.exists());
        assert!(store.attachment_stages.lock().unwrap().is_empty());
        let failed = store
            .stage_attachment("note", "full.pdf", |file| {
                file.write_all(b"partial")?;
                Err(io::Error::new(io::ErrorKind::StorageFull, "Full disk"))
            })
            .await;
        assert!(matches!(failed, Err(StoreError::Io(_))));
        assert!(store.attachment_stages.lock().unwrap().is_empty());
        assert!(
            std::fs::read_dir(root.path().join("sessions/note/attachments"))
                .unwrap()
                .next()
                .is_none()
        );
        assert!(store.freeze_for_vault_move().await.is_ok());
    }

    #[tokio::test]
    async fn missing_or_invalid_sessions_do_not_read_input_or_create_an_attachment_directory() {
        let (root, store) = fixture().await;
        assert!(
            store
                .stage_attachment("missing", "a.pdf", |_| panic!(
                    "missing session must not consume bytes"
                ))
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .stage_attachment("../private", "a.pdf", |_| panic!(
                    "invalid session must not consume bytes"
                ))
                .await
                .is_err()
        );
        assert!(!root.path().join("sessions/missing").exists());
        assert!(!root.path().join("sessions/note/attachments").exists());
    }
}
