//! Note-attachment storage under `<session_dir>/attachments/`. Same semantics as the
//! fs-sync plugin's `attachment_save`: hidden staging, durable bytes and exclusive
//! deduplicated publication so existing files are never overwritten.

#[cfg(test)]
use hypr_storage::fs::windows_safe_filename as windows_basename;
use std::path::{Path, PathBuf};

use super::{SessionStore, StoreError, validate_session_id};

#[derive(Clone, Debug, PartialEq)]
pub struct SavedAttachment {
    /// Final on-disk filename — the attachment id referenced from note markdown.
    pub attachment_id: String,
    /// Vault-relative path of the stored file.
    pub relative_path: PathBuf,
}

#[derive(Debug)]
pub struct OpenedAttachment {
    pub file: std::fs::File,
    pub attachment_id: String,
}

impl SessionStore {
    pub async fn open_attachment(
        &self,
        id: &str,
        attachment_id: &str,
    ) -> Result<Option<OpenedAttachment>, StoreError> {
        validate_session_id(id)?;
        hypr_vault_read::paths::validate_attachment_id(attachment_id)?;
        let store = self.clone();
        let id = id.to_owned();
        let attachment_id = attachment_id.to_owned();
        tokio::spawn(async move {
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            if store
                .metadata_snapshot_locked(&snapshot._guard, &id)
                .await?
                .is_none()
            {
                return Ok(None);
            }
            let directory = store
                .vault_base
                .join(store.session_dir_locked(&snapshot._guard, &id).await?)
                .join("attachments");
            store
                .spawn_blocking(move || {
                    let _snapshot = snapshot;
                    let Some(metadata) = attachment_metadata(&directory)? else {
                        return Ok(None);
                    };
                    if !metadata.is_dir() || metadata.file_type().is_symlink() {
                        return Err(StoreError::Io(
                            "Attachment directory is not a regular directory".into(),
                        ));
                    }
                    let Some(file) =
                        crate::media::open_regular_file(&directory.join(&attachment_id))?
                    else {
                        return Ok(None);
                    };
                    Ok(Some(OpenedAttachment {
                        file,
                        attachment_id,
                    }))
                })
                .await
                .map_err(|cause| {
                    StoreError::Io(format!("attachment read worker failed: {cause}"))
                })?
        })
        .await
        .map_err(|cause| StoreError::Io(format!("attachment request worker failed: {cause}")))?
    }

