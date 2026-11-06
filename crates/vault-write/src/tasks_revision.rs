use std::path::Path;

use hypr_loofah_protocol::{DocumentTarget, TaskSource, TasksSaveOutcome, TasksSnapshot};

use crate::tasks::TasksFile;
use crate::{SessionStore, StoreError, TaskInput, WriteGuard, paths};

const TASKS_LIMIT: usize = 8 * 1024 * 1024;

impl SessionStore {
    pub async fn read_tasks_snapshot(
        &self,
        operation_root: Option<&Path>,
        session_id: &str,
    ) -> Result<Option<TasksSnapshot>, StoreError> {
        self.recovered_read(
            operation_root,
            session_id,
            |store, snapshot, id| async move {
                store.tasks_snapshot_locked(&snapshot._guard, &id).await
            },
        )
        .await
    }

    pub async fn replace_tasks_if_revision(
        &self,
        operation_root: &Path,
        session_id: &str,
        source: &TaskSource,
        inputs: Vec<TaskInput>,
        expected_revision: &str,
    ) -> Result<Option<TasksSaveOutcome>, StoreError> {
        crate::validate_session_id(session_id)?;
        let guard = self.lock_writes().await;
        self.ensure_ready()?;
        self.recover_document_write_locked(&guard, operation_root, session_id)
            .await?;
        let Some(current) = self.tasks_snapshot_locked(&guard, session_id).await? else {
            return Ok(None);
        };
        if current.revision != expected_revision {
            return Ok(Some(TasksSaveOutcome::Conflict(current)));
        }
        let (source_type, source_id) = match source {
            TaskSource::RawNote => ("session_raw_note", session_id),
            TaskSource::Summary => {
                if self
                    .document_snapshot_locked(&guard, session_id, &DocumentTarget::Summary)
                    .await?
                    .is_none_or(|snapshot| snapshot.document.is_none())
                {
                    return Ok(None);
                }
                ("session_summary", session_id)
            }
            TaskSource::TemplateOutput { document_id } => {
                if self
                    .document_snapshot_locked(
                        &guard,
                        session_id,
                        &DocumentTarget::TemplateOutput {
                            document_id: document_id.clone(),
                        },
                    )
                    .await?
                    .is_none_or(|snapshot| snapshot.document.is_none())
                {
                    return Ok(None);
                }
                ("enhanced_note", document_id.as_str())
            }
        };
        let existing = current.tasks;
        let next = crate::tasks::replaced_source_tasks(&existing, source_type, source_id, inputs);
        if serde_json::to_vec_pretty(&TasksFile {
            tasks: next.clone(),
        })
        .map_err(|cause| StoreError::Serialize(cause.to_string()))?
        .len()
            > TASKS_LIMIT
        {
            return Err(StoreError::ResourceLimit(
                "Session tasks exceed their size limit".into(),
            ));
        }
        self.persist_generated_tasks(&guard, session_id, &next)
            .await?;
        let saved = self
            .tasks_snapshot_locked(&guard, session_id)
            .await?
            .ok_or_else(|| StoreError::Io("Session disappeared after replacing tasks".into()))?;
        Ok(Some(TasksSaveOutcome::Saved(saved)))
    }

