use hypr_vault_read::{AudioLayout, AudioSource, SessionAudio};

use super::{SessionMeta, SessionStore, StoreError, WriteGuard};

impl SessionStore {
    pub async fn lock_session_audio(
        &self,
        id: &str,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, StoreError> {
        super::validate_session_id(id)?;
        let _writes = self.lock_writes().await;
        self.ensure_ready()?;
        let lock = self
            .audio_operations
            .lock()
            .unwrap()
            .entry(id.into())
            .or_default()
            .clone();
        lock.try_lock_owned().map_err(|_| {
            StoreError::Conflict("another audio operation is running for this session".into())
        })
    }

    pub async fn resolve_session_audio(&self, id: &str) -> Result<SessionAudio, StoreError> {
        let guard = self.lock_writes().await;
        self.resolve_session_audio_locked(&guard, id).await
    }

    pub(crate) async fn resolve_session_audio_locked(
        &self,
        guard: &WriteGuard<'_>,
        id: &str,
    ) -> Result<SessionAudio, StoreError> {
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
            return Err(StoreError::Io("Audio import was interrupted. Import the audio again before transcribing or recording.".into()));
        }
        if let Some(value) = meta.extra.get("audio") {
            return serde_json::from_value(value.clone())
                .map_err(|e| StoreError::Serialize(format!("unsupported audio metadata: {e}")));
        }
        let vault = self.vault_base.clone();
        let dir = self.session_dir(id).await?;
        let transcripts = tokio::task::spawn_blocking(move || {
            hypr_vault_read::transcript::read_transcript_json_in(&vault, &dir)
        })
        .await
        .map_err(|e| StoreError::Io(e.to_string()))??;
        let words = transcripts.transcripts.iter().flat_map(|t| &t.words);
        let sources: Vec<_> = words
            .map(|w| {
                w.metadata
                    .as_ref()
                    .and_then(|m| m.get("capture_source"))
                    .and_then(|v| v.as_str())
            })
            .collect();
        let audio = infer_audio(&meta, &sources);
        meta.extra
            .insert("audio".into(), serde_json::to_value(audio).unwrap());
        self.write_meta_locked(guard, &meta).await?;
        Ok(audio)
    }

    pub async fn begin_audio_import(&self, id: &str) -> Result<(), StoreError> {
        let guard = self.lock_writes().await;
        if self.is_recording(id) {
            return Err(StoreError::Conflict(
                "cannot import audio while recording".into(),
            ));
        }
        let mut meta = self
            .read_meta(id)
            .await?
            .ok_or_else(|| StoreError::Io("session not found".into()))?;
        meta.extra
            .insert("audio_import_pending".into(), true.into());
        self.write_meta_locked(&guard, &meta).await
    }

    pub async fn finish_audio_import(&self, id: &str) -> Result<(), StoreError> {
        let guard = self.lock_writes().await;
        let mut meta = self
            .read_meta(id)
            .await?
            .ok_or_else(|| StoreError::Io("session not found".into()))?;
        meta.extra.insert(
            "audio".into(),
            serde_json::to_value(SessionAudio {
                source: AudioSource::Import,
                layout: AudioLayout::Mixed,
            })
            .unwrap(),
        );
        meta.extra.remove("audio_import_pending");
        self.write_meta_locked(&guard, &meta).await
    }

    pub async fn clear_audio_metadata(&self, id: &str) -> Result<(), StoreError> {
        let guard = self.lock_writes().await;
        let mut meta = self
            .read_meta(id)
            .await?
            .ok_or_else(|| StoreError::Io("session not found".into()))?;
        meta.extra.remove("audio");
        meta.extra.remove("audio_import_pending");
        self.write_meta_locked(&guard, &meta).await
    }
}

