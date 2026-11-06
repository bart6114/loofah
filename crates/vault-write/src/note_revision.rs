use std::path::Path;

use crate::{NoteSaveOutcome, NoteSnapshot, SessionStore, StoreError};

impl SessionStore {
    pub async fn reconcile_note_tasks(
        &self,
        operation_root: &Path,
        session_id: &str,
    ) -> Result<Option<hypr_loofah_protocol::SessionDocument>, StoreError> {
        let root = operation_root.to_path_buf();
        self.recovered_read(
            Some(operation_root),
            session_id,
            move |store, snapshot, id| async move {
                store.ensure_ready()?;
                let Some(current) = store.session_document_locked(&snapshot._guard, &id).await?
                else {
                    return Ok(None);
                };
                let markdown = current.note.markdown.clone().unwrap_or_default();
                let parse = markdown.clone();
                let content =
                    tokio::task::spawn_blocking(move || hypr_tiptap::md_to_tiptap_json(&parse))
                        .await
                        .map_err(|cause| StoreError::Io(cause.to_string()))?
                        .map_err(StoreError::Serialize)?;
                let tasks = store
                    .prepare_edited_tasks(&id, "session_raw_note", &id, &markdown, &content)
                    .await?;
                let mut ordered = tasks.clone();
                crate::tasks::sort_tasks(&mut ordered);
                if store.session_tasks(&id).await? == ordered {
                    return Ok(Some(current));
                }
                let pending = store
                    .begin_note_tasks_locked(&snapshot._guard, &root, &id, &current.note, &tasks)
                    .await?;
                store
                    .persist_generated_tasks(&snapshot._guard, &id, &tasks)
                    .await?;
                pending.finish().await?;
                Ok(Some(current))
            },
        )
        .await
    }

    pub async fn read_recovered_note_snapshot(
        &self,
        operation_root: Option<&Path>,
        session_id: &str,
    ) -> Result<Option<NoteSnapshot>, StoreError> {
        self.recovered_read(
            operation_root,
            session_id,
            |store, snapshot, id| async move {
                store.read_note_snapshot_locked(&snapshot._guard, &id).await
            },
        )
        .await
    }

    pub async fn read_recovered_session_document(
        &self,
        operation_root: Option<&Path>,
        session_id: &str,
    ) -> Result<Option<hypr_loofah_protocol::SessionDocument>, StoreError> {
        self.recovered_read(
            operation_root,
            session_id,
            |store, snapshot, id| async move {
                store.session_document_locked(&snapshot._guard, &id).await
            },
        )
        .await
    }