    async fn tasks_snapshot_locked(
        &self,
        guard: &WriteGuard<'_>,
        session_id: &str,
    ) -> Result<Option<TasksSnapshot>, StoreError> {
        if self
            .metadata_snapshot_locked(guard, session_id)
            .await?
            .is_none()
        {
            return Ok(None);
        }
        let vault = self.vault_base.clone();
        let dir = paths::validated_session_dir(session_id)?;
        let id = session_id.to_owned();
        let snapshot = tokio::task::spawn_blocking(move || {
            let path = vault.join(paths::session_tasks_path_in(&dir));
            let raw = match std::fs::symlink_metadata(&path) {
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => None,
                Err(cause) => return Err(StoreError::Io(cause.to_string())),
                Ok(meta) if meta.file_type().is_symlink() || !meta.is_file() => {
                    return Err(StoreError::Io(
                        "Session tasks are not a regular file".into(),
                    ));
                }
                Ok(meta) if meta.len() > TASKS_LIMIT as u64 => {
                    return Err(StoreError::ResourceLimit(
                        "Session tasks exceed their size limit".into(),
                    ));
                }
                Ok(_) => {
                    use std::io::Read;
                    let mut bytes = Vec::new();
                    std::fs::File::open(&path)
                        .map_err(|cause| StoreError::Io(cause.to_string()))?
                        .take(TASKS_LIMIT as u64 + 1)
                        .read_to_end(&mut bytes)
                        .map_err(|cause| StoreError::Io(cause.to_string()))?;
                    if bytes.len() > TASKS_LIMIT {
                        return Err(StoreError::ResourceLimit(
                            "Session tasks exceed their size limit".into(),
                        ));
                    }
                    Some(bytes)
                }
            };
            let tasks = raw
                .as_ref()
                .map(|bytes| serde_json::from_slice::<TasksFile>(bytes).map(|file| file.tasks))
                .transpose()
                .map_err(|cause| StoreError::Serialize(cause.to_string()))?
                .unwrap_or_default();
            let mut bytes = b"loofah/session-tasks/v1\0".to_vec();
            bytes.extend_from_slice(&(id.len() as u64).to_be_bytes());
            bytes.extend_from_slice(id.as_bytes());
            bytes.push(u8::from(raw.is_some()));
            if let Some(raw) = raw {
                bytes.extend_from_slice(&raw);
            }
            Ok(Some(TasksSnapshot {
                tasks,
                revision: crate::sha256(&bytes),
            }))
        })
        .await
        .map_err(|cause| StoreError::Io(cause.to_string()))??;
        let Some(mut snapshot) = snapshot else {
            return Ok(None);
        };
        self.normalize_summary_tasks(session_id, &mut snapshot.tasks, None)
            .await?;
        crate::tasks::sort_tasks(&mut snapshot.tasks);
        Ok(Some(snapshot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    async fn fixture() -> (tempfile::TempDir, SessionStore, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().join("vault"));
        let meta = serde_json::from_value(json!({"id":"note","title":"Title","created_at":"2026-11-05T10:00:00Z","tags":[],"future":{"preserve":true}})).unwrap();
        store.create_note_session(&meta, "User note").await.unwrap();
        let operations = root.path().join("runtime/document-saves");
        (root, store, operations)
    }
    fn input(id: &str, text: &str) -> TaskInput {
        TaskInput {
            id: id.into(),
            source_order: 0,
            status: "todo".into(),
            text: text.into(),
            body: json!([]),
            due_at: "".into(),
        }
    }
    async fn snapshot(store: &SessionStore, root: &Path) -> TasksSnapshot {
        store
            .read_tasks_snapshot(Some(root), "note")
            .await
            .unwrap()
            .unwrap()
    }
    fn saved(outcome: Option<TasksSaveOutcome>) -> TasksSnapshot {
        let Some(TasksSaveOutcome::Saved(snapshot)) = outcome else {
            panic!("expected saved tasks")
        };
        snapshot
    }

    #[tokio::test]
    async fn checked_replacements_share_legacy_field_rules_and_preserve_other_sources_and_files() {
        let (_root, store, operations) = fixture().await;
        store.ensure_summary("note").await.unwrap();
        store
            .replace_tasks("session_raw_note", "note", vec![input("raw", "Raw task")])
            .await
            .unwrap();
        store
            .replace_tasks(
                "session_summary",
                "note",
                vec![input("summary", "Summary task")],
            )
            .await
            .unwrap();
        let dir = store.vault_base().join("sessions/note");
        std::fs::write(dir.join("foreign.txt"), "User attachment").unwrap();
        std::fs::write(dir.join(".private"), "Private attachment").unwrap();
        let paths = [
            "_meta.json",
            "notes.md",
            "summary.md",
            "foreign.txt",
            ".private",
        ];
        let before_files: Vec<_> = paths
            .iter()
            .map(|name| std::fs::read(dir.join(name)).unwrap())
            .collect();
        let mut file: TasksFile =
            serde_json::from_slice(&std::fs::read(dir.join("tasks.json")).unwrap()).unwrap();
        let task = file.tasks.iter_mut().find(|task| task.id == "raw").unwrap();
        task.assignee = "person".into();
        std::fs::write(dir.join("tasks.json"), serde_json::to_vec(&file).unwrap()).unwrap();
        let before = snapshot(&store, &operations).await;
        let prior = before.tasks.iter().find(|task| task.id == "raw").unwrap();
        let other = before
            .tasks
            .iter()
            .find(|task| task.id == "summary")
            .unwrap()
            .clone();
        let mut patch = input("raw", "Changed task");
        patch.status = "done".into();
        patch.due_at = "2026-11-10".into();
        patch.body = json!([{"type":"paragraph","content":[{"type":"text","text":"Details"}]}]);
        let after = saved(
            store
                .replace_tasks_if_revision(
                    &operations,
                    "note",
                    &TaskSource::RawNote,
                    vec![patch.clone()],
                    &before.revision,
                )
                .await
                .unwrap(),
        );
        let task = after.tasks.iter().find(|task| task.id == "raw").unwrap();
        assert_eq!(
            (&task.assignee, &task.created_at),
            (&prior.assignee, &prior.created_at)
        );
        assert_eq!(
            (&task.text, &task.status, &task.body, &task.due_at),
            (&patch.text, &patch.status, &patch.body, &patch.due_at)
        );
        assert_eq!(
            after.tasks.iter().find(|task| task.id == "summary"),
            Some(&other)
        );
        for (path, bytes) in paths.iter().zip(before_files) {
            assert_eq!(std::fs::read(dir.join(path)).unwrap(), bytes);
        }
        let path = dir.join("tasks.json");
        let before_noop = std::fs::read(&path).unwrap();
        let same = saved(
            store
                .replace_tasks_if_revision(
                    &operations,
                    "note",
                    &TaskSource::RawNote,
                    vec![patch],
                    &after.revision,
                )
                .await
                .unwrap(),
        );
        assert_eq!(same.revision, after.revision);
        assert_eq!(std::fs::read(path).unwrap(), before_noop);
    }

    #[tokio::test]
    async fn parallel_replacements_have_one_winner_and_do_not_drop_another_sources_tasks() {
        let (_root, store, operations) = fixture().await;
        store.ensure_summary("note").await.unwrap();
        store
            .replace_tasks("session_raw_note", "note", vec![input("raw", "Raw")])
            .await
            .unwrap();
        store
            .replace_tasks("session_summary", "note", vec![input("summary", "Summary")])
            .await
            .unwrap();
        let before = snapshot(&store, &operations).await;
        let (one, two) = tokio::join!(
            store.replace_tasks_if_revision(
                &operations,
                "note",
                &TaskSource::RawNote,
                vec![input("raw", "Alice")],
                &before.revision
            ),
            store.replace_tasks_if_revision(
                &operations,
                "note",
                &TaskSource::Summary,
                vec![input("summary", "Bob")],
                &before.revision
            )
        );
        let outcomes = [one.unwrap().unwrap(), two.unwrap().unwrap()];
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, TasksSaveOutcome::Saved(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, TasksSaveOutcome::Conflict(_)))
                .count(),
            1
        );
        assert_eq!(snapshot(&store, &operations).await.tasks.len(), 2);
    }

