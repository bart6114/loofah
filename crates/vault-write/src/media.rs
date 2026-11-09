use std::{fs::File, path::Path};

use crate::{SessionStore, StoreError};

#[derive(Debug)]
pub struct OpenedRecording {
    pub file: File,
    pub filename: &'static str,
}

#[derive(Debug)]
pub enum RecordingSnapshot {
    Absent,
    Busy,
    File(OpenedRecording),
}

impl SessionStore {
    pub async fn open_recording(&self, id: &str) -> Result<Option<RecordingSnapshot>, StoreError> {
        crate::validate_session_id(id)?;
        let store = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            store.ensure_append_ready(&id)?;
            let Some(metadata) = store
                .metadata_snapshot_locked(&snapshot._guard, &id)
                .await?
            else {
                return Ok(None);
            };
            if store.is_recording(&id)
                || metadata
                    .meta
                    .extra
                    .get("audio_import_pending")
                    .and_then(|value| value.as_bool())
                    == Some(true)
            {
                return Ok(Some(RecordingSnapshot::Busy));
            }
            let directory = store
                .vault_base
                .join(store.session_dir_locked(&snapshot._guard, &id).await?);
            store
                .spawn_blocking(move || {
                    let _snapshot = snapshot;
                    let legacy = directory.join("audio");
                    for directory in [&directory, &legacy] {
                        match std::fs::symlink_metadata(directory) {
                            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
                            Err(cause) => return Err(io_error(cause)),
                            Ok(metadata)
                                if !metadata.is_dir() || metadata.file_type().is_symlink() =>
                            {
                                return Err(StoreError::Io(
                                    "Recording directory is not a regular directory".into(),
                                ));
                            }
                            Ok(_) => {}
                        }
                        if let Some(file) = open_recording_in(directory)? {
                            return Ok(Some(RecordingSnapshot::File(file)));
                        }
                    }
                    Ok(Some(RecordingSnapshot::Absent))
                })
                .await
                .map_err(|cause| StoreError::Io(format!("recording file worker failed: {cause}")))?
        })
        .await
        .map_err(|cause| StoreError::Io(format!("recording read worker failed: {cause}")))?
    }
}

pub(crate) fn open_recording_in(directory: &Path) -> Result<Option<OpenedRecording>, StoreError> {
    for filename in ["audio.mp3", "audio.wav", "audio.ogg"] {
        if let Some(file) = open_regular_file(&directory.join(filename))? {
            return Ok(Some(OpenedRecording { file, filename }));
        }
    }
    Ok(None)
}

pub(crate) fn open_regular_file(path: &Path) -> Result<Option<File>, StoreError> {
    match std::fs::symlink_metadata(path) {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => return Err(io_error(cause)),
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return Err(StoreError::Io("Vault file is not a regular file".into()));
        }
        Ok(_) => {}
    }
    let file = File::open(path).map_err(io_error)?;
    if !file.metadata().map_err(io_error)?.is_file() {
        return Err(StoreError::Io("Vault handle is not a regular file".into()));
    }
    Ok(Some(file))
}

