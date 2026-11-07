use hypr_loofah_protocol::{MetadataSaveOutcome, MetadataSnapshot};

use crate::{SessionMetaPatch, SessionStore, StoreError, WriteGuard, paths};

impl SessionStore {
    pub async fn read_recovered_metadata_snapshot(
        &self,
        operation_root: Option<&std::path::Path>,
        id: &str,
    ) -> Result<Option<MetadataSnapshot>, StoreError> {
        self.recovered_read(operation_root, id, |store, snapshot, id| async move {
            store.metadata_snapshot_locked(&snapshot._guard, &id).await
        })
        .await
    }

    pub async fn read_metadata_snapshot(
        &self,
        id: &str,
    ) -> Result<Option<MetadataSnapshot>, StoreError> {
        crate::validate_session_id(id)?;
        let guard = self.lock_writes().await;
        self.metadata_snapshot_locked(&guard, id).await
    }

    pub async fn update_metadata_if_revision(
        &self,
        id: &str,
        patch: SessionMetaPatch,
        expected_revision: &str,
    ) -> Result<Option<MetadataSaveOutcome>, StoreError> {
        self.update_metadata_with_recovery(None, id, patch, expected_revision)
            .await
    }

    pub async fn update_metadata_with_recovery(
        &self,
        operation_root: Option<&std::path::Path>,
        id: &str,
        patch: SessionMetaPatch,
        expected_revision: &str,
    ) -> Result<Option<MetadataSaveOutcome>, StoreError> {
        let expected_revision = expected_revision.to_owned();
        self.recovered_read(operation_root, id, move |store, snapshot, id| async move {
            store
                .update_metadata_locked(&snapshot._guard, &id, patch, &expected_revision)
                .await
        })
        .await
    }

    async fn update_metadata_locked(
        &self,
        guard: &WriteGuard<'_>,
        id: &str,
        patch: SessionMetaPatch,
        expected_revision: &str,
    ) -> Result<Option<MetadataSaveOutcome>, StoreError> {
        self.ensure_ready()?;
        let Some(current) = self.metadata_snapshot_locked(guard, id).await? else {
            return Ok(None);
        };
        if current.revision != expected_revision {
            return Ok(Some(MetadataSaveOutcome::Conflict(current)));
        }
        let mut meta = current.meta;
        crate::content::apply_meta_patch(&mut meta, patch);
        validate_metadata_size(&meta, self.content_limits.metadata_bytes)?;
        self.write_meta_locked(guard, &meta).await?;
        let snapshot = self
            .metadata_snapshot_locked(guard, id)
            .await?
            .ok_or_else(|| {
                StoreError::Io("Session disappeared after metadata replacement".into())
            })?;
        Ok(Some(MetadataSaveOutcome::Saved(snapshot)))
    }

    pub(super) async fn metadata_snapshot_locked(
        &self,
        _guard: &WriteGuard<'_>,
        id: &str,
    ) -> Result<Option<MetadataSnapshot>, StoreError> {
        let vault = self.vault_base.clone();
        let directory = paths::validated_session_dir(id)?;
        let id = id.to_owned();
        let limit = self.content_limits.metadata_bytes;
        tokio::task::spawn_blocking(move || {
            let path = vault.join(paths::meta_path_in(&directory));
            for (candidate, is_file) in [
                (vault.join("sessions"), false),
                (vault.join(&directory), false),
                (path.clone(), true),
            ] {
                match std::fs::symlink_metadata(candidate) {
                    Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                    Err(cause) => return Err(StoreError::Io(cause.to_string())),
                    Ok(meta)
                        if meta.file_type().is_symlink()
                            || (is_file && !meta.is_file())
                            || (!is_file && !meta.is_dir()) =>
                    {
                        return Err(StoreError::Io(
                            "Session metadata path is not a regular file or directory".into(),
                        ));
                    }
                    Ok(meta) if is_file && meta.len() > limit as u64 => {
                        return Err(StoreError::ResourceLimit(
                            "Session metadata exceeds its size limit".into(),
                        ));
                    }
                    Ok(_) => {}
                }
            }
            use std::io::Read;
            let mut raw = Vec::new();
            match std::fs::File::open(path) {
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(cause) => return Err(StoreError::Io(cause.to_string())),
                Ok(file) => file
                    .take(limit.saturating_add(1) as u64)
                    .read_to_end(&mut raw)
                    .map_err(|cause| StoreError::Io(cause.to_string()))?,
            };
            if raw.len() > limit {
                return Err(StoreError::ResourceLimit(
                    "Session metadata exceeds its size limit".into(),
                ));
            }
            let meta: hypr_vault_read::SessionMeta = serde_json::from_slice(&raw)
                .map_err(|cause| StoreError::Serialize(cause.to_string()))?;
            if meta.id != id {
                return Err(StoreError::Serialize(
                    "Session metadata identity does not match its directory".into(),
                ));
            }
            let mut bytes = b"loofah/session-meta/v1\0".to_vec();
            bytes.extend_from_slice(&(id.len() as u64).to_be_bytes());
            bytes.extend_from_slice(id.as_bytes());
            bytes.extend_from_slice(&raw);
            Ok(Some(MetadataSnapshot {
                meta,
                revision: crate::sha256(&bytes),
            }))
        })
        .await
        .map_err(|cause| StoreError::Io(cause.to_string()))?
    }
}