    #[tokio::test]
    async fn legacy_summary_sources_are_projected_without_read_writes_and_replaced_with_existing_rules()
     {
        let (_root, store, operations) = fixture().await;
        store
            .write_enhanced_doc(&crate::EnhancedDoc {
                id: "legacy-summary".into(),
                session_id: "note".into(),
                kind: "summary".into(),
                title: "Old summary".into(),
                template_id: "".into(),
                sort_order: 0,
                markdown: "- [ ] Legacy task".into(),
            })
            .await
            .unwrap();
        store
            .replace_tasks(
                "enhanced_note",
                "legacy-summary",
                vec![input("task", "Legacy task")],
            )
            .await
            .unwrap();
        let directory = store.vault_base().join("sessions/note");
        let task_bytes = std::fs::read(directory.join("tasks.json")).unwrap();
        let legacy_bytes = std::fs::read(directory.join("enhanced/legacy-summary.md")).unwrap();
        let before = snapshot(&store, &operations).await;
        assert_eq!(before.tasks[0].source_type, "session_summary");
        assert_eq!(before.tasks[0].source_id, "note");
        assert_eq!(
            std::fs::read(directory.join("tasks.json")).unwrap(),
            task_bytes
        );
        assert!(!directory.join("summary.md").exists());
        let mut changed = input("task", "Legacy task");
        changed.status = "done".into();
        let after = saved(
            store
                .replace_tasks_if_revision(
                    &operations,
                    "note",
                    &TaskSource::Summary,
                    vec![changed],
                    &before.revision,
                )
                .await
                .unwrap(),
        );
        assert_eq!(after.tasks[0].id, "task");
        assert_eq!(after.tasks[0].status, "done");
        assert_eq!(after.tasks[0].created_at, before.tasks[0].created_at);
        assert_eq!(
            std::fs::read(directory.join("enhanced/legacy-summary.md")).unwrap(),
            legacy_bytes
        );
        assert!(!directory.join("summary.md").exists());
    }