fn io_error(cause: std::io::Error) -> StoreError {
    StoreError::Io(cause.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    async fn fixture() -> (tempfile::TempDir, SessionStore) {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().into());
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Title","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Notes remain unchanged")
            .await
            .unwrap();
        (root, store)
    }

    #[tokio::test]
    async fn recording_reads_select_only_owned_names_and_keep_the_opened_generation() {
        let (root, store) = fixture().await;
        let directory = root.path().join("sessions/note");
        for name in [
            "unknown.wav",
            ".audio.mp3",
            "audio_mic.wav",
            "audio_spk.wav",
            "audio.mp3.tmp",
        ] {
            std::fs::write(directory.join(name), b"Never recording content").unwrap();
        }
        assert!(matches!(
            store.open_recording("note").await.unwrap(),
            Some(RecordingSnapshot::Absent)
        ));
        std::fs::create_dir(directory.join("audio")).unwrap();
        std::fs::write(directory.join("audio/audio.ogg"), b"Legacy recording").unwrap();
        let Some(RecordingSnapshot::File(mut legacy)) = store.open_recording("note").await.unwrap()
        else {
            panic!("missing legacy recording")
        };
        let mut bytes = vec![];
        legacy.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(legacy.filename, "audio.ogg");
        assert_eq!(bytes, b"Legacy recording");
        std::fs::write(directory.join("audio.wav"), b"Canonical recording").unwrap();
        let Some(RecordingSnapshot::File(mut source)) = store.open_recording("note").await.unwrap()
        else {
            panic!("missing canonical recording")
        };
        std::fs::rename(
            directory.join("audio.wav"),
            directory.join("prior-recording.wav"),
        )
        .unwrap();
        std::fs::write(directory.join("audio.wav"), b"New recording").unwrap();
        let mut bytes = vec![];
        source.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(source.filename, "audio.wav");
        assert_eq!(bytes, b"Canonical recording");
        assert_eq!(
            std::fs::read_to_string(directory.join("notes.md")).unwrap(),
            "Notes remain unchanged"
        );
        for name in [
            "unknown.wav",
            ".audio.mp3",
            "audio_mic.wav",
            "audio_spk.wav",
            "audio.mp3.tmp",
        ] {
            assert_eq!(
                std::fs::read(directory.join(name)).unwrap(),
                b"Never recording content"
            );
        }
    }

    #[tokio::test]
    async fn missing_busy_and_invalid_sessions_never_create_or_open_recordings() {
        let (root, store) = fixture().await;
        assert!(store.open_recording("missing").await.unwrap().is_none());
        assert!(!root.path().join("sessions/missing").exists());
        assert!(store.open_recording("../note").await.is_err());
        let directory = root.path().join("sessions/note");
        std::fs::write(directory.join("audio.mp3"), b"Recording").unwrap();
        store.note_recording_active("note");
        assert!(matches!(
            store.open_recording("note").await.unwrap(),
            Some(RecordingSnapshot::Busy)
        ));
        store.release_recording_prepare("note").await.unwrap();
        let mut meta = store.read_meta("note").await.unwrap().unwrap();
        meta.extra
            .insert("audio_import_pending".into(), true.into());
        store.write_meta(&meta).await.unwrap();
        assert!(matches!(
            store.open_recording("note").await.unwrap(),
            Some(RecordingSnapshot::Busy)
        ));
        meta.extra.remove("audio_import_pending");
        store.write_meta(&meta).await.unwrap();
        std::fs::remove_file(directory.join("audio.mp3")).unwrap();
        std::fs::create_dir(directory.join("audio.mp3")).unwrap();
        assert!(store.open_recording("note").await.is_err());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn recording_reads_refuse_symlinked_files_legacy_directories_and_metadata() {
        use std::os::unix::fs::symlink;
        for target in ["file", "legacy", "metadata", "session"] {
            let (root, store) = fixture().await;
            let outside = tempfile::tempdir().unwrap();
            std::fs::write(
                outside.path().join("audio.mp3"),
                b"Private outside recording",
            )
            .unwrap();
            let directory = root.path().join("sessions/note");
            match target {
                "file" => symlink(
                    outside.path().join("audio.mp3"),
                    directory.join("audio.mp3"),
                )
                .unwrap(),
                "legacy" => symlink(outside.path(), directory.join("audio")).unwrap(),
                "metadata" => {
                    std::fs::rename(
                        directory.join("_meta.json"),
                        outside.path().join("_meta.json"),
                    )
                    .unwrap();
                    symlink(
                        outside.path().join("_meta.json"),
                        directory.join("_meta.json"),
                    )
                    .unwrap();
                }
                _ => {
                    std::fs::rename(&directory, outside.path().join("note")).unwrap();
                    symlink(outside.path().join("note"), &directory).unwrap();
                }
            }
            assert!(store.open_recording("note").await.is_err(), "{target}");
            assert_eq!(
                std::fs::read(outside.path().join("audio.mp3")).unwrap(),
                b"Private outside recording"
            );
        }
    }
}