    pub async fn save_note_with_recovery(
        &self,
        operation_root: &Path,
        session_id: &str,
        markdown: &str,
        title: Option<&str>,
        expected_revision: &str,
    ) -> Result<NoteSaveOutcome, StoreError> {
        crate::validate_session_id(session_id)?;
        let markdown = markdown.to_owned();
        let parse = markdown.clone();
        let content = tokio::task::spawn_blocking(move || hypr_tiptap::md_to_tiptap_json(&parse))
            .await
            .map_err(|cause| StoreError::Io(cause.to_string()))?
            .map_err(StoreError::Serialize)?;
        let store = self.clone();
        let root = operation_root.to_path_buf();
        let id = session_id.to_owned();
        let title = title.map(str::to_owned);
        let expected_revision = expected_revision.to_owned();
        // A caller may disappear after the durable intent; complete its admitted save.
        tokio::spawn(async move {
            let guard = store.lock_writes().await;
            store.ensure_ready()?;
            store
                .recover_document_write_locked(&guard, &root, &id)
                .await?;
            let current = store
                .read_note_snapshot_locked(&guard, &id)
                .await?
                .ok_or_else(|| StoreError::Conflict("session was deleted".into()))?;
            if current.revision != expected_revision {
                return Ok(NoteSaveOutcome::Conflict(current));
            }
            let tasks = store
                .prepare_edited_tasks(
                    &id,
                    "session_raw_note",
                    &id,
                    current.markdown.as_deref().unwrap_or_default(),
                    &content,
                )
                .await?;
            let pending = store
                .begin_note_write_locked(
                    &guard,
                    &root,
                    &id,
                    &current,
                    (&markdown, title.as_deref().unwrap_or(&current.title)),
                    &tasks,
                )
                .await?;
            store
                .save_note_locked(&guard, &id, &markdown, title.as_deref())
                .await?;
            store.persist_generated_tasks(&guard, &id, &tasks).await?;
            pending.finish().await?;
            let saved = store
                .read_note_snapshot_locked(&guard, &id)
                .await?
                .ok_or_else(|| StoreError::Io("session disappeared after save".into()))?;
            Ok(NoteSaveOutcome::Saved(saved))
        })
        .await
        .map_err(|cause| StoreError::Io(format!("note save worker failed: {cause}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{TaskItem, tasks::TasksFile};
    use serde_json::json;

    async fn fixture(markdown: &str) -> (tempfile::TempDir, SessionStore, std::path::PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let store = SessionStore::new(root.path().join("vault"));
        let meta = serde_json::from_value(json!({"id":"note","title":"Title","created_at":"2026-11-05T10:00:00Z","tags":["project"],"future":{"preserve":true}})).unwrap();
        store.create_note_session(&meta, markdown).await.unwrap();
        let operations = root.path().join("runtime/document-saves");
        (root, store, operations)
    }
    async fn snapshot(store: &SessionStore, operations: &Path) -> NoteSnapshot {
        store
            .read_recovered_note_snapshot(Some(operations), "note")
            .await
            .unwrap()
            .unwrap()
    }
    async fn tasks(store: &SessionStore) -> Vec<TaskItem> {
        store.session_tasks("note").await.unwrap()
    }
    fn saved(outcome: NoteSaveOutcome) -> NoteSnapshot {
        let NoteSaveOutcome::Saved(value) = outcome else {
            panic!("expected saved note")
        };
        value
    }

    #[tokio::test]
    async fn checked_notes_reconcile_tasks_in_rust_and_preserve_other_sources_and_metadata() {
        let (_root, store, operations) = fixture("- [ ] Call Alice").await;
        store
            .reconcile_note_tasks(&operations, "note")
            .await
            .unwrap();
        store.ensure_summary("note").await.unwrap();
        store
            .update_generated_summary("note", "- [ ] Summary task", Some(""))
            .await
            .unwrap();
        let directory = store.vault_base().join("sessions/note");
        let mut file: TasksFile =
            serde_json::from_slice(&std::fs::read(directory.join("tasks.json")).unwrap()).unwrap();
        let raw = file
            .tasks
            .iter_mut()
            .find(|task| task.source_type == "session_raw_note")
            .unwrap();
        raw.status = "done".into();
        raw.due_at = "2026-12-01".into();
        raw.assignee = "person".into();
        let original = raw.clone();
        let other = file
            .tasks
            .iter()
            .find(|task| task.source_type == "session_summary")
            .unwrap()
            .clone();
        std::fs::write(
            directory.join("tasks.json"),
            serde_json::to_vec(&file).unwrap(),
        )
        .unwrap();
        for name in ["foreign.md", ".hidden"] {
            std::fs::write(directory.join(name), "Attachment canary").unwrap();
        }
        let before = snapshot(&store, &operations).await;
        let body = "New paragraph\n\n- [ ] Call Alice\n- [ ] New task";
        let after = saved(
            store
                .save_note_with_recovery(
                    &operations,
                    "note",
                    body,
                    Some("Changed"),
                    &before.revision,
                )
                .await
                .unwrap(),
        );
        assert_eq!(after.markdown.as_deref(), Some(body));
        assert_eq!(after.title, "Changed");
        let current = tasks(&store).await;
        let old = current.iter().find(|task| task.id == original.id).unwrap();
        assert_eq!(old.status, "done");
        assert_eq!(old.due_at, original.due_at);
        assert_eq!(old.assignee, original.assignee);
        assert_eq!(old.created_at, original.created_at);
        assert_eq!(
            current.iter().find(|task| task.id == other.id),
            Some(&other)
        );
        let next = current
            .iter()
            .find(|task| task.text == "New task")
            .unwrap()
            .clone();
        assert_eq!(next.status, "todo");
        let checked_body = body.replace("[ ] New task", "[x] New task");
        let checked = saved(
            store
                .save_note_with_recovery(&operations, "note", &checked_body, None, &after.revision)
                .await
                .unwrap(),
        );
        let current = tasks(&store).await;
        assert_eq!(
            current
                .iter()
                .find(|task| task.id == next.id)
                .unwrap()
                .status,
            "done"
        );
        let no_op = saved(
            store
                .save_note_with_recovery(
                    &operations,
                    "note",
                    &checked_body,
                    None,
                    &checked.revision,
                )
                .await
                .unwrap(),
        );
        assert_eq!(checked, no_op);
        assert_eq!(tasks(&store).await, current);
        store
            .save_note_with_recovery(&operations, "note", "No tasks", None, &no_op.revision)
            .await
            .unwrap();
        assert_eq!(tasks(&store).await, vec![other]);
        let meta = store.read_meta("note").await.unwrap().unwrap();
        assert_eq!(meta.tags, vec!["project"]);
        assert_eq!(meta.extra["future"], json!({"preserve":true}));
        for name in ["foreign.md", ".hidden"] {
            assert_eq!(
                std::fs::read_to_string(directory.join(name)).unwrap(),
                "Attachment canary"
            );
        }
        assert_eq!(
            store.read_summary("note").await.unwrap().as_deref(),
            Some("- [ ] Summary task")
        );
        assert_eq!(std::fs::read_dir(&operations).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn concurrent_note_saves_have_one_winner_with_matching_tasks() {
        let (_root, store, operations) = fixture("Seed").await;
        let before = snapshot(&store, &operations).await;
        let (alice, bob) = tokio::join!(
            store.save_note_with_recovery(
                &operations,
                "note",
                "- [ ] Alice",
                Some("Alice"),
                &before.revision
            ),
            store.save_note_with_recovery(
                &operations,
                "note",
                "- [ ] Bob",
                Some("Bob"),
                &before.revision
            ),
        );
        let results = [alice.unwrap(), bob.unwrap()];
        assert_eq!(
            results
                .iter()
                .filter(|value| matches!(value, NoteSaveOutcome::Saved(_)))
                .count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|value| matches!(value, NoteSaveOutcome::Conflict(_)))
                .count(),
            1
        );
        let current = snapshot(&store, &operations).await;
        let tasks = tasks(&store).await;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, current.title);
        assert_eq!(current.markdown, Some(format!("- [ ] {}", tasks[0].text)));
        assert_eq!(std::fs::read_dir(&operations).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn initial_task_reconciliation_preserves_exact_note_bytes_metadata_and_stable_ids() {
        let (_root, store, operations) =
            fixture("---\ncustom: preserve\n---\n\n- [ ] Call Alice").await;
        let directory = store.vault_base().join("sessions/note");
        let before_note = std::fs::read(directory.join("notes.md")).unwrap();
        let before_meta = std::fs::read(directory.join("_meta.json")).unwrap();
        let before = snapshot(&store, &operations).await;
        let first = store
            .reconcile_note_tasks(&operations, "note")
            .await
            .unwrap()
            .unwrap();
        let first_tasks = tasks(&store).await;
        assert_eq!(first.note, before);
        assert_eq!(first_tasks.len(), 1);
        assert_eq!(first_tasks[0].text, "Call Alice");
        let path = directory.join("tasks.json");
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        store
            .reconcile_note_tasks(&operations, "note")
            .await
            .unwrap();
        assert_eq!(tasks(&store).await, first_tasks);
        assert_eq!(
            std::fs::metadata(path).unwrap().modified().unwrap(),
            modified
        );
        assert_eq!(
            std::fs::read(directory.join("notes.md")).unwrap(),
            before_note
        );
        assert_eq!(
            std::fs::read(directory.join("_meta.json")).unwrap(),
            before_meta
        );
    }

    #[tokio::test]
    async fn missing_note_legacy_fallback_and_deleted_sessions_keep_existing_ownership_rules() {
        let (_root, store, operations) = fixture("- [ ] Legacy task").await;
        let directory = store.vault_base().join("sessions/note");
        std::fs::rename(directory.join("notes.md"), directory.join("_memo.md")).unwrap();
        let before = snapshot(&store, &operations).await;
        saved(
            store
                .save_note_with_recovery(
                    &operations,
                    "note",
                    "- [x] Legacy task",
                    None,
                    &before.revision,
                )
                .await
                .unwrap(),
        );
        assert!(!directory.join("_memo.md").exists());
        assert_eq!(tasks(&store).await[0].status, "done");
        assert!(store.vault_base().join(".trash").is_dir());
        std::fs::remove_file(directory.join("notes.md")).unwrap();
        let absent = snapshot(&store, &operations).await;
        assert!(absent.markdown.is_none());
        saved(
            store
                .save_note_with_recovery(
                    &operations,
                    "note",
                    "Restored explicit edit",
                    None,
                    &absent.revision,
                )
                .await
                .unwrap(),
        );
        let before_delete = snapshot(&store, &operations).await;
        store.delete_session("note").await.unwrap();
        assert!(
            store
                .save_note_with_recovery(
                    &operations,
                    "note",
                    "Do not recreate",
                    None,
                    &before_delete.revision
                )
                .await
                .is_err()
        );
        assert!(!directory.exists());
        assert!(
            store
                .reconcile_note_tasks(&operations, "missing")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn raw_frontmatter_changes_and_unavailable_operation_storage_reject_before_writing() {
        let (_root, store, operations) = fixture("Original").await;
        let before = snapshot(&store, &operations).await;
        let directory = store.vault_base().join("sessions/note");
        std::fs::write(
            directory.join("notes.md"),
            "---\nid: note:note\nposition: 0\nsession_id: note\n---\n\nOriginal",
        )
        .unwrap();
        let current = snapshot(&store, &operations).await;
        assert_eq!(current.markdown, before.markdown);
        assert_ne!(current.revision, before.revision);
        let bytes = std::fs::read(directory.join("notes.md")).unwrap();
        assert_eq!(
            store
                .save_note_with_recovery(&operations, "note", "Stale", None, &before.revision)
                .await
                .unwrap(),
            NoteSaveOutcome::Conflict(current.clone())
        );
        assert_eq!(std::fs::read(directory.join("notes.md")).unwrap(), bytes);
        assert!(
            store
                .save_note_with_recovery(
                    &directory.join("runtime"),
                    "note",
                    "Do not write",
                    Some("New title"),
                    &current.revision
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(directory.join("notes.md")).unwrap(), bytes);
        assert_eq!(
            store.read_meta("note").await.unwrap().unwrap().title,
            "Title"
        );
        assert!(!directory.join("tasks.json").exists());
    }

    #[tokio::test]
    async fn task_file_growth_is_rejected_before_any_note_title_or_task_bytes_commit() {
        let (_root, store, operations) = fixture("Original").await;
        store.ensure_summary("note").await.unwrap();
        store
            .replace_tasks(
                "session_summary",
                "note",
                vec![crate::TaskInput {
                    id: "large".into(),
                    source_order: 0,
                    status: "todo".into(),
                    text: "Existing task".into(),
                    body: json!("x".repeat(7 * 1024 * 1024)),
                    due_at: String::new(),
                }],
            )
            .await
            .unwrap();
        let before = snapshot(&store, &operations).await;
        let directory = store.vault_base().join("sessions/note");
        let before_tasks = crate::sha256(&std::fs::read(directory.join("tasks.json")).unwrap());
        let before_meta = std::fs::read(directory.join("_meta.json")).unwrap();
        let markdown = format!("- [ ] {}", "x".repeat(600 * 1024));
        let outcome = store
            .save_note_with_recovery(
                &operations,
                "note",
                &markdown,
                Some("Do not rename"),
                &before.revision,
            )
            .await;
        assert!(matches!(outcome, Err(StoreError::ResourceLimit(_))));
        assert_eq!(snapshot(&store, &operations).await, before);
        assert_eq!(
            std::fs::read(directory.join("_meta.json")).unwrap(),
            before_meta
        );
        assert_eq!(
            crate::sha256(&std::fs::read(directory.join("tasks.json")).unwrap()),
            before_tasks
        );
        assert!(!operations.exists());
    }

    #[tokio::test]
    async fn checked_note_reads_reject_oversize_files_without_changing_them() {
        let (_root, store, operations) = fixture("Original").await;
        let path = store.vault_base().join("sessions/note/notes.md");
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_len(16 * 1024 * 1024 + 1).unwrap();
        assert!(matches!(
            store
                .read_recovered_note_snapshot(Some(&operations), "note")
                .await,
            Err(StoreError::ResourceLimit(_))
        ));
        assert_eq!(std::fs::metadata(path).unwrap().len(), 16 * 1024 * 1024 + 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn checked_note_saves_never_follow_symbolic_links_into_user_files() {
        let (root, store, operations) = fixture("Original").await;
        let before = snapshot(&store, &operations).await;
        let outside = root.path().join("outside");
        std::fs::write(&outside, "User file").unwrap();
        let path = store.vault_base().join("sessions/note/notes.md");
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        assert!(
            store
                .read_recovered_note_snapshot(Some(&operations), "note")
                .await
                .is_err()
        );
        assert!(
            store
                .save_note_with_recovery(&operations, "note", "Never write", None, &before.revision)
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(outside).unwrap(), "User file");
        assert!(
            std::fs::symlink_metadata(path)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