    #[tokio::test]
    async fn pending_document_tasks_recover_before_a_checked_replacement_can_overwrite_them() {
        let (_root, store, operations) = fixture().await;
        store.ensure_summary("note").await.unwrap();
        store
            .replace_tasks(
                "session_summary",
                "note",
                vec![input("task", "Recovered task")],
            )
            .await
            .unwrap();
        let before = snapshot(&store, &operations).await;
        let mut prepared = before.tasks.clone();
        prepared[0].status = "done".into();
        {
            let guard = store.lock_writes().await;
            let _pending = store
                .begin_document_write_locked(
                    &guard,
                    &operations,
                    "note",
                    &DocumentTarget::Summary,
                    b"- [x] Recovered task",
                    &prepared,
                )
                .await
                .unwrap();
            std::fs::write(
                store.vault_base().join("sessions/note/summary.md"),
                "- [x] Recovered task",
            )
            .unwrap();
        }
        let result = store
            .replace_tasks_if_revision(
                &operations,
                "note",
                &TaskSource::Summary,
                vec![input("task", "Do not lose the recovered completion")],
                &before.revision,
            )
            .await
            .unwrap();
        let Some(TasksSaveOutcome::Conflict(current)) = result else {
            panic!("expected conflict with the recovered tasks")
        };
        assert_eq!(current.tasks, prepared);
        assert_eq!(std::fs::read_dir(&operations).unwrap().count(), 0);
        assert_eq!(snapshot(&store, &operations).await.tasks, prepared);
    }

