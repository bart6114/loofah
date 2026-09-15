//! Note-attachment storage under `<session_dir>/attachments/`. Same semantics as the
//! fs-sync plugin's `attachment_save`: sanitize to a basename, then a `create_new`
//! dedupe loop so an existing file is never overwritten.

use std::path::{Path, PathBuf};

use super::{SessionStore, StoreError, validate_session_id};

#[derive(Clone, Debug, PartialEq)]
pub struct SavedAttachment {
    /// Final on-disk filename — the attachment id referenced from note markdown.
    pub attachment_id: String,
    /// Vault-relative path of the stored file.
    pub relative_path: PathBuf,
}

impl SessionStore {
    pub async fn save_attachment(
        &self,
        id: &str,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<SavedAttachment, StoreError> {
        validate_session_id(id)?;
        let guard = self.lock_writes().await;
        let dir = self.session_dir_locked(&guard, id).await?;

        let relative_dir = dir.join("attachments");
        let abs_dir = self.vault_base.join(&relative_dir);
        let filename = filename.to_string();
        let final_filename = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&abs_dir)
                .map_err(|e| StoreError::Io(format!("failed to create attachments dir: {e}")))?;
            let safe_filename = sanitize_filename(&filename)?;
            write_unique_file(&abs_dir, &safe_filename, &bytes)
        })
        .await
        .map_err(|e| StoreError::Io(format!("task join error: {e}")))??;
        drop(guard);
        self.notify_artifacts_changed(id);

        Ok(SavedAttachment {
            relative_path: relative_dir.join(&final_filename),
            attachment_id: final_filename,
        })
    }
}

/// Basename only — a caller-supplied path must not be able to place the file outside
/// the session's `attachments/` directory.
fn sanitize_filename(filename: &str) -> Result<String, StoreError> {
    let clean_name = Path::new(filename)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| StoreError::Io("invalid attachment filename".to_string()))?;

    if clean_name.is_empty() || clean_name.contains(['/', '\\', '\0']) {
        return Err(StoreError::Io(
            "invalid attachment filename characters".to_string(),
        ));
    }

    Ok(clean_name.to_string())
}