    pub async fn save_attachment(
        &self,
        id: &str,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<SavedAttachment, StoreError> {
        validate_session_id(id)?;
        let store = self.clone();
        let id = id.to_owned();
        let filename = filename.to_string();
        // Blocking file I/O outlives request cancellation; its write reservation must too.
        tokio::spawn(async move {
            let guard = store.lock_writes().await;
            let dir = store.session_dir_locked(&guard, &id).await?;
            let relative_dir = dir.join("attachments");
            let abs_dir = store.vault_base.join(&relative_dir);
            let final_filename = store
                .spawn_blocking(move || {
                    use std::io::Write;
                    let filename = hypr_fs_sync_core::attachment::sanitize_filename(&filename)
                        .map_err(|cause| StoreError::Io(cause.to_string()))?;
                    let staged = hypr_fs_sync_core::attachment::StagedAttachment::prepare(
                        &abs_dir,
                        |file| file.write_all(&bytes),
                    )
                    .map_err(|cause| StoreError::Io(cause.to_string()))?;
                    staged
                        .publish_unique(&filename)
                        .map(|(_, name)| name)
                        .map_err(|cause| StoreError::Io(cause.to_string()))
                })
                .await
                .map_err(|e| StoreError::Io(format!("attachment file worker failed: {e}")))??;
            drop(guard);
            store.notify_attachment_changed(&id, Some(&final_filename));
            Ok(SavedAttachment {
                relative_path: relative_dir.join(&final_filename),
                attachment_id: final_filename,
            })
        })
        .await
        .map_err(|e| StoreError::Io(format!("attachment save worker failed: {e}")))?
    }
}

fn attachment_metadata(path: &Path) -> Result<Option<std::fs::Metadata>, StoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(cause) => Err(StoreError::Io(cause.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::super::SessionMeta;
    use super::*;

    #[tokio::test]
    async fn attachment_writes_notify_the_final_deduplicated_basename() {
        use crate::index::{ArtifactChange, ArtifactTarget, IndexEntity};
        let (store, _vault) = test_store().await;
        store.write_meta(&meta("s1", "Attachments")).await.unwrap();
        let mut changes = store.subscribe_index_changes();
        for expected in ["report.pdf", "report 1.pdf"] {
            let saved = store
                .save_attachment("s1", "report.pdf", b"Complete original".to_vec())
                .await
                .unwrap();
            assert_eq!(saved.attachment_id, expected);
            let changed = changes.try_recv().unwrap();
            assert_eq!(changed.entity, IndexEntity::Artifacts);
            assert_eq!(changed.ids, ["s1"]);
            assert_eq!(
                changed.artifacts,
                Some(vec![ArtifactChange {
                    session_id: "s1".into(),
                    target: ArtifactTarget::Attachment {
                        attachment_id: expected.into()
                    }
                }])
            );
            assert!(changes.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn attachment_readers_open_only_owned_basenames_and_release_the_store_lock() {
        use std::io::Read;
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Attachments")).await.unwrap();
        let directory = vault.path().join("sessions/s1");
        std::fs::write(directory.join("unknown-user.pdf"), b"Never content").unwrap();
        assert!(
            store
                .open_attachment("s1", "unknown-user.pdf")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .open_attachment("missing", "file.txt")
                .await
                .unwrap()
                .is_none()
        );
        let saved = store
            .save_attachment("s1", "présentation été.pdf", b"Original bytes".to_vec())
            .await
            .unwrap();
        let mut source = store
            .open_attachment("s1", &saved.attachment_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.attachment_id, saved.attachment_id);
        std::fs::rename(
            vault.path().join(&saved.relative_path),
            directory.join("retired-user.pdf"),
        )
        .unwrap();
        std::fs::write(
            vault.path().join(&saved.relative_path),
            b"Replacement bytes",
        )
        .unwrap();
        let mut original = vec![];
        source.file.read_to_end(&mut original).unwrap();
        assert_eq!(original, b"Original bytes");
        let mut current = store
            .open_attachment("s1", &saved.attachment_id)
            .await
            .unwrap()
            .unwrap();
        let mut replacement = vec![];
        current.file.read_to_end(&mut replacement).unwrap();
        assert_eq!(replacement, b"Replacement bytes");
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            store.save_note("s1", "Editing stays responsive", None),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            std::fs::read(directory.join("unknown-user.pdf")).unwrap(),
            b"Never content"
        );
        assert_eq!(
            std::fs::read(directory.join("retired-user.pdf")).unwrap(),
            b"Original bytes"
        );
        store
            .save_attachment(
                "s1",
                "_meta.json",
                b"Owned attachment, not session metadata".to_vec(),
            )
            .await
            .unwrap();
        assert!(
            store
                .open_attachment("s1", "_meta.json")
                .await
                .unwrap()
                .is_some()
        );
        for id in [
            "../_meta.json",
            "nested/file.txt",
            "nested\\file.txt",
            ".hidden",
            "..",
            "",
        ] {
            assert!(store.open_attachment("s1", id).await.is_err(), "{id:?}");
        }
        assert!(store.open_attachment("../s1", "file.txt").await.is_err());
        std::fs::create_dir(directory.join("attachments/directory.txt")).unwrap();
        assert!(store.open_attachment("s1", "directory.txt").await.is_err());
        store.set_startup_pending(true);
        assert!(
            store
                .open_attachment("s1", &saved.attachment_id)
                .await
                .is_err()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn attachment_readers_refuse_symlinked_files_and_directories() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Attachments")).await.unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("private.pdf"), b"Private outside bytes").unwrap();
        let directory = vault.path().join("sessions/s1/attachments");
        std::os::unix::fs::symlink(outside.path(), &directory).unwrap();
        assert!(store.open_attachment("s1", "private.pdf").await.is_err());
        std::fs::remove_file(&directory).unwrap();
        std::fs::create_dir(&directory).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("private.pdf"),
            directory.join("link.pdf"),
        )
        .unwrap();
        assert!(store.open_attachment("s1", "link.pdf").await.is_err());
        assert_eq!(
            std::fs::read(outside.path().join("private.pdf")).unwrap(),
            b"Private outside bytes"
        );
    }

    #[test]
    fn cancelled_attachment_save_keeps_its_reservation_until_blocking_io_completes() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let (store, vault) = test_store().await;
            store.write_meta(&meta("s1", "Standup")).await.unwrap();
            let (started, waiting) = tokio::sync::oneshot::channel();
            let (finish, completed) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                started.send(()).unwrap();
                completed
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
            });
            waiting.await.unwrap();
            let writer = store.clone();
            let request = tokio::spawn(async move {
                writer
                    .save_attachment("s1", "record.txt", b"Complete attachment".to_vec())
                    .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while store.write_lock.try_lock().is_ok() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            request.abort();
            assert!(request.await.unwrap_err().is_cancelled());
            let subsequent = store.lock_snapshot();
            tokio::pin!(subsequent);
            let held = tokio::time::timeout(std::time::Duration::from_millis(30), &mut subsequent)
                .await
                .is_err();
            finish.send(()).unwrap();
            blocker.await.unwrap();
            assert!(
                held,
                "request cancellation released the attachment writer's reservation"
            );
            let guard = tokio::time::timeout(std::time::Duration::from_secs(2), subsequent)
                .await
                .unwrap();
            assert_eq!(
                std::fs::read(vault.path().join("sessions/s1/attachments/record.txt")).unwrap(),
                b"Complete attachment"
            );
            drop(guard);
            let next = store
                .save_attachment("s1", "record.txt", b"Next attachment".to_vec())
                .await
                .unwrap();
            assert_eq!(next.attachment_id, "record 1.txt");
        });
    }

    #[test]
    fn windows_attachment_names_cannot_address_devices_or_alternate_data_streams() {
        for (name, expected) in [
            ("CON.txt", "_CON.txt"),
            ("aux", "_aux"),
            ("COM¹.pdf", "_COM¹.pdf"),
            ("CONOUT$", "_CONOUT$"),
            ("notes.txt:stream", "notes.txt_stream"),
            ("Agenda. ", "Agenda"),
            (".hidden", "_.hidden"),
        ] {
            assert_eq!(windows_basename(name), expected);
        }
        assert_eq!(windows_basename("Réunion notes.pdf"), "Réunion notes.pdf");
        let long = windows_basename(&format!("{}.pdf", "会議".repeat(100)));
        assert!(long.len() <= 180);
        assert!(long.ends_with(".pdf"));
    }

    #[tokio::test]
    #[cfg(target_os = "windows")]
    async fn windows_attachment_rename_is_returned_to_the_note_editor() {
        let (store, vault) = test_store().await;
        store.write_meta(&meta("s1", "Meeting")).await.unwrap();
        let saved = store
            .save_attachment("s1", "NUL.txt", b"ordinary attachment".to_vec())
            .await
            .unwrap();
        assert_eq!(saved.attachment_id, "_NUL.txt");
        assert_eq!(
            std::fs::read(vault.path().join(saved.relative_path)).unwrap(),
            b"ordinary attachment"
        );
    }

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
        use std::io::Read;
        let mut source = store
            .open_attachment("abc123", &saved.attachment_id)
            .await
            .unwrap()
            .unwrap();
        let mut bytes = vec![];
        source.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"png-bytes");
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
