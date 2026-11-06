use std::path::{Path, PathBuf};

use hypr_vault_read::reserved::{SESSION_PEAKS_FILE, SESSION_TRANSIENT_FILES};

use crate::{SessionStore, StoreError, file_snapshot::FileSnapshot, media::open_regular_file};

const RECORDINGS: [&str; 3] = ["audio.mp3", "audio.wav", "audio.ogg"];

#[derive(Debug, PartialEq, Eq)]
pub enum RecordingDeletion {
    Deleted,
    Absent,
    Busy,
    Changed,
}

struct Candidate {
    path: PathBuf,
    snapshot: FileSnapshot,
}

pub enum RecordingDeletionPlan<T = RecordingDeletionCommit> {
    Prepared(Box<T>),
    Finished(RecordingDeletion),
}

pub struct RecordingDeletionCommit {
    store: SessionStore,
    id: String,
    directory: PathBuf,
    original: Vec<Candidate>,
    hashes: Vec<String>,
    _audio: tokio::sync::OwnedMutexGuard<()>,
}

pub struct CheckedRecordingDeletionCommit(RecordingDeletionCommit);

impl SessionStore {
    pub async fn delete_recording_checked(
        &self,
        id: &str,
        expected_sha256: &str,
    ) -> Result<Option<RecordingDeletion>, StoreError> {
        self.delete_recording_with(id, expected_sha256, || {}).await
    }

    async fn delete_recording_with(
        &self,
        id: &str,
        expected_sha256: &str,
        before_hash: impl FnOnce() + Send + 'static,
    ) -> Result<Option<RecordingDeletion>, StoreError> {
        let store = self.clone();
        let id = id.to_owned();
        let expected = expected_sha256.to_owned();
        tokio::spawn(async move {
            match store
                .prepare_recording_deletion_with(&id, &expected, before_hash)
                .await?
            {
                None => Ok(None),
                Some(RecordingDeletionPlan::Finished(status)) => Ok(Some(status)),
                Some(RecordingDeletionPlan::Prepared(commit)) => (*commit).commit().await,
            }
        })
        .await
        .map_err(worker_error)?
    }

    pub async fn prepare_recording_deletion(
        &self,
        id: &str,
        expected_sha256: &str,
    ) -> Result<Option<RecordingDeletionPlan>, StoreError> {
        self.prepare_recording_deletion_with(id, expected_sha256, || {})
            .await
    }

    async fn prepare_recording_deletion_with(
        &self,
        id: &str,
        expected_sha256: &str,
        before_hash: impl FnOnce() + Send + 'static,
    ) -> Result<Option<RecordingDeletionPlan>, StoreError> {
        crate::validate_session_id(id)?;
        let store = self.clone();
        let id = id.to_owned();
        let expected = expected_sha256.to_owned();
        tokio::spawn(async move {
            let audio = store.lock_session_audio(&id).await?;
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            let Some(meta) = store
                .metadata_snapshot_locked(&snapshot._guard, &id)
                .await?
            else {
                return Ok(None);
            };
            if unavailable(&store, &id, &meta.meta) {
                return Ok(Some(RecordingDeletionPlan::Finished(
                    RecordingDeletion::Busy,
                )));
            }
            let directory = store
                .vault_base
                .join(store.session_dir_locked(&snapshot._guard, &id).await?);
            let initial = directory.clone();
            let original = tokio::task::spawn_blocking(move || {
                let _snapshot = snapshot;
                candidates(&initial)
            })
            .await
            .map_err(worker_error)??;
            let (original, hashes) = tokio::task::spawn_blocking(move || {
                before_hash();
                let hashes = recording_hashes(&original)?;
                Ok::<_, StoreError>((original, hashes))
            })
            .await
            .map_err(worker_error)??;
            if hashes.first().is_some_and(|sha| sha != &expected) {
                return Ok(Some(RecordingDeletionPlan::Finished(
                    RecordingDeletion::Changed,
                )));
            }
            Ok(Some(RecordingDeletionPlan::Prepared(Box::new(
                RecordingDeletionCommit {
                    store,
                    id,
                    directory,
                    original,
                    hashes,
                    _audio: audio,
                },
            ))))
        })
        .await
        .map_err(worker_error)?
    }
}

