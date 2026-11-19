use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{IndexEntity, SessionMeta, SessionStore, StoreError, paths};

const CAPTURE_DIGEST: &str = "iphone_capture_digest";
const CAPTURE_STAGE: &str = "iphone_capture_processing";

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStage {
    Transcribe,
    Summarize,
}

impl SessionStore {
    pub async fn commit_iphone_capture(
        &self,
        mut meta: SessionMeta,
        note: String,
        audio: Option<PathBuf>,
        digest: String,
    ) -> Result<(), StoreError> {
        let id = &meta.id;
        if uuid::Uuid::parse_str(id)
            .map(|uuid| uuid.to_string())
            .as_ref()
            != Ok(id)
        {
            return Err(StoreError::Serialize("invalid iPhone capture ID".into()));
        }
        let _audio_lease = self.lock_session_audio(id).await?;
        if audio.is_some() {
            meta.extra.insert(
                "audio".into(),
                serde_json::json!({"source": "import", "layout": "mixed"}),
            );
            meta.extra.insert(
                CAPTURE_STAGE.into(),
                serde_json::json!(CaptureStage::Transcribe),
            );
        }
        meta.extra
            .insert(CAPTURE_DIGEST.into(), digest.clone().into());

        let dir = self.session_dir(id).await?;
        let vault = self.vault_base.clone();
        let metadata = serde_json::to_vec_pretty(&meta)
            .map_err(|error| StoreError::Serialize(error.to_string()))?;
        let stage = vault.join("sessions").join(format!(".iphone-capture-{id}"));
        let note_for_index = crate::strip_leading_frontmatter(note.clone());

        let guard = self.lock_writes().await;
        self.ensure_ready()?;
        self.session_dir_locked(&guard, id).await?;
        if let Some(existing) = self.read_meta(id).await? {
            if existing
                .extra
                .get(CAPTURE_DIGEST)
                .and_then(|value| value.as_str())
                == Some(digest.as_str())
            {
                return Ok(());
            }
            return Err(StoreError::Conflict(
                "iPhone capture ID is already in use".into(),
            ));
        }
        drop(guard);

        let staged = stage.clone();
        let sessions = vault.join("sessions");
        tokio::task::spawn_blocking(move || {
            fs::metadata(&vault)?;
            fs::create_dir(&sessions).or_else(ignore_exists)?;
            fs::create_dir(&staged).or_else(ignore_exists)?;
            if !fs::symlink_metadata(&staged)?.is_dir() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "invalid capture staging directory",
                ));
            }
            let result = stage_files(&staged, &metadata, &note, audio.as_deref());
            if result.is_err() {
                let _ = fs::remove_dir_all(&staged);
            }
            result
        })
        .await
        .map_err(io_error)?
        .map_err(io_error)?;

        let guard = self.lock_writes().await;
        let commit = async {
            self.ensure_ready()?;
            self.session_dir_locked(&guard, id).await?;
            let target = self.vault_base.join(dir);
            let staged = stage.clone();
            tokio::task::spawn_blocking(move || {
                if target.symlink_metadata().is_ok() {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::AlreadyExists,
                        "session directory already exists",
                    ));
                }
                fs::rename(staged, &target)?;
                File::open(target.parent().expect("session has a parent"))?.sync_all()
            })
            .await
            .map_err(io_error)?
            .map_err(io_error)
        }
        .await;
        if let Err(error) = commit {
            let _ = tokio::task::spawn_blocking(move || fs::remove_dir_all(stage)).await;
            return Err(error);
        }
        self.index_set_note_and_meta(&meta, note_for_index);
        self.notify_index_changed(IndexEntity::Sessions, vec![id.clone()]);
        self.notify_artifacts_changed(id);
        Ok(())
    }

    pub fn iphone_capture_jobs(&self) -> Vec<(String, CaptureStage)> {
        let index = self.index.read().unwrap();
        let mut jobs: Vec<_> = index
            .sessions
            .values()
            .filter_map(|entry| {
                let stage =
                    serde_json::from_value(entry.meta.extra.get(CAPTURE_STAGE)?.clone()).ok()?;
                Some((entry.meta.created_at.as_str(), entry.meta.id.clone(), stage))
            })
            .collect();
        jobs.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
        jobs.into_iter().map(|(_, id, stage)| (id, stage)).collect()
    }

    pub fn is_iphone_capture(&self, id: &str) -> bool {
        self.index
            .read()
            .unwrap()
            .sessions
            .get(id)
            .is_some_and(|entry| entry.meta.extra.contains_key(CAPTURE_DIGEST))
    }

    pub async fn advance_iphone_capture(
        &self,
        id: &str,
        expected: CaptureStage,
    ) -> Result<(), StoreError> {
        let guard = self.lock_writes().await;
        self.ensure_ready()?;
        let Some(mut meta) = self.read_meta(id).await? else {
            return Ok(());
        };
        if meta.extra.get(CAPTURE_STAGE) != Some(&serde_json::json!(expected)) {
            return Ok(());
        }
        match expected {
            CaptureStage::Transcribe => {
                meta.extra.insert(
                    CAPTURE_STAGE.into(),
                    serde_json::json!(CaptureStage::Summarize),
                );
            }
            CaptureStage::Summarize => {
                meta.extra.remove(CAPTURE_STAGE);
            }
        }
        self.write_meta_locked(&guard, &meta).await
    }
}

