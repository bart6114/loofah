use std::path::{Path, PathBuf};

use hypr_loofah_protocol::DocumentTarget;
use serde::{Deserialize, Serialize};

use crate::{SessionStore, StoreError, TaskItem, WriteGuard, paths};

const MAX_INTENT: u64 = 32 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Intent {
    schema_version: u32,
    session_id: String,
    target: DocumentTarget,
    before_revision: String,
    after_revision: String,
    before_tasks_hash: String,
    tasks: Vec<TaskItem>,
}

pub(super) struct PendingDocument(PathBuf);

impl PendingDocument {
    pub(super) async fn finish(self) -> Result<(), StoreError> {
        tokio::task::spawn_blocking(move || remove(&self.0))
            .await
            .map_err(join_error)?
    }
}

impl SessionStore {
    pub async fn recover_session_document_write(
        &self,
        root: &Path,
        session_id: &str,
    ) -> Result<(), StoreError> {
        crate::validate_session_id(session_id)?;
        let guard = self.lock_writes().await;
        self.recover_document_write_locked(&guard, root, session_id)
            .await
    }

    pub async fn recover_document_writes(&self, root: &Path) -> Result<(), StoreError> {
        self.validate_document_root(root)?;
        let guard = self.lock_writes().await;
        let root = root.to_path_buf();
        let files = tokio::task::spawn_blocking(move || {
            if !regular_directory(&root)? {
                return Ok(Vec::new());
            }
            let mut files = Vec::new();
            for entry in std::fs::read_dir(root).map_err(io_error)? {
                let entry = entry.map_err(io_error)?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name.starts_with('.') {
                    continue;
                }
                if name.len() != 69
                    || !name.ends_with(".json")
                    || !name[..64].bytes().all(|b| b.is_ascii_hexdigit())
                {
                    return Err(StoreError::Io(
                        "unrecognized document recovery record".into(),
                    ));
                }
                files.push(entry.path());
                if files.len() > 256 {
                    return Err(StoreError::Io("too many pending document writes".into()));
                }
            }
            Ok(files)
        })
        .await
        .map_err(join_error)??;
        for path in files {
            self.recover_document_write_at_locked(&guard, &path).await?;
        }
        Ok(())
    }

    pub(super) async fn recover_document_write_locked(
        &self,
        guard: &WriteGuard<'_>,
        root: &Path,
        session_id: &str,
    ) -> Result<(), StoreError> {
        self.validate_document_root(root)?;
        self.recover_document_write_at_locked(guard, &intent_path(root, session_id))
            .await
    }

    async fn recover_document_write_at_locked(
        &self,
        guard: &WriteGuard<'_>,
        path: &Path,
    ) -> Result<(), StoreError> {
        let read_path = path.to_path_buf();
        let intent = tokio::task::spawn_blocking(move || read(&read_path))
            .await
            .map_err(join_error)??;
        let Some(intent) = intent else {
            return Ok(());
        };
        crate::validate_session_id(&intent.session_id)?;
        if intent.schema_version != 1
            || path.file_name()
                != intent_path(path.parent().unwrap(), &intent.session_id).file_name()
        {
            return Err(StoreError::Io(
                "invalid document recovery identity or version".into(),
            ));
        }
        let current = self
            .document_snapshot_locked(guard, &intent.session_id, &intent.target)
            .await?;
        let pending = PendingDocument(path.to_path_buf());
        let Some(current) = current else {
            return pending.finish().await;
        };
        let before_tasks =
            self.document_tasks_hash(&intent.session_id).await? == intent.before_tasks_hash;
        if current.revision == intent.after_revision {
            let tasks = self.session_tasks(&intent.session_id).await?;
            let mut planned = intent.tasks.clone();
            planned.sort_by(|a, b| {
                (&a.source_type, &a.source_id, a.source_order, &a.id).cmp(&(
                    &b.source_type,
                    &b.source_id,
                    b.source_order,
                    &b.id,
                ))
            });
            if tasks != planned {
                if !before_tasks {
                    return Err(StoreError::Conflict("tasks changed during an interrupted document write; recovery requires review".into()));
                }
                self.persist_generated_tasks(guard, &intent.session_id, &intent.tasks)
                    .await?;
            }
            pending.finish().await
        } else if current.revision == intent.before_revision && before_tasks {
            pending.finish().await
        } else {
            Err(StoreError::Conflict(
                "document changed during an interrupted write; recovery requires review".into(),
            ))
        }
    }

