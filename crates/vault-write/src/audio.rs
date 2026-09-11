use std::path::{Path, PathBuf};

use super::{SessionStore, StoreError, paths, validate_session_id};

/// Moves `source` to `dest` (assumed to not yet exist; `dest`'s parent must already exist).
/// Prefers `rename` (atomic, no data-duplication window); falls back to copy+delete when
/// `rename` fails (typically EXDEV -- source and dest on different volumes, which
/// `std::fs::rename` can't cross). Returns whether the fallback path was taken, so callers
/// (and tests) can tell which branch actually ran. `rename` is injected so the fallback branch
/// can be exercised deterministically in tests without needing a real cross-device setup.
fn move_or_copy_delete(
    source: &Path,
    dest: &Path,
    rename: fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<bool, StoreError> {
    if rename(source, dest).is_ok() {
        return Ok(false);
    }

    std::fs::copy(source, dest)
        .map_err(|e| StoreError::Io(format!("failed to copy audio file: {}", e)))?;
    std::fs::remove_file(source).map_err(|e| {
        StoreError::Io(format!(
            "failed to remove source audio file after copy: {}",
            e
        ))
    })?;
    Ok(true)
}

/// `std::fs::rename` is generic over `AsRef<Path>`, so it doesn't coerce directly to the
/// `for<'a, 'b> fn(&'a Path, &'b Path) -> io::Result<()>` pointer type `move_or_copy_delete`
/// expects (a specific monomorphization isn't the same as being generic over all lifetimes) --
/// this plain wrapper has no such generics and coerces cleanly.
fn plain_rename(source: &Path, dest: &Path) -> std::io::Result<()> {
    std::fs::rename(source, dest)
}

/// The recording file names every reader knows about: `hypr_fs_sync_core::audio` (the audio
/// player's existence/path/metadata lookups), `listener_core::resolve_final_audio_path` (the
/// path handed to post-capture batch transcription) all agree on
/// `audio.{mp3,wav,ogg}` inside the session directory, and none of them search anywhere else.
const READABLE_AUDIO_EXTENSIONS: [&str; 3] = ["mp3", "wav", "ogg"];

fn canonical_audio_file_name(source: &Path) -> Result<String, StoreError> {
    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .ok_or_else(|| StoreError::Io("source audio path has no extension".to_string()))?;

    if !READABLE_AUDIO_EXTENSIONS.contains(&extension.as_str()) {
        return Err(StoreError::Io(format!(
            "unsupported audio extension {:?}: a recording stored under it would be invisible to the app",
            extension
        )));
    }

    Ok(format!("audio.{}", extension))
}

/// A recording that already sits in its session's resolved directory -- or in any
/// directory named for its session, which covers a recorder writing into a location
/// discovery doesn't know yet (no `_meta.json` written so far) -- stays where it is;
/// only a file from outside (an import) is moved into the resolved session directory.
fn canonical_audio_dir(resolved_dir_abs: &Path, session_id: &str, source: &Path) -> PathBuf {
    let session_dir = source.parent().filter(|parent| {
        is_same_file_path(parent, resolved_dir_abs)
            || parent
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name == session_id)
    });

    match session_dir {
        Some(dir) => dir.to_path_buf(),
        None => resolved_dir_abs.to_path_buf(),
    }
}

