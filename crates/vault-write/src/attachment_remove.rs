use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::Read,
    path::{Path, PathBuf},
};

use crate::{SessionStore, StoreError, file_snapshot::FileSnapshot, media::open_regular_file};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentRemoval {
    Removed,
    Absent,
    Changed,
    Busy,
    Uncertain,
    Rejected,
}

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct Request {
    session: String,
    attachment: String,
    expected_sha256: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    request: Request,
    target: Option<String>,
    state: State,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Pending,
    Removed,
    Absent,
    Uncertain,
    Rejected,
}

pub struct AttachmentRemovalPlan {
    store: SessionStore,
    path: PathBuf,
    request: Request,
    initial: Option<FileSnapshot>,
    existing: Option<Receipt>,
    finished: Option<AttachmentRemoval>,
    _serial: tokio::sync::OwnedMutexGuard<()>,
}

impl SessionStore {
    pub async fn prepare_attachment_removal(
        &self,
        operations: &Path,
        namespace: &str,
        request_id: uuid::Uuid,
        session: &str,
        attachment: &str,
        expected_sha256: &str,
    ) -> Result<Option<AttachmentRemovalPlan>, StoreError> {
        crate::validate_session_id(session)?;
        hypr_vault_read::paths::validate_attachment_id(attachment)?;
        if expected_sha256.len() != 64
            || !expected_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(StoreError::Serialize(
                "A lowercase SHA-256 attachment revision is required".into(),
            ));
        }
        let request = Request {
            session: session.into(),
            attachment: attachment.into(),
            expected_sha256: expected_sha256.into(),
        };
        let key = operation_key(namespace, request_id);
        let path = operations.join(format!("{key}.json"));
        let store = self.clone();
        tokio::spawn(async move {
            let serial = store.attachment_removals.clone().lock_owned().await;
            let read_path = path.clone();
            let existing = tokio::task::spawn_blocking(move || read_receipt(&read_path))
                .await
                .map_err(worker_error)??;
            if let Some(receipt) = &existing
                && receipt.request != request
            {
                return Err(StoreError::Conflict(
                    "Attachment removal receipt does not match this request".into(),
                ));
            }
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            if existing.is_none()
                && store
                    .metadata_snapshot_locked(&snapshot._guard, &request.session)
                    .await?
                    .is_none()
            {
                return Ok(None);
            }
            let source = store
                .vault_base
                .join("sessions")
                .join(&request.session)
                .join("attachments")
                .join(&request.attachment);
            let fresh = existing.is_none();
            let initial = tokio::task::spawn_blocking(move || {
                let _snapshot = snapshot;
                if !fresh {
                    return Ok(None);
                }
                open_candidate(&source)
            })
            .await
            .map_err(worker_error)??;
            let request_hash = request.expected_sha256.clone();
            let (initial, finished) = tokio::task::spawn_blocking(move || {
                if let Some(file) = &initial {
                    if file.hash()? != request_hash {
                        return Ok((initial, Some(AttachmentRemoval::Changed)));
                    }
                    if !file.lock_for_recheck()? {
                        return Ok((initial, Some(AttachmentRemoval::Busy)));
                    }
                    if file.hash()? != request_hash {
                        return Ok((initial, Some(AttachmentRemoval::Changed)));
                    }
                }
                Ok::<_, StoreError>((initial, None))
            })
            .await
            .map_err(worker_error)??;
            Ok(Some(AttachmentRemovalPlan {
                store,
                path,
                request,
                initial,
                existing,
                finished,
                _serial: serial,
            }))
        })
        .await
        .map_err(worker_error)?
    }
}

impl AttachmentRemovalPlan {
    pub async fn commit(self) -> Result<AttachmentRemoval, StoreError> {
        self.commit_with(|| Ok(())).await
    }

