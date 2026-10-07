use super::{SessionStore, StoreError, WriteGuard, paths, validate_session_id};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub(crate) struct DeletedSession {
    pub trash_path: PathBuf,
}

impl SessionStore {
    pub async fn session_dir(&self, id: &str) -> Result<PathBuf, StoreError> {
        Ok(paths::validated_session_dir(id)?)
    }

    pub(crate) async fn session_dir_locked(
        &self,
        _guard: &WriteGuard<'_>,
        id: &str,
    ) -> Result<PathBuf, StoreError> {
        if self.deleted_sessions.lock().unwrap().contains(id) {
            return Err(StoreError::Io(format!("session {id} was deleted")));
        }
        self.session_dir(id).await
    }

    pub fn session_id_for_relative_path(&self, relative: &Path) -> Option<String> {
        let mut components = relative.strip_prefix("sessions").ok()?.components();
        let id = components.next()?.as_os_str().to_str()?;
        validate_session_id(id).ok()?;
        Some(id.to_string())
    }

    pub fn is_recording(&self, id: &str) -> bool {
        self.active_recordings.lock().unwrap().contains_key(id)
    }

    pub fn note_recording_active(&self, id: &str) {
        self.active_recordings
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_insert(1);
    }

    /// Count reservations so a failed duplicate start releases only its own attempt.
    /// The write lock also serializes acquisition with whole-vault relocation.
    pub async fn prepare_recording(&self, id: &str) -> Result<PathBuf, StoreError> {
        let _audio_guard = self.lock_session_audio(id).await?;
        let dir = self.session_dir(id).await?;
        let guard = self.lock_writes().await;
        self.ensure_ready()?;
        self.read_meta(id).await?;
        if self.deleted_sessions.lock().unwrap().contains(id) {
            return Err(StoreError::Io(format!("session {id} was deleted")));
        }
        let has_audio = ["audio.mp3", "audio.wav", "audio.ogg"]
            .iter()
            .any(|name| self.vault_base.join(&dir).join(name).exists());
        if has_audio {
            self.resolve_session_audio_locked(&guard, id).await?;
        } else {
            let mut meta = self
                .read_meta(id)
                .await?
                .ok_or_else(|| StoreError::Io("session not found".into()))?;
            if meta
                .extra
                .get("audio_import_pending")
                .and_then(|v| v.as_bool())
                == Some(true)
            {
                return Err(StoreError::Conflict("audio import is pending".into()));
            }
            meta.extra.insert(
                "audio".into(),
                serde_json::json!({"source": "recording", "layout": "mic_system"}),
            );
            self.write_meta_locked(&guard, &meta).await?;
        }
        *self
            .active_recordings
            .lock()
            .unwrap()
            .entry(id.to_string())
            .or_insert(0) += 1;
        Ok(dir)
    }

    pub async fn release_recording_prepare(&self, id: &str) -> Result<(), StoreError> {
        validate_session_id(id)?;
        let _guard = self.lock_writes().await;
        self.release_recording_reservation(id);
        Ok(())
    }

    pub(super) fn release_recording_reservation(&self, id: &str) {
        let mut reservations = self.active_recordings.lock().unwrap();
        match reservations.get_mut(id) {
            Some(count) if *count > 1 => *count -= 1,
            Some(_) => {
                reservations.remove(id);
            }
            None => {}
        }
    }
}