    #[tokio::test]
    async fn raw_json_changes_and_absence_have_distinct_revisions_without_side_effects() {
        let (_root, store, operations) = fixture().await;
        let absent = snapshot(&store, &operations).await;
        let path = store.vault_base().join("sessions/note/tasks.json");
        assert!(!path.exists());
        std::fs::write(&path, "{\"tasks\":[]}\n").unwrap();
        let empty = snapshot(&store, &operations).await;
        assert_eq!(absent.tasks, empty.tasks);
        assert_ne!(absent.revision, empty.revision);
        std::fs::write(&path, "{ \"tasks\": [] }\n").unwrap();
        let changed = snapshot(&store, &operations).await;
        assert_ne!(empty.revision, changed.revision);
        let result = store
            .replace_tasks_if_revision(
                &operations,
                "note",
                &TaskSource::RawNote,
                vec![input("new", "Do not write")],
                &empty.revision,
            )
            .await
            .unwrap();
        assert_eq!(result, Some(TasksSaveOutcome::Conflict(changed)));
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "{ \"tasks\": [] }\n"
        );
    }

    #[tokio::test]
    async fn deleted_sessions_and_missing_or_other_session_outputs_are_not_recreated() {
        let (_root, store, operations) = fixture().await;
        let before = snapshot(&store, &operations).await;
        assert!(
            store
                .replace_tasks_if_revision(
                    &operations,
                    "note",
                    &TaskSource::Summary,
                    vec![input("new", "No summary")],
                    &before.revision
                )
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .replace_tasks_if_revision(
                    &operations,
                    "note",
                    &TaskSource::TemplateOutput {
                        document_id: "missing".into()
                    },
                    vec![input("new", "No output")],
                    &before.revision
                )
                .await
                .unwrap()
                .is_none()
        );
        assert!(!store.vault_base().join("sessions/note/tasks.json").exists());
        std::fs::remove_dir_all(store.vault_base().join("sessions/note")).unwrap();
        assert!(
            store
                .read_tasks_snapshot(Some(&operations), "note")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .replace_tasks_if_revision(
                    &operations,
                    "note",
                    &TaskSource::RawNote,
                    vec![input("new", "Deleted")],
                    &before.revision
                )
                .await
                .unwrap()
                .is_none()
        );
        assert!(!store.vault_base().join("sessions/note").exists());
    }

    #[tokio::test]
    async fn template_source_proof_uses_the_requested_session_even_with_duplicate_document_ids() {
        let (_root, store, operations) = fixture().await;
        for id in ["note", "other"] {
            if id == "other" {
                let meta = serde_json::from_value(
                    json!({"id":id,"title":"Other","created_at":"2026-11-05T10:00:00Z","tags":[]}),
                )
                .unwrap();
                store
                    .create_note_session(&meta, "Other note")
                    .await
                    .unwrap();
            }
            store
                .write_enhanced_doc(&crate::EnhancedDoc {
                    id: "same-output".into(),
                    session_id: id.into(),
                    kind: "template_output".into(),
                    title: "Template".into(),
                    template_id: "template".into(),
                    sort_order: 0,
                    markdown: "Body".into(),
                })
                .await
                .unwrap();
        }
        let before = snapshot(&store, &operations).await;
        let source = TaskSource::TemplateOutput {
            document_id: "same-output".into(),
        };
        let after = saved(
            store
                .replace_tasks_if_revision(
                    &operations,
                    "note",
                    &source,
                    vec![input("task", "Only this session")],
                    &before.revision,
                )
                .await
                .unwrap(),
        );
        assert_eq!(after.tasks[0].source_type, "enhanced_note");
        assert_eq!(after.tasks[0].source_id, "same-output");
        assert!(
            !store
                .vault_base()
                .join("sessions/other/tasks.json")
                .exists()
        );
    }

    #[tokio::test]
    async fn malformed_and_oversized_task_files_or_output_are_kept() {
        let (_root, store, operations) = fixture().await;
        let before = snapshot(&store, &operations).await;
        let mut large = input("task", "Large");
        large.body = json!("x".repeat(TASKS_LIMIT));
        assert!(
            store
                .replace_tasks_if_revision(
                    &operations,
                    "note",
                    &TaskSource::RawNote,
                    vec![large],
                    &before.revision
                )
                .await
                .is_err()
        );
        let path = store.vault_base().join("sessions/note/tasks.json");
        assert!(!path.exists());
        for bytes in [b"invalid JSON".to_vec(), vec![b' '; TASKS_LIMIT + 1]] {
            std::fs::write(&path, &bytes).unwrap();
            assert!(
                store
                    .read_tasks_snapshot(Some(&operations), "note")
                    .await
                    .is_err()
            );
            assert!(
                store
                    .replace_tasks_if_revision(
                        &operations,
                        "note",
                        &TaskSource::RawNote,
                        vec![input("task", "Keep")],
                        &before.revision
                    )
                    .await
                    .is_err()
            );
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn task_symlinks_cannot_read_or_replace_foreign_files() {
        let (root, store, operations) = fixture().await;
        let before = snapshot(&store, &operations).await;
        let foreign = root.path().join("foreign.json");
        std::fs::write(&foreign, r#"{"tasks":[]}"#).unwrap();
        let path = store.vault_base().join("sessions/note/tasks.json");
        std::os::unix::fs::symlink(&foreign, &path).unwrap();
        assert!(
            store
                .read_tasks_snapshot(Some(&operations), "note")
                .await
                .is_err()
        );
        assert!(
            store
                .replace_tasks_if_revision(
                    &operations,
                    "note",
                    &TaskSource::RawNote,
                    vec![input("task", "Keep")],
                    &before.revision
                )
                .await
                .is_err()
        );
        assert!(
            std::fs::symlink_metadata(path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(std::fs::read_to_string(foreign).unwrap(), r#"{"tasks":[]}"#);
    }
}