    async fn commit_with(
        self,
        after_move: impl FnOnce() -> Result<(), StoreError> + Send + 'static,
    ) -> Result<AttachmentRemoval, StoreError> {
        tokio::spawn(async move {
            let snapshot = self.store.lock_snapshot().await;
            self.store.ensure_ready()?;
            if self.existing.is_none() {
                if self
                    .store
                    .metadata_snapshot_locked(&snapshot._guard, &self.request.session)
                    .await?
                    .is_none()
                {
                    return Ok(AttachmentRemoval::Changed);
                }
                self.store
                    .session_dir_locked(&snapshot._guard, &self.request.session)
                    .await?;
            }
            let source = self
                .store
                .vault_base
                .join("sessions")
                .join(&self.request.session)
                .join("attachments")
                .join(&self.request.attachment);
            tokio::task::spawn_blocking(move || {
                let _snapshot = snapshot;
                let _serial = self._serial;
                if let Some(result) = self.finished {
                    return Ok(result);
                }
                if let Some(mut receipt) = self.existing.clone() {
                    if receipt.schema_version != 1 {
                        return Ok(AttachmentRemoval::Uncertain);
                    }
                    let result = match receipt.state {
                        State::Removed => AttachmentRemoval::Removed,
                        State::Absent => AttachmentRemoval::Absent,
                        State::Uncertain => AttachmentRemoval::Uncertain,
                        State::Rejected => AttachmentRemoval::Rejected,
                        State::Pending => {
                            // Never resume an interrupted destructive action against the live name.
                            let target = receipt.target.as_deref().ok_or_else(|| {
                                StoreError::Io("Missing attachment trash target".into())
                            })?;
                            let target =
                                validate_target(self.store.vault_base(), &self.request, target)?;
                            if let Some(file) = open_candidate(&target)? {
                                if file.hash()? == self.request.expected_sha256 {
                                    receipt.state = State::Removed;
                                    AttachmentRemoval::Removed
                                } else {
                                    receipt.state = State::Uncertain;
                                    AttachmentRemoval::Uncertain
                                }
                            } else {
                                receipt.state = State::Uncertain;
                                AttachmentRemoval::Uncertain
                            }
                        }
                    };
                    persist(&self.path, &receipt)?;
                    return Ok(result);
                }
                let current = open_candidate(&source)?;
                let same = match (&self.initial, &current) {
                    (None, None) => true,
                    (Some(before), Some(after)) => before.unchanged(after)?,
                    _ => false,
                };
                if !same {
                    return Ok(AttachmentRemoval::Changed);
                }
                let mut receipt = Receipt {
                    schema_version: 1,
                    request: self.request.clone(),
                    target: None,
                    state: State::Absent,
                };
                if current.is_none() {
                    persist(&self.path, &receipt)?;
                    return Ok(AttachmentRemoval::Absent);
                }
                let target = hypr_fs_sync_core::export::prepare_trash_destination(
                    self.store.vault_base(),
                    &source,
                )
                .map_err(fs_error)?;
                let relative = target
                    .strip_prefix(self.store.vault_base())
                    .map_err(|_| StoreError::Io("Invalid attachment trash target".into()))?;
                let relative = relative
                    .components()
                    .map(|part| {
                        part.as_os_str()
                            .to_str()
                            .ok_or_else(|| StoreError::Io("Invalid attachment trash name".into()))
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .join("/");
                receipt.target = Some(relative);
                receipt.state = State::Pending;
                persist(&self.path, &receipt)?;
                if let Err(cause) = hypr_storage::fs::rename_no_replace(&source, &target) {
                    receipt.state = State::Rejected;
                    persist(&self.path, &receipt)?;
                    return Err(io_error(cause));
                }
                self.store.notify_attachment_changed(
                    &self.request.session,
                    Some(&self.request.attachment),
                );
                sync_parent(&source)?;
                sync_parent(&target)?;
                after_move()?;
                receipt.state = State::Removed;
                persist(&self.path, &receipt)?;
                Ok(AttachmentRemoval::Removed)
            })
            .await
            .map_err(worker_error)?
        })
        .await
        .map_err(worker_error)?
    }
}

fn operation_key(namespace: &str, request_id: uuid::Uuid) -> String {
    Sha256::digest(serde_json::to_vec(&(namespace, request_id)).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn open_candidate(path: &Path) -> Result<Option<FileSnapshot>, StoreError> {
    if let Some(parent) = path.parent() {
        match std::fs::symlink_metadata(parent) {
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(cause) => return Err(io_error(cause)),
            Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                return Err(StoreError::Io("Attachment directory is not regular".into()));
            }
            Ok(_) => {}
        }
    }
    open_regular_file(path)?.map(FileSnapshot::new).transpose()
}
fn validate_target(root: &Path, request: &Request, target: &str) -> Result<PathBuf, StoreError> {
    let parts = target.split('/').collect::<Vec<_>>();
    if parts.len() != 6
        || parts[0] != ".trash"
        || chrono::NaiveDate::parse_from_str(parts[1], "%Y-%m-%d").is_err()
        || parts[2] != "sessions"
        || parts[3] != request.session
        || parts[4] != "attachments"
        || hypr_vault_read::paths::validate_attachment_id(parts[5]).is_err()
    {
        return Err(StoreError::Io("Invalid attachment trash receipt".into()));
    }
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
        if let Ok(meta) = std::fs::symlink_metadata(&path)
            && meta.file_type().is_symlink()
        {
            return Err(StoreError::Io("Linked attachment trash path".into()));
        }
    }
    Ok(path)
}
fn read_receipt(path: &Path) -> Result<Option<Receipt>, StoreError> {
    let Some(file) = open_regular_file(path)? else {
        return Ok(None);
    };
    if file.metadata().map_err(io_error)?.len() > 64 * 1024 {
        return Err(StoreError::Io("Invalid attachment removal receipt".into()));
    }
    let mut bytes = Vec::new();
    file.take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > 64 * 1024 {
        return Err(StoreError::Io("Invalid attachment removal receipt".into()));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| StoreError::Io("Invalid attachment removal receipt".into()))
}
fn persist(path: &Path, receipt: &Receipt) -> Result<(), StoreError> {
    ensure_operation_directory(
        path.parent()
            .ok_or_else(|| StoreError::Io("Missing removal receipt directory".into()))?,
    )?;
    hypr_storage::fs::atomic_write(
        path,
        &serde_json::to_string(receipt)
            .map_err(|cause| StoreError::Serialize(cause.to_string()))?,
    )
    .map_err(io_error)?;
    sync_parent(path)
}
fn ensure_operation_directory(path: &Path) -> Result<(), StoreError> {
    if path.is_dir() {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Io("Invalid removal receipt directory".into()))?;
    ensure_operation_directory(parent)?;
    match std::fs::create_dir(path) {
        Ok(()) => sync_parent(path),
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(cause) => Err(io_error(cause)),
    }
}
fn sync_parent(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    std::fs::File::open(path.parent().unwrap())
        .and_then(|parent| parent.sync_all())
        .map_err(io_error)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn worker_error(cause: tokio::task::JoinError) -> StoreError {
    StoreError::Io(format!("Attachment removal worker failed: {cause}"))
}
fn io_error(cause: std::io::Error) -> StoreError {
    StoreError::Io(cause.to_string())
}
fn fs_error(cause: hypr_fs_sync_core::Error) -> StoreError {
    StoreError::Io(cause.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn sha(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    async fn fixture() -> (tempfile::TempDir, tempfile::TempDir, SessionStore) {
        let root = tempfile::tempdir().unwrap();
        let operations = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().into());
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Title","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Canonical note stays unchanged")
            .await
            .unwrap();
        (root, operations, store)
    }
    async fn prepare(
        store: &SessionStore,
        root: &Path,
        id: uuid::Uuid,
        hash: &str,
    ) -> AttachmentRemovalPlan {
        store
            .prepare_attachment_removal(root, "alice", id, "note", "file.pdf", hash)
            .await
            .unwrap()
            .unwrap()
    }
    fn receipt_path(root: &Path, id: uuid::Uuid) -> PathBuf {
        root.join(format!("{}.json", operation_key("alice", id)))
    }
    #[tokio::test]
    async fn successful_removal_keeps_original_in_trash_and_receipt_retry_cannot_remove_restored_bytes()
     {
        let (vault, operations, store) = fixture().await;
        let source = vault.path().join("sessions/note/attachments/file.pdf");
        store
            .save_attachment("note", "file.pdf", b"original".to_vec())
            .await
            .unwrap();
        std::fs::write(
            vault.path().join("sessions/note/unknown.pdf"),
            b"User owned",
        )
        .unwrap();
        let id = uuid::Uuid::new_v4();
        let hash = sha(b"original");
        assert_eq!(
            prepare(&store, operations.path(), id, &hash)
                .await
                .commit()
                .await
                .unwrap(),
            AttachmentRemoval::Removed
        );
        assert!(!source.exists());
        let receipt = read_receipt(&receipt_path(operations.path(), id))
            .unwrap()
            .unwrap();
        let target = validate_target(
            vault.path(),
            &receipt.request,
            receipt.target.as_deref().unwrap(),
        )
        .unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"original");
        std::fs::rename(&target, &source).unwrap();
        assert_eq!(
            prepare(&store, operations.path(), id, &hash)
                .await
                .commit()
                .await
                .unwrap(),
            AttachmentRemoval::Removed
        );
        assert_eq!(std::fs::read(&source).unwrap(), b"original");
        let fresh = uuid::Uuid::new_v4();
        assert_eq!(
            prepare(&store, operations.path(), fresh, &hash)
                .await
                .commit()
                .await
                .unwrap(),
            AttachmentRemoval::Removed
        );
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/note/notes.md")).unwrap(),
            "Canonical note stays unchanged"
        );
        assert_eq!(
            std::fs::read(vault.path().join("sessions/note/unknown.pdf")).unwrap(),
            b"User owned"
        );
    }
    #[tokio::test]
    async fn stale_hash_and_replaced_generation_are_refused_without_overwriting_a_new_file() {
        let (vault, operations, store) = fixture().await;
        store
            .save_attachment("note", "file.pdf", b"original".to_vec())
            .await
            .unwrap();
        let id = uuid::Uuid::new_v4();
        assert_eq!(
            prepare(&store, operations.path(), id, &sha(b"different"))
                .await
                .commit()
                .await
                .unwrap(),
            AttachmentRemoval::Changed
        );
        let pending = prepare(&store, operations.path(), id, &sha(b"original")).await;
        let directory = vault.path().join("sessions/note/attachments");
        std::fs::write(directory.join("replacement"), b"new bytes").unwrap();
        hypr_storage::fs::rename_with_retry(
            &directory.join("replacement"),
            &directory.join("file.pdf"),
        )
        .unwrap();
        assert_eq!(pending.commit().await.unwrap(), AttachmentRemoval::Changed);
        assert_eq!(
            std::fs::read(directory.join("file.pdf")).unwrap(),
            b"new bytes"
        );
        assert!(!vault.path().join(".trash").exists());
    }
    #[tokio::test]
    async fn absence_is_idempotent_and_reusing_an_id_for_other_content_is_refused() {
        let (vault, operations, store) = fixture().await;
        let id = uuid::Uuid::new_v4();
        let hash = sha(b"original");
        assert_eq!(
            prepare(&store, operations.path(), id, &hash)
                .await
                .commit()
                .await
                .unwrap(),
            AttachmentRemoval::Absent
        );
        assert!(!vault.path().join("sessions/note/attachments").exists());
        store
            .save_attachment("note", "file.pdf", b"original".to_vec())
            .await
            .unwrap();
        assert_eq!(
            prepare(&store, operations.path(), id, &hash)
                .await
                .commit()
                .await
                .unwrap(),
            AttachmentRemoval::Absent
        );
        assert!(
            vault
                .path()
                .join("sessions/note/attachments/file.pdf")
                .exists()
        );
        assert!(
            store
                .prepare_attachment_removal(
                    operations.path(),
                    "alice",
                    id,
                    "note",
                    "different.pdf",
                    &hash
                )
                .await
                .is_err()
        );
        assert!(
            store
                .prepare_attachment_removal(
                    operations.path(),
                    "alice",
                    uuid::Uuid::new_v4(),
                    "missing",
                    "file.pdf",
                    &hash
                )
                .await
                .unwrap()
                .is_none()
        );
    }
    #[tokio::test]
    async fn a_pending_intent_never_resumes_deletion_of_the_live_name() {
        let (vault, operations, store) = fixture().await;
        let id = uuid::Uuid::new_v4();
        let hash = sha(b"original");
        store
            .save_attachment("note", "file.pdf", b"original".to_vec())
            .await
            .unwrap();
        let request = Request {
            session: "note".into(),
            attachment: "file.pdf".into(),
            expected_sha256: hash.clone(),
        };
        let target = format!(
            ".trash/{}/sessions/note/attachments/file.pdf",
            chrono::Utc::now().format("%Y-%m-%d")
        );
        persist(
            &receipt_path(operations.path(), id),
            &Receipt {
                schema_version: 1,
                request,
                target: Some(target),
                state: State::Pending,
            },
        )
        .unwrap();
        assert_eq!(
            prepare(&store, operations.path(), id, &hash)
                .await
                .commit()
                .await
                .unwrap(),
            AttachmentRemoval::Uncertain
        );
        assert_eq!(
            prepare(&store, operations.path(), id, &hash)
                .await
                .commit()
                .await
                .unwrap(),
            AttachmentRemoval::Uncertain
        );
        assert_eq!(
            std::fs::read(vault.path().join("sessions/note/attachments/file.pdf")).unwrap(),
            b"original"
        );
    }
    #[tokio::test]
    async fn restart_recovers_a_moved_file_and_never_touches_a_recreated_source() {
        let (vault, operations, store) = fixture().await;
        let id = uuid::Uuid::new_v4();
        let hash = sha(b"original");
        store
            .save_attachment("note", "file.pdf", b"original".to_vec())
            .await
            .unwrap();
        assert!(
            prepare(&store, operations.path(), id, &hash)
                .await
                .commit_with(|| Err(StoreError::Io("Interrupted after the durable move".into())))
                .await
                .is_err()
        );
        let path = receipt_path(operations.path(), id);
        assert!(matches!(
            read_receipt(&path).unwrap().unwrap().state,
            State::Pending
        ));
        store
            .save_attachment("note", "file.pdf", b"new generation".to_vec())
            .await
            .unwrap();
        drop(store);
        let restarted = SessionStore::new(vault.path().into());
        assert_eq!(
            prepare(&restarted, operations.path(), id, &hash)
                .await
                .commit()
                .await
                .unwrap(),
            AttachmentRemoval::Removed
        );
        assert_eq!(
            std::fs::read(vault.path().join("sessions/note/attachments/file.pdf")).unwrap(),
            b"new generation"
        );
        assert!(matches!(
            read_receipt(&path).unwrap().unwrap().state,
            State::Removed
        ));
    }
    #[tokio::test]
    async fn malformed_or_future_receipts_and_changed_sessions_cannot_remove_content() {
        let (vault, operations, store) = fixture().await;
        let id = uuid::Uuid::new_v4();
        let hash = sha(b"original");
        store
            .save_attachment("note", "file.pdf", b"original".to_vec())
            .await
            .unwrap();
        let pending = prepare(&store, operations.path(), id, &hash).await;
        store.delete_session("note").await.unwrap();
        assert_eq!(pending.commit().await.unwrap(), AttachmentRemoval::Changed);
        store.restore_session("note").await.unwrap();
        let path = receipt_path(operations.path(), id);
        let request = Request {
            session: "note".into(),
            attachment: "file.pdf".into(),
            expected_sha256: hash.clone(),
        };
        persist(
            &path,
            &Receipt {
                schema_version: 999,
                request,
                target: Some("../../private".into()),
                state: State::Pending,
            },
        )
        .unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(
            prepare(&store, operations.path(), id, &hash)
                .await
                .commit()
                .await
                .unwrap(),
            AttachmentRemoval::Uncertain
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(
            vault
                .path()
                .join("sessions/note/attachments/file.pdf")
                .exists()
        );
    }
    #[tokio::test]
    async fn abandoned_commit_keeps_its_write_and_receipt_guards_until_actual_io_finishes() {
        let (vault, operations, store) = fixture().await;
        let id = uuid::Uuid::new_v4();
        let hash = sha(b"original");
        store
            .save_attachment("note", "file.pdf", b"original".to_vec())
            .await
            .unwrap();
        let pending = prepare(&store, operations.path(), id, &hash).await;
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(async move {
            pending
                .commit_with(move || {
                    let _ = entered_tx.send(());
                    release_rx.recv().unwrap();
                    Ok(())
                })
                .await
        });
        entered_rx.await.unwrap();
        task.abort();
        let other = store.clone();
        let op = operations.path().to_path_buf();
        let expected = hash.clone();
        let mut retry = tokio::spawn(async move {
            other
                .prepare_attachment_removal(&op, "alice", id, "note", "file.pdf", &expected)
                .await
                .unwrap()
                .unwrap()
                .commit()
                .await
                .unwrap()
        });
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), &mut retry)
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(20), store.lock_snapshot())
                .await
                .is_err()
        );
        release_tx.send(()).unwrap();
        assert_eq!(retry.await.unwrap(), AttachmentRemoval::Removed);
        assert!(
            !vault
                .path()
                .join("sessions/note/attachments/file.pdf")
                .exists()
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn linked_sources_and_trash_directories_are_refused() {
        use std::os::unix::fs::symlink;
        let (vault, operations, store) = fixture().await;
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("private.pdf"), b"private").unwrap();
        let directory = vault.path().join("sessions/note/attachments");
        std::fs::create_dir(&directory).unwrap();
        symlink(
            outside.path().join("private.pdf"),
            directory.join("file.pdf"),
        )
        .unwrap();
        assert!(
            store
                .prepare_attachment_removal(
                    operations.path(),
                    "alice",
                    uuid::Uuid::new_v4(),
                    "note",
                    "file.pdf",
                    &sha(b"private")
                )
                .await
                .is_err()
        );
        std::fs::remove_file(directory.join("file.pdf")).unwrap();
        store
            .save_attachment("note", "file.pdf", b"original".to_vec())
            .await
            .unwrap();
        symlink(outside.path(), vault.path().join(".trash")).unwrap();
        assert!(
            prepare(
                &store,
                operations.path(),
                uuid::Uuid::new_v4(),
                &sha(b"original")
            )
            .await
            .commit()
            .await
            .is_err()
        );
        assert_eq!(
            std::fs::read(directory.join("file.pdf")).unwrap(),
            b"original"
        );
        assert_eq!(
            std::fs::read(outside.path().join("private.pdf")).unwrap(),
            b"private"
        );
    }
}