    pub(super) async fn begin_document_write_locked(
        &self,
        guard: &WriteGuard<'_>,
        root: &Path,
        session_id: &str,
        target: &DocumentTarget,
        after_bytes: &[u8],
        tasks: &[TaskItem],
    ) -> Result<PendingDocument, StoreError> {
        self.validate_document_root(root)?;
        let current = self
            .document_snapshot_locked(guard, session_id, target)
            .await?
            .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?;
        let intent = Intent {
            schema_version: 1,
            session_id: session_id.into(),
            target: target.clone(),
            before_revision: current.revision,
            after_revision: super::document_revision::revision(
                session_id,
                target,
                Some(after_bytes),
            )?,
            before_tasks_hash: self.document_tasks_hash(session_id).await?,
            tasks: tasks.to_vec(),
        };
        let path = intent_path(root, session_id);
        let write_path = path.clone();
        tokio::task::spawn_blocking(move || {
            let bytes =
                serde_json::to_string(&intent).map_err(|e| StoreError::Serialize(e.to_string()))?;
            if bytes.len() as u64 > MAX_INTENT {
                return Err(StoreError::Io(
                    "document recovery record exceeds its size limit".into(),
                ));
            }
            let root = write_path.parent().unwrap();
            if !regular_directory(root)? {
                let mut builder = std::fs::DirBuilder::new();
                builder.recursive(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::DirBuilderExt;
                    builder.mode(0o700);
                }
                builder.create(root).map_err(io_error)?;
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
                    .map_err(io_error)?;
            }
            if std::fs::symlink_metadata(&write_path).is_ok() {
                return Err(StoreError::Conflict(
                    "a document write still needs recovery".into(),
                ));
            }
            hypr_storage::fs::atomic_write(&write_path, &bytes).map_err(io_error)?;
            sync_parent(&write_path)
        })
        .await
        .map_err(join_error)??;
        Ok(PendingDocument(path))
    }

    async fn document_tasks_hash(&self, session_id: &str) -> Result<String, StoreError> {
        let dir = self.session_dir(session_id).await?;
        let path = self.vault_base.join(paths::session_tasks_path_in(&dir));
        tokio::task::spawn_blocking(move || {
            let raw = match std::fs::symlink_metadata(&path) {
                Ok(meta) if !meta.is_file() => {
                    return Err(StoreError::Io("tasks is not a regular file".into()));
                }
                Ok(_) => Some(std::fs::read(path).map_err(io_error)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(io_error(e)),
            };
            let mut bytes = vec![u8::from(raw.is_some())];
            if let Some(raw) = raw {
                bytes.extend_from_slice(&raw);
            }
            Ok(crate::sha256(&bytes))
        })
        .await
        .map_err(join_error)?
    }

    fn validate_document_root(&self, root: &Path) -> Result<(), StoreError> {
        if !root.is_absolute()
            || root
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir))
            || root.starts_with(self.vault_base.join("sessions"))
        {
            return Err(StoreError::Io(
                "document recovery requires an absolute runtime directory outside sessions".into(),
            ));
        }
        Ok(())
    }
}

fn intent_path(root: &Path, session_id: &str) -> PathBuf {
    root.join(format!("{}.json", crate::sha256(session_id.as_bytes())))
}

fn regular_directory(path: &Path) -> Result<bool, StoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => Err(StoreError::Io(
            "document recovery root is not a regular directory".into(),
        )),
        Ok(_) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_error(e)),
    }
}