fn infer_audio(meta: &SessionMeta, sources: &[Option<&str>]) -> SessionAudio {
    if sources.contains(&Some("import")) {
        SessionAudio {
            source: AudioSource::Import,
            layout: AudioLayout::Mixed,
        }
    } else if (!sources.is_empty() && sources.iter().all(|s| *s == Some("recording")))
        || (sources.is_empty() && meta.started_at.is_some())
    {
        SessionAudio {
            source: AudioSource::Recording,
            layout: AudioLayout::MicSystem,
        }
    } else {
        SessionAudio::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (SessionStore, tempfile::TempDir) {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().into());
        let meta: SessionMeta = serde_json::from_value(serde_json::json!({
            "id":"audio-test", "title":"Synthetic", "created_at":"2026-01-01T00:00:00Z", "tags":[], "custom":"preserve"
        })).unwrap();
        store.create_session_meta(&meta).await.unwrap();
        (store, vault)
    }

    #[tokio::test]
    async fn import_survives_restart_transcript_deletion_and_resume() {
        let (store, vault) = fixture().await;
        store.begin_audio_import("audio-test").await.unwrap();
        assert!(store.resolve_session_audio("audio-test").await.is_err());
        assert!(store.prepare_recording("audio-test").await.is_err());
        store.finish_audio_import("audio-test").await.unwrap();
        std::fs::write(
            vault.path().join("sessions/audio-test/audio.mp3"),
            b"synthetic placeholder",
        )
        .unwrap();
        let restarted = SessionStore::new(vault.path().into());
        assert_eq!(
            restarted
                .resolve_session_audio("audio-test")
                .await
                .unwrap()
                .source,
            AudioSource::Import
        );
        restarted.prepare_recording("audio-test").await.unwrap();
        restarted
            .mark_recording_started("audio-test", "2026-01-02T00:00:00Z")
            .await
            .unwrap();
        assert_eq!(
            restarted
                .resolve_session_audio("audio-test")
                .await
                .unwrap()
                .layout,
            AudioLayout::Mixed
        );
        assert_eq!(
            restarted
                .read_meta("audio-test")
                .await
                .unwrap()
                .unwrap()
                .extra["custom"],
            "preserve"
        );
    }

    #[tokio::test]
    async fn fresh_capture_and_deleted_audio_establish_new_layout() {
        let (store, _vault) = fixture().await;
        assert_eq!(
            store.resolve_session_audio("audio-test").await.unwrap(),
            SessionAudio::default()
        );
        store.prepare_recording("audio-test").await.unwrap();
        assert_eq!(
            store
                .resolve_session_audio("audio-test")
                .await
                .unwrap()
                .layout,
            AudioLayout::MicSystem
        );
        store.release_recording_prepare("audio-test").await.unwrap();
        store.clear_audio_metadata("audio-test").await.unwrap();
        store.finish_audio_import("audio-test").await.unwrap();
        assert_eq!(
            store
                .resolve_session_audio("audio-test")
                .await
                .unwrap()
                .source,
            AudioSource::Import
        );
    }

    #[tokio::test]
    async fn audio_operations_exclude_capture_and_each_other() {
        let (store, _vault) = fixture().await;
        let guard = store.lock_session_audio("audio-test").await.unwrap();
        assert!(store.lock_session_audio("audio-test").await.is_err());
        assert!(store.prepare_recording("audio-test").await.is_err());
        drop(guard);
        assert!(store.prepare_recording("audio-test").await.is_ok());
        assert!(store.begin_audio_import("audio-test").await.is_err());
    }

    #[tokio::test]
    async fn legacy_provenance_overrides_recording_timestamps() {
        let (store, _vault) = fixture().await;
        let mut meta = store.read_meta("audio-test").await.unwrap().unwrap();
        meta.started_at = Some("2026-01-02T00:00:00Z".into());
        assert_eq!(
            infer_audio(&meta, &[Some("import"), Some("recording")]).layout,
            AudioLayout::Mixed
        );
        assert_eq!(
            infer_audio(&meta, &[Some("recording")]).layout,
            AudioLayout::MicSystem
        );
        assert_eq!(infer_audio(&meta, &[]).layout, AudioLayout::MicSystem);
        assert_eq!(infer_audio(&meta, &[None]).layout, AudioLayout::Mixed);
        meta.extra.insert(
            "audio".into(),
            serde_json::json!({"source":"import", "layout":"future_layout"}),
        );
        store.write_meta(&meta).await.unwrap();
        assert!(store.resolve_session_audio("audio-test").await.is_err());
    }
}