impl RecordingDeletionCommit {
    pub async fn recheck(
        self,
    ) -> Result<RecordingDeletionPlan<CheckedRecordingDeletionCommit>, StoreError> {
        self.recheck_with(|| {}).await
    }

    async fn recheck_with(
        self,
        before_hash: impl FnOnce() + Send + 'static,
    ) -> Result<RecordingDeletionPlan<CheckedRecordingDeletionCommit>, StoreError> {
        tokio::task::spawn_blocking(move || {
            if !lock_for_recheck(&self.original)? {
                return Ok(RecordingDeletionPlan::Finished(RecordingDeletion::Busy));
            }
            before_hash();
            // Windows can preserve all change metadata after an in-place edit.
            if recording_hashes(&self.original)? != self.hashes {
                return Ok(RecordingDeletionPlan::Finished(RecordingDeletion::Changed));
            }
            Ok(RecordingDeletionPlan::Prepared(Box::new(
                CheckedRecordingDeletionCommit(self),
            )))
        })
        .await
        .map_err(worker_error)?
    }

    pub async fn commit(self) -> Result<Option<RecordingDeletion>, StoreError> {
        self.commit_with(|| {}, |path| std::fs::remove_file(path))
            .await
    }

    async fn commit_with(
        self,
        before_lock: impl FnOnce() + Send + 'static,
        remove: impl FnMut(&Path) -> std::io::Result<()> + Send + 'static,
    ) -> Result<Option<RecordingDeletion>, StoreError> {
        match self.recheck().await? {
            RecordingDeletionPlan::Prepared(checked) => {
                (*checked).commit_with(before_lock, remove).await
            }
            RecordingDeletionPlan::Finished(status) => Ok(Some(status)),
        }
    }
}

impl CheckedRecordingDeletionCommit {
    pub async fn commit(self) -> Result<Option<RecordingDeletion>, StoreError> {
        self.commit_with(|| {}, |path| std::fs::remove_file(path))
            .await
    }

    async fn commit_with(
        self,
        before_lock: impl FnOnce() + Send + 'static,
        mut remove: impl FnMut(&Path) -> std::io::Result<()> + Send + 'static,
    ) -> Result<Option<RecordingDeletion>, StoreError> {
        tokio::spawn(async move {
            let prepared = self.0;
            before_lock();
            let snapshot = prepared.store.lock_snapshot().await;
            prepared.store.ensure_ready()?;
            let Some(meta) = prepared
                .store
                .metadata_snapshot_locked(&snapshot._guard, &prepared.id)
                .await?
            else {
                return Ok(None);
            };
            if unavailable(&prepared.store, &prepared.id, &meta.meta) {
                return Ok(Some(RecordingDeletion::Busy));
            }
            tokio::task::spawn_blocking(move || {
                let _snapshot = snapshot;
                let _audio = prepared._audio;
                let original = prepared.original;
                let current = candidates(&prepared.directory)?;
                if !same_candidates(&original, &current)? {
                    return Ok(Some(RecordingDeletion::Changed));
                }
                let mut paths = Vec::new();
                for base in [&prepared.directory, &prepared.directory.join("audio")] {
                    if !regular_directory(base)? {
                        continue;
                    }
                    for name in SESSION_TRANSIENT_FILES
                        .iter()
                        .copied()
                        .chain([SESSION_PEAKS_FILE])
                        .chain(RECORDINGS)
                    {
                        let path = base.join(name);
                        if open_regular_file(&path)?.is_some() {
                            paths.push(path);
                        }
                    }
                }
                let primary = original.first().map(|entry| &entry.path);
                paths.sort_by_key(|path| Some(path) == primary);
                let mut changed = false;
                let result = (|| {
                    for path in paths {
                        remove(&path).map_err(io_error)?;
                        changed = true;
                    }
                    Ok(Some(if original.is_empty() {
                        RecordingDeletion::Absent
                    } else {
                        RecordingDeletion::Deleted
                    }))
                })();
                if changed {
                    prepared.store.notify_artifacts_changed(&prepared.id);
                }
                result
            })
            .await
            .map_err(worker_error)?
        })
        .await
        .map_err(worker_error)?
    }
}

fn unavailable(store: &SessionStore, id: &str, meta: &crate::SessionMeta) -> bool {
    store.is_recording(id)
        || meta
            .extra
            .get("audio_import_pending")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
}