pub(super) fn validate_metadata_size(
    meta: &crate::SessionMeta,
    limit: usize,
) -> Result<(), StoreError> {
    if limit == usize::MAX {
        return Ok(());
    }
    let bytes = serde_json::to_vec_pretty(meta)
        .map_err(|cause| StoreError::Serialize(cause.to_string()))?;
    if bytes.len() > limit {
        return Err(StoreError::ResourceLimit(
            "Session metadata exceeds its size limit".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoteSaveOutcome;
    use serde_json::json;
    const METADATA_LIMIT: usize = 8 * 1024 * 1024;

    async fn fixture() -> (tempfile::TempDir, SessionStore) {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().into());
        let meta = serde_json::from_value(json!({"id":"note","title":"Original","created_at":"2026-11-05T10:00:00Z","tags":[],"tag_suggestions":{"items":["project","other"],"dismissed":["ignored"]},"future":{"keep":true},"skill":"writer"})).unwrap();
        store.create_note_session(&meta, "User note").await.unwrap();
        std::fs::write(
            root.path().join("sessions/note/foreign.txt"),
            "User attachment",
        )
        .unwrap();
        (root, store)
    }

    #[tokio::test]
    async fn checked_patches_share_existing_semantics_and_preserve_unrelated_files_and_unknown_metadata()
     {
        let (root, store) = fixture().await;
        let current = store.read_metadata_snapshot("note").await.unwrap().unwrap();
        let patch = SessionMetaPatch {
            title: Some("Updated".into()),
            created_at: Some("2026-11-06T10:00:00Z".into()),
            started_at: Some("".into()),
            ended_at: Some("2026-11-06T11:00:00Z".into()),
            tags: Some(vec!["project".into()]),
            tracking_id: Some("tracking".into()),
            folder: Some("folder".into()),
            author: Some("agent".into()),
        };
        let MetadataSaveOutcome::Saved(saved) = store
            .update_metadata_if_revision("note", patch.clone(), &current.revision)
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("save failed");
        };
        assert_eq!(saved.meta.extra.get("future"), Some(&json!({"keep":true})));
        assert_eq!(saved.meta.skill.as_deref(), Some("writer"));
        assert_eq!(
            saved.meta.tag_suggestions.as_ref().unwrap().items,
            ["other"]
        );
        assert_eq!(
            saved.meta.tag_suggestions.as_ref().unwrap().dismissed,
            ["ignored"]
        );
        assert_eq!(saved.meta.started_at.as_deref(), Some(""));
        assert_eq!(
            std::fs::read_to_string(root.path().join("sessions/note/notes.md")).unwrap(),
            "User note"
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("sessions/note/foreign.txt")).unwrap(),
            "User attachment"
        );
        let (_legacy_root, legacy) = fixture().await;
        legacy.update_meta("note", patch).await.unwrap();
        assert_eq!(legacy.read_meta("note").await.unwrap().unwrap(), saved.meta);
        assert_eq!(store.session_get("note").unwrap().meta, saved.meta);
    }

    #[tokio::test]
    async fn two_metadata_replacements_from_the_same_revision_have_one_winner() {
        let (_root, store) = fixture().await;
        let baseline = store.read_metadata_snapshot("note").await.unwrap().unwrap();
        let (first, second) = tokio::join!(
            store.update_metadata_if_revision(
                "note",
                SessionMetaPatch {
                    title: Some("First".into()),
                    ..Default::default()
                },
                &baseline.revision
            ),
            store.update_metadata_if_revision(
                "note",
                SessionMetaPatch {
                    title: Some("Second".into()),
                    ..Default::default()
                },
                &baseline.revision
            )
        );
        let outcomes = [first.unwrap().unwrap(), second.unwrap().unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, MetadataSaveOutcome::Saved(_)))
                .count(),
            1
        );
        let actual = store.read_metadata_snapshot("note").await.unwrap().unwrap();
        for outcome in outcomes {
            match outcome {
                MetadataSaveOutcome::Saved(value) | MetadataSaveOutcome::Conflict(value) => {
                    assert_eq!(value, actual)
                }
            }
        }
    }

    #[tokio::test]
    async fn raw_metadata_changes_conflict_while_independent_note_edits_keep_metadata_revisions() {
        let (root, store) = fixture().await;
        let original = store.read_metadata_snapshot("note").await.unwrap().unwrap();
        let note = store.read_note_snapshot("note").await.unwrap().unwrap();
        store
            .save_note_if_revision("note", "Changed body", None, &note.revision)
            .await
            .unwrap();
        assert_eq!(
            store.read_metadata_snapshot("note").await.unwrap().unwrap(),
            original
        );
        let MetadataSaveOutcome::Saved(tags) = store
            .update_metadata_if_revision(
                "note",
                SessionMetaPatch {
                    tags: Some(vec!["project".into()]),
                    ..Default::default()
                },
                &original.revision,
            )
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("metadata save failed");
        };
        let path = root.path().join("sessions/note/_meta.json");
        let raw = std::fs::read(&path).unwrap();
        let mut edited = raw.clone();
        edited.push(b'\n');
        std::fs::write(&path, &edited).unwrap();
        let MetadataSaveOutcome::Conflict(latest) = store
            .update_metadata_if_revision(
                "note",
                SessionMetaPatch {
                    title: Some("Stale".into()),
                    ..Default::default()
                },
                &tags.revision,
            )
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("raw edit was overwritten");
        };
        assert_eq!(latest.meta, tags.meta);
        assert_ne!(latest.revision, tags.revision);
        assert_eq!(std::fs::read(&path).unwrap(), edited);
        let current_note = store.read_note_snapshot("note").await.unwrap().unwrap();
        store
            .update_metadata_if_revision(
                "note",
                SessionMetaPatch {
                    title: Some("Renamed".into()),
                    ..Default::default()
                },
                &latest.revision,
            )
            .await
            .unwrap();
        assert!(matches!(
            store
                .save_note_if_revision(
                    "note",
                    "Stale body",
                    Some("Original"),
                    &current_note.revision
                )
                .await
                .unwrap(),
            NoteSaveOutcome::Conflict(_)
        ));
        assert_eq!(
            store.read_note("note").await.unwrap().as_deref(),
            Some("Changed body")
        );
    }

    #[tokio::test]
    async fn missing_deleted_corrupt_and_oversized_metadata_never_creates_a_replacement() {
        let (root, store) = fixture().await;
        assert!(
            store
                .read_metadata_snapshot("missing")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .update_metadata_if_revision("missing", Default::default(), "revision")
                .await
                .unwrap()
                .is_none()
        );
        assert!(!root.path().join("sessions/missing").exists());
        for id in ["../outside", ".hidden", "CON", "a/b"] {
            assert!(store.read_metadata_snapshot(id).await.is_err());
        }
        let path = root.path().join("sessions/note/_meta.json");
        for bytes in [
            b"invalid JSON".to_vec(),
            serde_json::to_vec(&json!({"id":"other","title":"Other","created_at":"","tags":[]}))
                .unwrap(),
            vec![b' '; METADATA_LIMIT + 1],
        ] {
            std::fs::write(&path, &bytes).unwrap();
            assert!(store.read_metadata_snapshot("note").await.is_err());
            assert!(
                store
                    .update_metadata_if_revision(
                        "note",
                        SessionMetaPatch {
                            title: Some("New".into()),
                            ..Default::default()
                        },
                        "revision"
                    )
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
        let (deleted_root, deleted) = fixture().await;
        let before = deleted
            .read_metadata_snapshot("note")
            .await
            .unwrap()
            .unwrap();
        deleted.delete_session("note").await.unwrap();
        assert!(
            deleted
                .update_metadata_if_revision(
                    "note",
                    SessionMetaPatch {
                        title: Some("Recreate".into()),
                        ..Default::default()
                    },
                    &before.revision
                )
                .await
                .unwrap()
                .is_none()
        );
        assert!(!deleted_root.path().join("sessions/note").exists());
    }

    #[tokio::test]
    async fn growth_beyond_the_metadata_limit_is_rejected_before_committing() {
        let (root, store) = fixture().await;
        let mut meta = store.read_meta("note").await.unwrap().unwrap();
        meta.extra
            .insert("large".into(), json!("x".repeat(7 * 1024 * 1024)));
        store.write_meta(&meta).await.unwrap();
        let before = store.read_metadata_snapshot("note").await.unwrap().unwrap();
        let path = root.path().join("sessions/note/_meta.json");
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            store
                .update_metadata_if_revision(
                    "note",
                    SessionMetaPatch {
                        title: Some("x".repeat(2 * 1024 * 1024)),
                        ..Default::default()
                    },
                    &before.revision
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn metadata_symlinks_cannot_read_or_replace_another_file() {
        let (root, store) = fixture().await;
        let before = store.read_metadata_snapshot("note").await.unwrap().unwrap();
        let foreign = root.path().join("foreign.json");
        let bytes = serde_json::to_vec(&before.meta).unwrap();
        std::fs::write(&foreign, &bytes).unwrap();
        let path = root.path().join("sessions/note/_meta.json");
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&foreign, &path).unwrap();
        assert!(store.read_metadata_snapshot("note").await.is_err());
        assert!(
            store
                .update_metadata_if_revision(
                    "note",
                    SessionMetaPatch {
                        title: Some("Changed".into()),
                        ..Default::default()
                    },
                    &before.revision
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(&foreign).unwrap(), bytes);
    }
}