/// First try the sanitized name verbatim, then `{stem} {counter}.{ext}` — `create_new`
/// makes the existence check and the create one atomic step, so two concurrent saves
/// can never claim the same name.
fn write_unique_file(dir: &Path, filename: &str, data: &[u8]) -> Result<String, StoreError> {
    use std::io::Write;

    let path = Path::new(filename);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let extension = path.extension().and_then(|e| e.to_str());

    let mut counter = 0;
    loop {
        let candidate_filename = if counter == 0 {
            filename.to_string()
        } else {
            match extension {
                Some(ext) => format!("{} {}.{}", stem, counter, ext),
                None => format!("{} {}", stem, counter),
            }
        };

        let candidate_path = dir.join(&candidate_filename);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate_path)
        {
            Ok(mut file) => {
                if let Err(error) = file.write_all(data) {
                    drop(file);
                    if let Err(cleanup_error) = std::fs::remove_file(&candidate_path) {
                        tracing::warn!(
                            error = %cleanup_error,
                            "failed to remove partially written attachment"
                        );
                    }
                    return Err(StoreError::Io(format!(
                        "failed to write attachment: {error}"
                    )));
                }
                return Ok(candidate_filename);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                counter += 1;
                continue;
            }
            Err(e) => {
                return Err(StoreError::Io(format!(
                    "failed to create attachment file: {e}"
                )));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::SessionMeta;
    use super::*;

    fn meta(id: &str, title: &str) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            title: title.to_string(),
            started_at: None,
            ended_at: None,
            created_at: "2026-07-24T00:00:00Z".to_string(),
            tags: vec![],
            tag_suggestions: None,
            tracking_id: None,
            folder: None,
            author: None,
            skill: None,
            extra: Default::default(),
        }
    }

    async fn test_store() -> (SessionStore, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        let store = SessionStore::new(temp.path().to_path_buf());
        (store, temp)
    }

    #[tokio::test]
    async fn same_filename_twice_dedupes_with_a_counter_suffix() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Standup")).await.unwrap();

        let first = store
            .save_attachment("s1", "file.txt", b"first".to_vec())
            .await
            .unwrap();
        let second = store
            .save_attachment("s1", "file.txt", b"second".to_vec())
            .await
            .unwrap();

        assert_eq!(first.attachment_id, "file.txt");
        assert_eq!(second.attachment_id, "file 1.txt");
        assert_eq!(
            std::fs::read(vault.path().join(&first.relative_path)).unwrap(),
            b"first"
        );
        assert_eq!(
            std::fs::read(vault.path().join(&second.relative_path)).unwrap(),
            b"second"
        );
    }

    #[tokio::test]
    async fn directory_components_are_stripped_and_empty_filename_errors() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Standup")).await.unwrap();

        let saved = store
            .save_attachment("s1", "nested/path/evil.txt", b"payload".to_vec())
            .await
            .unwrap();
        assert_eq!(saved.attachment_id, "evil.txt");
        assert_eq!(
            saved.relative_path.file_name().unwrap().to_str().unwrap(),
            "evil.txt"
        );
        let abs = vault.path().join(&saved.relative_path);
        assert!(abs.is_file());
        assert_eq!(
            abs.parent().unwrap().file_name().unwrap().to_str().unwrap(),
            "attachments",
            "the file must land directly in attachments/, not in a nested subtree"
        );
        assert_eq!(std::fs::read_dir(abs.parent().unwrap()).unwrap().count(), 1);

        assert!(
            store
                .save_attachment("s1", "", b"nope".to_vec())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn resolves_a_canonical_legacy_id_session_directory() {
        let (store, vault) = test_store().await;
        let dir = vault.path().join("sessions/abc123");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            br#"{"id":"abc123","title":"Standup","started_at":null,"ended_at":null,"created_at":"2026-03-20T00:00:00Z","tags":[]}"#,
        )
        .unwrap();

        let saved = store
            .save_attachment("abc123", "shot.png", b"png-bytes".to_vec())
            .await
            .unwrap();

        assert_eq!(
            saved.relative_path,
            PathBuf::from("sessions/abc123/attachments/shot.png")
        );
        assert_eq!(
            std::fs::read(dir.join("attachments/shot.png")).unwrap(),
            b"png-bytes"
        );
        assert!(vault.path().join("sessions/abc123").exists());
    }

    #[tokio::test]
    async fn relative_path_joins_with_vault_base_to_an_existing_file() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Standup")).await.unwrap();

        let saved = store
            .save_attachment("s1", "doc.pdf", b"%PDF".to_vec())
            .await
            .unwrap();

        assert!(saved.relative_path.is_relative());
        let abs = vault.path().join(&saved.relative_path);
        assert!(abs.is_file());
        assert_eq!(std::fs::read(abs).unwrap(), b"%PDF");
    }

    #[tokio::test]
    async fn filenames_with_spaces_and_unicode_are_preserved_verbatim() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Standup")).await.unwrap();

        let saved = store
            .save_attachment("s1", "image 73.png", b"a".to_vec())
            .await
            .unwrap();
        assert_eq!(saved.attachment_id, "image 73.png");

        let unicode = store
            .save_attachment("s1", "présentation été.png", b"b".to_vec())
            .await
            .unwrap();
        assert_eq!(unicode.attachment_id, "présentation été.png");
        assert!(vault.path().join(&unicode.relative_path).is_file());

        let deduped = store
            .save_attachment("s1", "image 73.png", b"c".to_vec())
            .await
            .unwrap();
        assert_eq!(deduped.attachment_id, "image 73 1.png");
    }
}