fn candidates(directory: &Path) -> Result<Vec<Candidate>, StoreError> {
    let mut candidates = Vec::new();
    for base in [directory, &directory.join("audio")] {
        if !regular_directory(base)? {
            continue;
        }
        for filename in RECORDINGS {
            let path = base.join(filename);
            if let Some(file) = open_regular_file(&path)? {
                candidates.push(Candidate {
                    path,
                    snapshot: FileSnapshot::new(file)?,
                });
            }
        }
    }
    Ok(candidates)
}

fn recording_hashes(candidates: &[Candidate]) -> Result<Vec<String>, StoreError> {
    candidates
        .iter()
        .map(|candidate| candidate.snapshot.hash())
        .collect()
}

fn lock_for_recheck(candidates: &[Candidate]) -> Result<bool, StoreError> {
    for candidate in candidates {
        if !candidate.snapshot.lock_for_recheck()? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn same_candidates(before: &[Candidate], after: &[Candidate]) -> Result<bool, StoreError> {
    if before.len() != after.len() {
        return Ok(false);
    }
    for (before, after) in before.iter().zip(after) {
        if before.path != after.path || !before.snapshot.unchanged(&after.snapshot)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn regular_directory(path: &Path) -> Result<bool, StoreError> {
    match std::fs::symlink_metadata(path) {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(cause) => Err(io_error(cause)),
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => Err(StoreError::Io(
            "Recording directory is not a regular directory".into(),
        )),
        Ok(_) => Ok(true),
    }
}
fn worker_error(cause: tokio::task::JoinError) -> StoreError {
    StoreError::Io(format!("Recording deletion worker failed: {cause}"))
}
fn io_error(cause: std::io::Error) -> StoreError {
    StoreError::Io(cause.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (tempfile::TempDir, SessionStore, String) {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().into());
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Title","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Keep this note")
            .await
            .unwrap();
        std::fs::write(
            root.path().join("sessions/note/audio.wav"),
            b"Original recording",
        )
        .unwrap();
        let sha = hypr_fs_sync_core::audio::metadata(&root.path().join("sessions/note"))
            .unwrap()
            .unwrap()
            .sha256;
        (root, store, sha)
    }

    #[tokio::test]
    async fn checked_deletion_removes_exact_owned_recordings_and_artifacts_but_preserves_unknown_files()
     {
        let (root, store, sha) = fixture().await;
        let directory = root.path().join("sessions/note");
        std::fs::create_dir(directory.join("audio")).unwrap();
        for name in [
            "audio.ogg",
            "audio.peaks.json",
            "audio_mic.wav",
            "audio_spk.wav",
            "audio.ogg.tmp",
            "audio/audio.mp3",
            "audio/audio.peaks.json",
        ] {
            std::fs::write(directory.join(name), b"Owned recording artifact").unwrap();
        }
        let protected = [
            "notes.md",
            "_meta.json",
            "user.wav",
            ".hidden",
            "audio/user.wav",
            "audio/.private",
        ];
        for name in &protected[2..] {
            std::fs::write(directory.join(name), b"Protected attachment").unwrap();
        }
        let before: Vec<_> = protected
            .iter()
            .map(|name| std::fs::read(directory.join(name)).unwrap())
            .collect();
        assert_eq!(
            store.delete_recording_checked("note", &sha).await.unwrap(),
            Some(RecordingDeletion::Deleted)
        );
        for (name, bytes) in protected.iter().zip(before) {
            assert_eq!(std::fs::read(directory.join(name)).unwrap(), bytes);
        }
        for name in [
            "audio.wav",
            "audio.ogg",
            "audio.peaks.json",
            "audio_mic.wav",
            "audio_spk.wav",
            "audio.ogg.tmp",
            "audio/audio.mp3",
            "audio/audio.peaks.json",
        ] {
            assert!(!directory.join(name).exists(), "{name}");
        }
        assert_eq!(
            store.delete_recording_checked("note", &sha).await.unwrap(),
            Some(RecordingDeletion::Absent)
        );
        assert_eq!(
            store
                .delete_recording_checked("missing", &sha)
                .await
                .unwrap(),
            None
        );
        assert!(!root.path().join("sessions/missing").exists());
    }

    #[tokio::test]
    async fn stale_hash_active_capture_and_pending_import_leave_every_file_unchanged() {
        let (root, store, sha) = fixture().await;
        let path = root.path().join("sessions/note/audio.wav");
        let original_modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, b"Changed! recording").unwrap();
        let times = std::fs::FileTimes::new().set_modified(original_modified);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(times)
            .unwrap();
        assert_eq!(
            store.delete_recording_checked("note", &sha).await.unwrap(),
            Some(RecordingDeletion::Changed)
        );
        store.note_recording_active("note");
        assert_eq!(
            store.delete_recording_checked("note", &sha).await.unwrap(),
            Some(RecordingDeletion::Busy)
        );
        store.release_recording_prepare("note").await.unwrap();
        store.begin_audio_import("note").await.unwrap();
        assert_eq!(
            store.delete_recording_checked("note", &sha).await.unwrap(),
            Some(RecordingDeletion::Busy)
        );
        assert_eq!(std::fs::read(path).unwrap(), b"Changed! recording");
    }

    #[tokio::test]
    async fn atomic_replacement_during_hashing_is_not_deleted_even_with_the_same_content_and_timestamps()
     {
        let (root, store, sha) = fixture().await;
        let path = root.path().join("sessions/note/audio.wav");
        let initial = std::fs::metadata(&path).unwrap().modified().unwrap();
        let directory = root.path().join("sessions/note");
        let result = store
            .delete_recording_with("note", &sha, move || {
                std::fs::rename(&path, directory.join("user-original.wav")).unwrap();
                std::fs::write(&path, b"Original recording").unwrap();
                std::fs::File::options()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .set_times(std::fs::FileTimes::new().set_modified(initial))
                    .unwrap();
            })
            .await
            .unwrap();
        assert_eq!(result, Some(RecordingDeletion::Changed));
        assert_eq!(
            std::fs::read(root.path().join("sessions/note/audio.wav")).unwrap(),
            b"Original recording"
        );
        assert!(
            root.path()
                .join("sessions/note/user-original.wav")
                .is_file()
        );
    }

    #[tokio::test]
    async fn cancelled_hashing_keeps_audio_ownership_until_completion_without_blocking_note_writes()
    {
        let (root, store, sha) = fixture().await;
        let worker = store.clone();
        let (started, wait) = tokio::sync::oneshot::channel();
        let (finish, blocked) = std::sync::mpsc::channel();
        let request = tokio::spawn(async move {
            worker
                .delete_recording_with("note", &sha, move || {
                    started.send(()).unwrap();
                    blocked
                        .recv_timeout(std::time::Duration::from_secs(5))
                        .unwrap();
                })
                .await
        });
        wait.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            store.save_note("note", "Still responsive", None),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(store.prepare_recording("note").await.is_err());
        assert!(store.store_audio("note", "unused.wav").await.is_err());
        assert!(store.delete_session("note").await.is_err());
        finish.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(guard) = store.lock_session_audio("note").await {
                    drop(guard);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!root.path().join("sessions/note/audio.wav").exists());
        assert_eq!(
            std::fs::read_to_string(root.path().join("sessions/note/notes.md")).unwrap(),
            "Still responsive"
        );
    }

    #[tokio::test]
    async fn busy_state_is_rechecked_after_hashing_before_any_file_is_removed() {
        for import in [false, true] {
            let (root, store, sha) = fixture().await;
            let worker = store.clone();
            let (started, wait) = tokio::sync::oneshot::channel();
            let (finish, blocked) = std::sync::mpsc::channel();
            let request = tokio::spawn(async move {
                worker
                    .delete_recording_with("note", &sha, move || {
                        started.send(()).unwrap();
                        blocked
                            .recv_timeout(std::time::Duration::from_secs(5))
                            .unwrap();
                    })
                    .await
            });
            wait.await.unwrap();
            if import {
                store.begin_audio_import("note").await.unwrap();
            } else {
                store.note_recording_active("note");
            }
            finish.send(()).unwrap();
            assert_eq!(
                request.await.unwrap().unwrap(),
                Some(RecordingDeletion::Busy)
            );
            assert_eq!(
                std::fs::read(root.path().join("sessions/note/audio.wav")).unwrap(),
                b"Original recording"
            );
        }
    }

    #[tokio::test]
    async fn cancelled_commit_waiting_for_the_write_lock_keeps_its_audio_reservation() {
        let (root, store, sha) = fixture().await;
        let Some(RecordingDeletionPlan::Prepared(commit)) = store
            .prepare_recording_deletion("note", &sha)
            .await
            .unwrap()
        else {
            panic!("missing commit");
        };
        let snapshot = store.lock_snapshot().await;
        let reservation = store.audio_operations.lock().unwrap()["note"].clone();
        let (started, wait) = tokio::sync::oneshot::channel();
        let request = tokio::spawn((*commit).commit_with(
            move || {
                started.send(()).unwrap();
            },
            |path| std::fs::remove_file(path),
        ));
        wait.await.unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert!(reservation.clone().try_lock_owned().is_err());
        assert!(root.path().join("sessions/note/audio.wav").exists());
        drop(snapshot);
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(guard) = reservation.clone().try_lock_owned() {
                    drop(guard);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!root.path().join("sessions/note/audio.wav").exists());
    }

    #[tokio::test]
    async fn in_place_edits_with_restored_mtime_after_preparation_cannot_be_deleted() {
        let (root, store, sha) = fixture().await;
        let path = root.path().join("sessions/note/audio.wav");
        let initial = std::fs::metadata(&path).unwrap().modified().unwrap();
        let Some(RecordingDeletionPlan::Prepared(commit)) = store
            .prepare_recording_deletion("note", &sha)
            .await
            .unwrap()
        else {
            panic!("missing commit");
        };
        std::fs::write(&path, b"Changed! recording").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(initial))
            .unwrap();
        assert_eq!(
            (*commit).commit().await.unwrap(),
            Some(RecordingDeletion::Changed)
        );
        assert_eq!(std::fs::read(path).unwrap(), b"Changed! recording");
    }

    #[tokio::test]
    async fn changed_secondary_recording_is_rejected_even_when_the_primary_and_timestamps_match() {
        let (root, store, sha) = fixture().await;
        let path = root.path().join("sessions/note/audio.ogg");
        std::fs::write(&path, b"Original recording").unwrap();
        let initial = std::fs::metadata(&path).unwrap().modified().unwrap();
        let Some(RecordingDeletionPlan::Prepared(commit)) = store
            .prepare_recording_deletion("note", &sha)
            .await
            .unwrap()
        else {
            panic!("missing commit");
        };
        use std::io::Write;
        let mut writer = std::fs::File::options().write(true).open(&path).unwrap();
        writer.write_all(b"Changed! recording").unwrap();
        writer
            .set_times(std::fs::FileTimes::new().set_modified(initial))
            .unwrap();
        assert_eq!(
            (*commit).commit().await.unwrap(),
            Some(RecordingDeletion::Changed)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"Changed! recording");
        assert_eq!(
            std::fs::read(root.path().join("sessions/note/audio.wav")).unwrap(),
            b"Original recording"
        );
    }

    #[tokio::test]
    async fn cancelled_recheck_retains_ownership_without_holding_the_vault_write_lock() {
        let (root, store, sha) = fixture().await;
        let Some(RecordingDeletionPlan::Prepared(commit)) = store
            .prepare_recording_deletion("note", &sha)
            .await
            .unwrap()
        else {
            panic!("missing commit");
        };
        let reservation = store.audio_operations.lock().unwrap()["note"].clone();
        let (started, wait) = tokio::sync::oneshot::channel();
        let (finish, blocked) = std::sync::mpsc::channel();
        let request = tokio::spawn((*commit).recheck_with(move || {
            started.send(()).unwrap();
            blocked
                .recv_timeout(std::time::Duration::from_secs(5))
                .unwrap();
        }));
        wait.await.unwrap();
        request.abort();
        let Err(cause) = request.await else {
            panic!("recheck caller was not cancelled");
        };
        assert!(cause.is_cancelled());
        assert!(reservation.clone().try_lock_owned().is_err());
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            store.save_note("note", "Still responsive", None),
        )
        .await
        .unwrap()
        .unwrap();
        finish.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let Ok(guard) = reservation.clone().try_lock_owned() {
                    drop(guard);
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read(root.path().join("sessions/note/audio.wav")).unwrap(),
            b"Original recording"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn recheck_blocks_writes_from_an_existing_handle_while_allowing_playback_until_dropped() {
        use std::io::{Seek, SeekFrom, Write};
        let (root, store, sha) = fixture().await;
        let path = root.path().join("sessions/note/audio.wav");
        let mut writer = std::fs::File::options().write(true).open(&path).unwrap();
        let Some(RecordingDeletionPlan::Prepared(commit)) = store
            .prepare_recording_deletion("note", &sha)
            .await
            .unwrap()
        else {
            panic!("missing commit");
        };
        let RecordingDeletionPlan::Prepared(checked) = (*commit).recheck().await.unwrap() else {
            panic!("missing checked commit");
        };
        assert!(writer.write_all(b"Changed! recording").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"Original recording");
        drop(checked);
        writer.seek(SeekFrom::Start(0)).unwrap();
        writer.write_all(b"Changed! recording").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"Changed! recording");
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn existing_exclusive_lock_refuses_deletion_without_removing_owned_artifacts() {
        let (root, store, sha) = fixture().await;
        let directory = root.path().join("sessions/note");
        let Some(RecordingDeletionPlan::Prepared(commit)) = store
            .prepare_recording_deletion("note", &sha)
            .await
            .unwrap()
        else {
            panic!("missing commit");
        };
        std::fs::write(directory.join("audio.peaks.json"), b"Keep peaks").unwrap();
        let writer = std::fs::File::options()
            .write(true)
            .open(directory.join("audio.wav"))
            .unwrap();
        writer.try_lock().unwrap();
        assert_eq!(
            (*commit).commit().await.unwrap(),
            Some(RecordingDeletion::Busy)
        );
        assert_eq!(
            std::fs::read(directory.join("audio.peaks.json")).unwrap(),
            b"Keep peaks"
        );
        drop(writer);
        assert_eq!(
            std::fs::read(directory.join("audio.wav")).unwrap(),
            b"Original recording"
        );
    }

    #[tokio::test]
    async fn partial_cleanup_failure_keeps_the_primary_recording_and_checked_retry_finishes() {
        let (root, store, sha) = fixture().await;
        let directory = root.path().join("sessions/note");
        std::fs::write(directory.join("audio_mic.wav"), b"Owned intermediate").unwrap();
        std::fs::write(directory.join("audio.peaks.json"), b"Owned cache").unwrap();
        std::fs::write(directory.join("unknown.wav"), b"Protected attachment").unwrap();
        let Some(RecordingDeletionPlan::Prepared(commit)) = store
            .prepare_recording_deletion("note", &sha)
            .await
            .unwrap()
        else {
            panic!("missing commit");
        };
        let mut removed = 0;
        let result = (*commit)
            .commit_with(
                || {},
                move |path| {
                    if removed == 1 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::PermissionDenied,
                            "Injected unlink failure",
                        ));
                    }
                    std::fs::remove_file(path)?;
                    removed += 1;
                    Ok(())
                },
            )
            .await;
        assert!(result.is_err());
        assert!(!directory.join("audio_mic.wav").exists());
        assert_eq!(
            std::fs::read(directory.join("audio.wav")).unwrap(),
            b"Original recording"
        );
        assert_eq!(
            std::fs::read(directory.join("unknown.wav")).unwrap(),
            b"Protected attachment"
        );
        assert_eq!(
            store.delete_recording_checked("note", &sha).await.unwrap(),
            Some(RecordingDeletion::Deleted)
        );
        assert!(!directory.join("audio.wav").exists());
        assert!(!directory.join("audio.peaks.json").exists());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlinked_artifacts_are_refused_before_any_owned_bytes_are_removed() {
        let (root, store, sha) = fixture().await;
        let target = root.path().join("protected");
        std::fs::write(&target, b"Protected outside content").unwrap();
        let directory = root.path().join("sessions/note");
        std::fs::write(
            directory.join("audio_mic.wav"),
            b"Keep until deletion is admitted",
        )
        .unwrap();
        std::os::unix::fs::symlink(&target, directory.join("audio.peaks.json")).unwrap();
        assert!(store.delete_recording_checked("note", &sha).await.is_err());
        assert_eq!(
            std::fs::read(&target).unwrap(),
            b"Protected outside content"
        );
        assert!(directory.join("audio.wav").exists());
        assert!(directory.join("audio_mic.wav").exists());
    }
}