fn stage_files(
    directory: &Path,
    meta: &[u8],
    note: &str,
    audio: Option<&Path>,
) -> std::io::Result<()> {
    write_synced(&paths::note_path_in(directory), note.as_bytes())?;
    if let Some(source) = audio {
        let mut dest = File::create(directory.join("audio.wav"))?;
        std::io::copy(&mut File::open(source)?, &mut dest)?;
        dest.sync_all()?;
    }
    write_synced(&paths::meta_path_in(directory), meta)?;
    File::open(directory)?.sync_all()
}

fn write_synced(path: &Path, content: &[u8]) -> std::io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(content)?;
    file.sync_all()
}

fn io_error(error: impl std::fmt::Display) -> StoreError {
    StoreError::Io(error.to_string())
}

fn ignore_exists(error: std::io::Error) -> std::io::Result<()> {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        Ok(())
    } else {
        Err(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn committed_capture_and_processing_survive_restart_and_replay() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("vault");
        fs::create_dir(&vault).unwrap();
        let source = temp.path().join("source.wav");
        fs::write(&source, b"audio bytes").unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let meta: SessionMeta = serde_json::from_value(serde_json::json!({
            "id": id, "title": "", "created_at": "2026-11-18T12:02:00Z", "tags": []
        }))
        .unwrap();

        let store = SessionStore::new(vault.clone());
        store
            .commit_iphone_capture(
                meta.clone(),
                "Original note".into(),
                Some(source.clone()),
                "receipt".into(),
            )
            .await
            .unwrap();
        drop(store);

        let store = SessionStore::new(vault.clone());
        store.rebuild_index().await.unwrap();
        assert_eq!(
            store.iphone_capture_jobs(),
            vec![(id.clone(), CaptureStage::Transcribe)]
        );
        store.write_note(&id, "Edited on Mac").await.unwrap();
        store
            .commit_iphone_capture(meta, "Original note".into(), Some(source), "receipt".into())
            .await
            .unwrap();
        assert_eq!(store.session_ids(), [id.clone()]);
        assert_eq!(
            store.read_note(&id).await.unwrap().as_deref(),
            Some("Edited on Mac")
        );
        assert_eq!(
            store.read_meta(&id).await.unwrap().unwrap().created_at,
            "2026-11-18T12:02:00Z"
        );
        assert_eq!(
            fs::read(vault.join("sessions").join(&id).join("audio.wav")).unwrap(),
            b"audio bytes"
        );
        store
            .advance_iphone_capture(&id, CaptureStage::Transcribe)
            .await
            .unwrap();
        assert_eq!(
            store.iphone_capture_jobs(),
            vec![(id.clone(), CaptureStage::Summarize)]
        );
        store
            .advance_iphone_capture(&id, CaptureStage::Summarize)
            .await
            .unwrap();
        assert!(store.iphone_capture_jobs().is_empty());
    }

    #[tokio::test]
    async fn interrupted_staging_is_invisible_and_text_capture_needs_no_processing() {
        let vault = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let session_files = vault.path().join("sessions");
        let stage = session_files.join(format!(".iphone-capture-{id}"));
        fs::create_dir_all(&stage).unwrap();
        fs::write(stage.join("notes.md"), b"interrupted write").unwrap();
        let store = SessionStore::new(vault.path().to_owned());
        store.rebuild_index().await.unwrap();
        assert!(store.session_ids().is_empty());

        let meta = serde_json::from_value(serde_json::json!({
            "id": id, "title": "", "created_at": "2026-11-18T12:02:00Z", "tags": []
        }))
        .unwrap();
        store
            .commit_iphone_capture(meta, "Text note".into(), None, "receipt".into())
            .await
            .unwrap();
        assert_eq!(store.session_ids(), [id.clone()]);
        assert_eq!(
            store.read_note(&id).await.unwrap().as_deref(),
            Some("Text note")
        );
        assert!(!stage.exists());
        assert!(store.iphone_capture_jobs().is_empty());
    }

    #[tokio::test]
    async fn unavailable_vault_cannot_be_recreated_by_handoff() {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().join("missing");
        let store = SessionStore::new(vault.clone());
        let meta = serde_json::from_value(serde_json::json!({
            "id": uuid::Uuid::new_v4().to_string(), "title": "", "created_at": "2026-11-18T12:02:00Z", "tags": []
        })).unwrap();
        assert!(
            store
                .commit_iphone_capture(meta, "A note".into(), None, "digest".into())
                .await
                .is_err()
        );
        assert!(!vault.exists());
    }
}