fn is_same_file_path(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

impl SessionStore {
    /// Settles a finished recording at the one path the rest of the app reads:
    /// `<session dir>/audio.<ext>`. The recorder already writes there, so the
    /// usual case is a no-op; an imported file from outside the vault is moved in, preferring
    /// `std::fs::rename` (atomic, no data-duplication window) and falling back to copy+delete
    /// across volumes. Returns the absolute settled path -- callers hold paths to this file
    /// (batch transcription re-reads it, the player resolves it), so they need the path the
    /// recording actually ended up at, not the one they passed in.
    pub async fn store_audio(
        &self,
        session_id: &str,
        source_path: &str,
    ) -> Result<String, StoreError> {
        validate_session_id(session_id)?;
        let resolved_dir_abs = self.vault_base.join(self.session_dir(session_id).await?);
        let session_id = session_id.to_string();
        let source_path = PathBuf::from(source_path);

        tokio::task::spawn_blocking(move || -> Result<String, StoreError> {
            let file_name = canonical_audio_file_name(&source_path)?;
            let dest_dir = canonical_audio_dir(&resolved_dir_abs, &session_id, &source_path);
            let dest_abs = dest_dir.join(&file_name);

            if !is_same_file_path(&source_path, &dest_abs) {
                std::fs::create_dir_all(&dest_dir).map_err(|e| {
                    StoreError::Io(format!("failed to create session directory: {}", e))
                })?;
                move_or_copy_delete(&source_path, &dest_abs, plain_rename)?;
            }

            dest_abs
                .to_str()
                .map(|s| s.to_string())
                .ok_or_else(|| StoreError::Io("invalid destination path".to_string()))
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {}", e)))?
    }

    /// Lists audio file names (not full paths) under `sessions/<id>/audio/`, sorted. Missing
    /// directory -> empty list, matching the "nothing recorded yet" state rather than an error.
    ///
    /// Nothing writes that directory any more -- recordings settle at `<session dir>/audio.<ext>`
    /// (see `store_audio`). This and `delete_audio` support explicitly deleting recordings from vaults
    /// written by the builds that did relocate recordings there.
    pub async fn list_audio(&self, session_id: &str) -> Result<Vec<String>, StoreError> {
        validate_session_id(session_id)?;
        let session_dir = self.session_dir(session_id).await?;
        let vault_base = self.vault_base.clone();

        tokio::task::spawn_blocking(move || -> Result<Vec<String>, StoreError> {
            let dir = vault_base.join(paths::audio_dir_in(&session_dir));
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(e) => {
                    return Err(StoreError::Io(format!(
                        "failed to read audio directory: {}",
                        e
                    )));
                }
            };

            let mut names = Vec::new();
            for entry in entries {
                let entry = entry
                    .map_err(|e| StoreError::Io(format!("failed to read dir entry: {}", e)))?;
                if entry.path().is_file()
                    && let Some(name) = entry.file_name().to_str()
                {
                    names.push(name.to_string());
                }
            }
            names.sort();
            Ok(names)
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {}", e)))?
    }

    /// Permanently deletes one legacy recording at the user's request.
    /// Unlike `delete_session`, this is not undo-able. Missing file is a no-op.
    pub async fn delete_audio(&self, session_id: &str, filename: &str) -> Result<(), StoreError> {
        validate_session_id(session_id)?;
        let session_dir = self.session_dir(session_id).await?;
        let vault_base = self.vault_base.clone();
        let filename = filename.to_string();

        tokio::task::spawn_blocking(move || -> Result<(), StoreError> {
            // `filename` must be a bare file name: callers only ever pass one back from
            // `list_audio`, but reject path separators/traversal defensively so a bad
            // filename can't escape the audio directory via this command boundary.
            if filename.is_empty() || filename.contains(['/', '\\']) || filename == ".." {
                return Err(StoreError::Io(format!(
                    "invalid audio filename: {}",
                    filename
                )));
            }

            let path = vault_base
                .join(paths::audio_dir_in(&session_dir))
                .join(&filename);
            match std::fs::remove_file(&path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(StoreError::Io(format!(
                    "failed to delete audio file: {}",
                    e
                ))),
            }
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {}", e)))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_store() -> (SessionStore, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let vault = temp.path().to_path_buf();
        let store = SessionStore::new(vault);
        (store, temp)
    }

    /// REGRESSION: `store_audio` used to move recordings into `sessions/<id>/audio/<name>`, a
    /// location nothing reads. The recorder writes `sessions/<id>/audio.<ext>`,
    /// the audio player gates on `hypr_fs_sync_core::audio::exists`, and
    /// post-capture batch transcription re-reads the path the recorder reported -- moving the
    /// file out from under all three cost every recording both its transcript and its player
    /// entry. Asserting through the real reader (rather than a hardcoded path) is the point:
    /// it fails if either side of the convention drifts again.
    #[tokio::test]
    async fn store_audio_leaves_the_recording_where_the_readers_look() {
        let (store, vault) = test_store().await;
        let source = vault.path().join("recording.wav");
        std::fs::write(&source, b"wav-bytes").unwrap();

        let stored = store
            .store_audio("s1", source.to_str().unwrap())
            .await
            .unwrap();

        let session_dir = vault.path().join("sessions/s1");
        assert_eq!(stored, session_dir.join("audio.wav").to_str().unwrap());
        assert!(
            hypr_fs_sync_core::audio::exists(&session_dir).unwrap(),
            "the audio player's existence check must find the stored recording"
        );
        assert_eq!(
            hypr_fs_sync_core::audio::path(&session_dir),
            Some(session_dir.join("audio.wav")),
            "the audio player's path lookup must resolve to the stored recording"
        );
        assert_eq!(std::fs::read(&stored).unwrap(), b"wav-bytes");
        assert!(!source.exists(), "source file must be moved, not copied");
    }

    /// REGRESSION: the recorder already writes into the session directory, so cataloging a
    /// finished recording must be a no-op that hands back the path it was given -- not a
    /// relocation that invalidates every path already handed out for that file.
    #[tokio::test]
    async fn store_audio_is_a_noop_when_the_recording_is_already_canonical() {
        let (store, vault) = test_store().await;
        let session_dir = vault.path().join("sessions/s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let source = session_dir.join("audio.mp3");
        std::fs::write(&source, b"mp3-bytes").unwrap();

        let stored = store
            .store_audio("s1", source.to_str().unwrap())
            .await
            .unwrap();

        assert_eq!(stored, source.to_str().unwrap());
        assert!(source.is_file(), "the recording must survive cataloging");
        assert_eq!(std::fs::read(&source).unwrap(), b"mp3-bytes");
    }

    /// A session stored under a user folder (`sessions/<folder>/<id>/`) keeps its recording in
    /// its own directory -- hoisting it to `sessions/<id>/` would hide it from the readers,
    /// which resolve the session directory by search rather than by the flat path.
    #[tokio::test]
    async fn store_audio_keeps_a_foldered_session_recording_beside_its_session() {
        let (store, vault) = test_store().await;
        let session_dir = vault.path().join("sessions/Work/s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let source = session_dir.join("audio.mp3");
        std::fs::write(&source, b"mp3-bytes").unwrap();

        let stored = store
            .store_audio("s1", source.to_str().unwrap())
            .await
            .unwrap();

        assert_eq!(stored, source.to_str().unwrap());
        assert!(hypr_fs_sync_core::audio::exists(&session_dir).unwrap());
    }

    /// Only extensions the readers know about are accepted -- silently storing `take.m4a`
    /// would reproduce the original bug's signature (a file on disk that no reader can find).
    #[tokio::test]
    async fn store_audio_rejects_an_extension_no_reader_understands() {
        let (store, vault) = test_store().await;
        let source = vault.path().join("take.m4a");
        std::fs::write(&source, b"bytes").unwrap();

        let result = store.store_audio("s1", source.to_str().unwrap()).await;

        assert!(result.is_err());
        assert!(source.exists(), "a rejected source must not be destroyed");
    }

    /// REGRESSION (reviewer-found minor): the previous version of this test asserted only the
    /// end state (file moved, source gone), which the plain-rename path already satisfies on a
    /// single filesystem -- it never actually forced `rename` to fail, so the copy+delete
    /// fallback branch went unexercised. `move_or_copy_delete` takes `rename` as a parameter
    /// specifically so a failure can be injected deterministically here, without needing a
    /// real cross-device mount.
    #[test]
    fn move_or_copy_delete_falls_back_to_copy_and_removes_the_source_when_rename_fails() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.wav");
        let dest = temp.path().join("dest.wav");
        std::fs::write(&source, b"fallback-content").unwrap();

        let used_fallback = move_or_copy_delete(&source, &dest, |_, _| {
            Err(std::io::Error::other("forced rename failure"))
        })
        .unwrap();

        assert!(used_fallback, "must report that the fallback branch ran");
        assert_eq!(std::fs::read(&dest).unwrap(), b"fallback-content");
        assert!(!source.exists(), "source must be removed after the copy");
    }

    #[test]
    fn move_or_copy_delete_reports_no_fallback_when_rename_succeeds() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source.wav");
        let dest = temp.path().join("dest.wav");
        std::fs::write(&source, b"renamed-content").unwrap();

        let used_fallback = move_or_copy_delete(&source, &dest, plain_rename).unwrap();

        assert!(!used_fallback, "must report that rename succeeded directly");
        assert_eq!(std::fs::read(&dest).unwrap(), b"renamed-content");
        assert!(!source.exists());
    }

    #[tokio::test]
    async fn list_audio_returns_empty_for_missing_session() {
        let (store, _vault) = test_store().await;
        assert_eq!(
            store.list_audio("nonexistent").await.unwrap(),
            Vec::<String>::new()
        );
    }

    #[tokio::test]
    async fn list_audio_returns_sorted_file_names() {
        let (store, vault) = test_store().await;
        let audio_dir = vault.path().join("sessions/s1/audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        std::fs::write(audio_dir.join("b.wav"), b"").unwrap();
        std::fs::write(audio_dir.join("a.wav"), b"").unwrap();

        assert_eq!(
            store.list_audio("s1").await.unwrap(),
            vec!["a.wav".to_string(), "b.wav".to_string()]
        );
    }

    #[tokio::test]
    async fn delete_audio_removes_file() {
        let (store, vault) = test_store().await;
        let audio_dir = vault.path().join("sessions/s1/audio");
        std::fs::create_dir_all(&audio_dir).unwrap();
        std::fs::write(audio_dir.join("take.wav"), b"").unwrap();

        store.delete_audio("s1", "take.wav").await.unwrap();

        assert!(!audio_dir.join("take.wav").exists());
    }

    #[tokio::test]
    async fn delete_audio_missing_file_is_a_noop() {
        let (store, _vault) = test_store().await;
        store.delete_audio("s1", "never-existed.wav").await.unwrap();
    }

    #[tokio::test]
    async fn delete_audio_rejects_path_traversal() {
        let (store, _vault) = test_store().await;
        let result = store.delete_audio("s1", "../../etc/passwd").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), StoreError::Io(_)));
    }
}