fn read(path: &Path) -> Result<Option<Intent>, StoreError> {
    regular_directory(path.parent().unwrap())?;
    match std::fs::symlink_metadata(path) {
        Ok(meta) if !meta.is_file() || meta.len() > MAX_INTENT => {
            return Err(StoreError::Io("invalid document recovery file".into()));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io_error(e)),
    }
    use std::io::Read;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(io_error)?
        .take(MAX_INTENT + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > MAX_INTENT {
        return Err(StoreError::Io(
            "document recovery record exceeds its size limit".into(),
        ));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| StoreError::Serialize(e.to_string()))
}

fn remove(path: &Path) -> Result<(), StoreError> {
    std::fs::remove_file(path).map_err(io_error)?;
    sync_parent(path)
}

fn sync_parent(path: &Path) -> Result<(), StoreError> {
    #[cfg(unix)]
    std::fs::File::open(path.parent().unwrap())
        .and_then(|f| f.sync_all())
        .map_err(io_error)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn io_error(e: std::io::Error) -> StoreError {
    StoreError::Io(e.to_string())
}
fn join_error(e: tokio::task::JoinError) -> StoreError {
    StoreError::Io(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn fixture() -> (
        tempfile::TempDir,
        tempfile::TempDir,
        SessionStore,
        Vec<TaskItem>,
    ) {
        let vault = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().into());
        let meta = serde_json::from_value(
            json!({"id":"s1","title":"Meeting","created_at":"2026-11-05T10:00:00Z","tags":[]}),
        )
        .unwrap();
        store
            .create_note_session(&meta, "Personal note")
            .await
            .unwrap();
        store.ensure_summary("s1").await.unwrap();
        store
            .update_generated_summary("s1", "- [ ] Call Alice", Some(""))
            .await
            .unwrap();
        let mut tasks = store.session_tasks("s1").await.unwrap();
        tasks[0].status = "done".into();
        tasks[0].updated_at = "2026-11-05T11:00:00Z".into();
        (vault, runtime, store, tasks)
    }

    async fn admit(
        store: &SessionStore,
        root: &Path,
        tasks: &[TaskItem],
        write_body: bool,
        write_tasks: bool,
    ) {
        let guard = store.lock_writes().await;
        store
            .begin_document_write_locked(
                &guard,
                root,
                "s1",
                &DocumentTarget::Summary,
                b"- [x] Call Alice",
                tasks,
            )
            .await
            .unwrap();
        if write_body {
            store
                .write_file_locked(
                    &guard,
                    PathBuf::from("sessions/s1/summary.md"),
                    b"- [x] Call Alice".to_vec(),
                )
                .await
                .unwrap();
        }
        if write_tasks {
            store
                .persist_generated_tasks(&guard, "s1", tasks)
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn restart_recovers_exact_tasks_after_body_commit_and_leaves_note_and_attachments_untouched()
     {
        let (vault, runtime, store, tasks) = fixture().await;
        std::fs::write(vault.path().join("sessions/s1/foreign.md"), "Attachment").unwrap();
        admit(&store, runtime.path(), &tasks, true, false).await;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(intent_path(runtime.path(), "s1"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
        drop(store);
        let store = SessionStore::new(vault.path().into());
        store.recover_document_writes(runtime.path()).await.unwrap();
        assert_eq!(store.session_tasks("s1").await.unwrap(), tasks);
        assert_eq!(
            store.read_summary("s1").await.unwrap().as_deref(),
            Some("- [x] Call Alice")
        );
        assert_eq!(
            store.read_note("s1").await.unwrap().as_deref(),
            Some("Personal note")
        );
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/s1/foreign.md")).unwrap(),
            "Attachment"
        );
        assert!(!intent_path(runtime.path(), "s1").exists());
        store.recover_document_writes(runtime.path()).await.unwrap();
    }

    #[tokio::test]
    async fn pre_body_crash_abandons_uncommitted_work_and_completed_tasks_are_not_rewritten() {
        let (_vault, runtime, store, tasks) = fixture().await;
        let before = store.session_tasks("s1").await.unwrap();
        admit(&store, runtime.path(), &tasks, false, false).await;
        store.recover_document_writes(runtime.path()).await.unwrap();
        assert_eq!(store.session_tasks("s1").await.unwrap(), before);
        assert_eq!(
            store.read_summary("s1").await.unwrap().as_deref(),
            Some("- [ ] Call Alice")
        );
        admit(&store, runtime.path(), &tasks, true, true).await;
        let path = store.vault_base().join("sessions/s1/tasks.json");
        let raw = std::fs::read(&path).unwrap();
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        store.recover_document_writes(runtime.path()).await.unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), raw);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            modified
        );
    }

    #[tokio::test]
    async fn ambiguous_external_edits_require_review_and_deleted_sessions_are_never_restored() {
        let (vault, runtime, store, tasks) = fixture().await;
        admit(&store, runtime.path(), &tasks, true, false).await;
        let task_path = vault.path().join("sessions/s1/tasks.json");
        let mut changed = tasks.clone();
        changed[0].text = "Changed elsewhere".into();
        let raw = serde_json::to_vec(&crate::tasks::TasksFile { tasks: changed }).unwrap();
        std::fs::write(&task_path, &raw).unwrap();
        assert!(matches!(
            store.recover_document_writes(runtime.path()).await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(std::fs::read(&task_path).unwrap(), raw);
        assert!(intent_path(runtime.path(), "s1").exists());
        std::fs::remove_file(vault.path().join("sessions/s1/_meta.json")).unwrap();
        store.recover_document_writes(runtime.path()).await.unwrap();
        assert!(!vault.path().join("sessions/s1/_meta.json").exists());
        assert!(!intent_path(runtime.path(), "s1").exists());
    }

    #[tokio::test]
    async fn changed_documents_and_future_corrupt_or_symlinked_records_fail_closed() {
        let (vault, runtime, store, tasks) = fixture().await;
        admit(&store, runtime.path(), &tasks, true, false).await;
        std::fs::write(
            vault.path().join("sessions/s1/summary.md"),
            "External replacement",
        )
        .unwrap();
        assert!(matches!(
            store.recover_document_writes(runtime.path()).await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/s1/summary.md")).unwrap(),
            "External replacement"
        );
        let path = intent_path(runtime.path(), "s1");
        let original = std::fs::read(&path).unwrap();
        let mut future: serde_json::Value = serde_json::from_slice(&original).unwrap();
        future["schema_version"] = 2.into();
        std::fs::write(&path, serde_json::to_vec(&future).unwrap()).unwrap();
        assert!(store.recover_document_writes(runtime.path()).await.is_err());
        std::fs::write(&path, "damaged").unwrap();
        assert!(store.recover_document_writes(runtime.path()).await.is_err());
        #[cfg(unix)]
        {
            std::fs::remove_file(&path).unwrap();
            let foreign = vault.path().join("private.json");
            std::fs::write(&foreign, original).unwrap();
            std::os::unix::fs::symlink(&foreign, &path).unwrap();
            assert!(store.recover_document_writes(runtime.path()).await.is_err());
            assert!(foreign.exists());
        }
    }

    #[tokio::test]
    async fn unavailable_or_misplaced_runtime_storage_prevents_the_body_commit() {
        let (vault, runtime, store, _) = fixture().await;
        let before = store
            .read_document_snapshot("s1", &DocumentTarget::Summary)
            .await
            .unwrap()
            .unwrap();
        let blocked = runtime.path().join("blocked");
        std::fs::write(&blocked, "Occupied").unwrap();
        for root in [
            blocked,
            PathBuf::from("relative"),
            vault.path().join("sessions/s1/runtime"),
        ] {
            assert!(
                store
                    .save_document_if_revision(
                        &root,
                        "s1",
                        &DocumentTarget::Summary,
                        "Do not save",
                        &before.revision
                    )
                    .await
                    .is_err()
            );
            assert_eq!(
                store
                    .read_document_snapshot("s1", &DocumentTarget::Summary)
                    .await
                    .unwrap()
                    .unwrap(),
                before
            );
        }
        let fresh = runtime.path().join("private");
        store
            .save_document_if_revision(
                &fresh,
                "s1",
                &DocumentTarget::Summary,
                "Safe",
                &before.revision,
            )
            .await
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(fresh).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
}
