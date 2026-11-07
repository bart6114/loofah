use std::collections::BTreeMap;

use super::{SessionStore, StoreError, validate_session_id};

pub struct AttachmentFile {
    pub filename: String,
    pub size_bytes: u64,
}

impl SessionStore {
    // One lookahead entry lets the application construct a cursor without rescanning.
    pub async fn list_attachments(
        &self,
        id: &str,
        limit: usize,
        after: Option<&str>,
    ) -> Result<Option<Vec<AttachmentFile>>, StoreError> {
        validate_session_id(id)?;
        if !(1..=200).contains(&limit) {
            return Err(StoreError::Serialize("Invalid attachment page size".into()));
        }
        if let Some(after) = after {
            hypr_vault_read::paths::validate_attachment_basename(after)?;
        }
        let store = self.clone();
        let id = id.to_owned();
        let after = after.map(str::to_owned);
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
            tokio::task::spawn_blocking(move || {
                let _snapshot = snapshot;
                let directory_meta = match std::fs::symlink_metadata(&directory) {
                    Ok(meta) => meta,
                    Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Some(Vec::new()));
                    }
                    Err(cause) => return Err(StoreError::Io(cause.to_string())),
                };
                if !directory_meta.is_dir() || directory_meta.file_type().is_symlink() {
                    return Err(StoreError::Io(
                        "Attachment directory is not a regular directory".into(),
                    ));
                }
                scan_attachments(directory, limit, after.as_deref(), 50_000).map(Some)
            })
            .await
            .map_err(|cause| StoreError::Io(format!("Attachment listing worker failed: {cause}")))?
        })
        .await
        .map_err(|cause| StoreError::Io(format!("Attachment listing request failed: {cause}")))?
    }
}

fn scan_attachments(
    directory: std::path::PathBuf,
    limit: usize,
    after: Option<&str>,
    entry_limit: usize,
) -> Result<Vec<AttachmentFile>, StoreError> {
    let entries =
        std::fs::read_dir(directory).map_err(|cause| StoreError::Io(cause.to_string()))?;
    let mut selected = BTreeMap::new();
    for (count, entry) in entries.enumerate() {
        if count >= entry_limit {
            return Err(StoreError::ResourceLimit(
                "Attachment directory exceeds the scan limit".into(),
            ));
        }
        let entry = entry.map_err(|cause| StoreError::Io(cause.to_string()))?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if hypr_vault_read::paths::validate_attachment_id(&name).is_err()
            || after.is_some_and(|cursor| name.as_str() <= cursor)
            || (selected.len() == limit + 1
                && selected
                    .last_key_value()
                    .is_some_and(|(last, _)| name >= *last))
        {
            continue;
        }
        let file_type = entry
            .file_type()
            .map_err(|cause| StoreError::Io(cause.to_string()))?;
        if !file_type.is_file() || file_type.is_symlink() {
            continue;
        }
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|cause| StoreError::Io(cause.to_string()))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            continue;
        }
        selected.insert(name, metadata.len());
        if selected.len() > limit + 1 {
            selected.pop_last();
        }
    }
    Ok(selected
        .into_iter()
        .map(|(filename, size_bytes)| AttachmentFile {
            filename,
            size_bytes,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn fixture() -> (tempfile::TempDir, SessionStore) {
        let vault = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().into());
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Title","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Keep note unchanged")
            .await
            .unwrap();
        (vault, store)
    }

    #[tokio::test]
    async fn listing_is_shallow_sorted_bounded_and_does_not_create_directories() {
        let (vault, store) = fixture().await;
        assert!(
            store
                .list_attachments("note", 2, None)
                .await
                .unwrap()
                .unwrap()
                .is_empty()
        );
        let directory = vault.path().join("sessions/note");
        assert!(!directory.join("attachments").exists());
        assert!(
            store
                .list_attachments("missing", 2, None)
                .await
                .unwrap()
                .is_none()
        );
        std::fs::write(
            directory.join("unknown-user.pdf"),
            "Never enumerate this file",
        )
        .unwrap();
        for name in ["z.txt", "a.txt", "ü.pdf", "b.png"] {
            store
                .save_attachment("note", name, b"original".to_vec())
                .await
                .unwrap();
        }
        std::fs::write(directory.join("attachments/.hidden"), "private").unwrap();
        std::fs::create_dir(directory.join("attachments/nested")).unwrap();
        std::fs::write(
            directory.join("attachments/nested/inside.txt"),
            "not a direct attachment",
        )
        .unwrap();
        let entries = store
            .list_attachments("note", 2, None)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.filename.as_str(), entry.size_bytes))
                .collect::<Vec<_>>(),
            vec![("a.txt", 8), ("b.png", 8), ("z.txt", 8)]
        );
        let next = store
            .list_attachments("note", 2, Some("b.png"))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            next.iter()
                .map(|entry| entry.filename.as_str())
                .collect::<Vec<_>>(),
            ["z.txt", "ü.pdf"]
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("notes.md")).unwrap(),
            "Keep note unchanged"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("unknown-user.pdf")).unwrap(),
            "Never enumerate this file"
        );
    }

    #[tokio::test]
    async fn invalid_pages_fail_before_touching_vault_paths() {
        let (_vault, store) = fixture().await;
        for limit in [0, 201, usize::MAX] {
            assert!(store.list_attachments("note", limit, None).await.is_err());
        }
        for cursor in ["", ".hidden", "../a", "a/b", "a\\b"] {
            assert!(
                store
                    .list_attachments("note", 1, Some(cursor))
                    .await
                    .is_err()
            );
        }
        assert!(store.list_attachments("../note", 1, None).await.is_err());
        store.set_startup_pending(true);
        assert!(store.list_attachments("note", 1, None).await.is_err());
    }

    #[test]
    fn hidden_entries_still_count_toward_the_scan_budget() {
        let directory = tempfile::tempdir().unwrap();
        for name in [".a", ".b", "visible.txt"] {
            std::fs::write(directory.path().join(name), b"original").unwrap();
        }
        let error = scan_attachments(directory.path().into(), 1, None, 2)
            .err()
            .unwrap();
        assert!(matches!(error, StoreError::ResourceLimit(_)));
        assert_eq!(
            scan_attachments(directory.path().into(), 1, None, 3).unwrap()[0].filename,
            "visible.txt"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn listing_refuses_linked_directories_and_skips_linked_files() {
        use std::os::unix::fs::symlink;
        let (vault, store) = fixture().await;
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("private.txt"), "private").unwrap();
        let directory = vault.path().join("sessions/note/attachments");
        symlink(outside.path(), &directory).unwrap();
        assert!(store.list_attachments("note", 10, None).await.is_err());
        std::fs::remove_file(&directory).unwrap();
        std::fs::create_dir(&directory).unwrap();
        symlink(
            outside.path().join("private.txt"),
            directory.join("linked.txt"),
        )
        .unwrap();
        assert!(
            store
                .list_attachments("note", 10, None)
                .await
                .unwrap()
                .unwrap()
                .is_empty()
        );
    }
}
