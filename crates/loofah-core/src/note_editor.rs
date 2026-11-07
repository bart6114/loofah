use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, MutationOutcome, NoteEditorSaveOutcome, SaveNoteEditorRequest,
};

use crate::{
    VaultContext,
    tasks::{RequestBudget, validate_task_fields},
    vault::{error, storage_error, validate_note_fields},
};

impl VaultContext {
    pub async fn save_note_editor(
        &self,
        session_id: &str,
        input: SaveNoteEditorRequest,
    ) -> Result<NoteEditorSaveOutcome, ApplicationError> {
        self.validate(session_id).map_err(before_commit)?;
        let limits = self.store.content_limits();
        let input = tokio::task::spawn_blocking(move || {
            validate_note_fields(
                &input.markdown,
                input.title.as_deref(),
                &input.expected_revision,
                limits,
            )
            .map_err(before_commit)?;
            validate_task_fields(&input.tasks, &input.expected_tasks_revision, limits)
                .map_err(before_commit)?;
            if limits.task_request_bytes != usize::MAX {
                serde_json::to_writer(RequestBudget(limits.task_request_bytes), &input.tasks)
                    .map_err(|_| {
                        before_commit(error(
                            ErrorCode::ResourceLimit,
                            "Editor tasks exceed the request size limit",
                        ))
                    })?;
            }
            if limits.editor_request_bytes != usize::MAX {
                serde_json::to_writer(RequestBudget(limits.editor_request_bytes), &input).map_err(
                    |_| {
                        before_commit(error(
                            ErrorCode::ResourceLimit,
                            "Editor content exceeds the request size limit",
                        ))
                    },
                )?;
            }
            Ok::<_, ApplicationError>(input)
        })
        .await
        .map_err(|_| {
            before_commit(error(
                ErrorCode::Internal,
                "Editor validation worker stopped",
            ))
        })??;
        let root = self.document_operation_root().ok_or_else(|| {
            before_commit(error(
                ErrorCode::Unavailable,
                "Operation storage is not configured",
            ))
        })?;
        self.store
            .save_note_editor(&root, session_id, input)
            .await
            .map_err(|cause| storage_error(cause, true))?
            .ok_or_else(|| before_commit(error(ErrorCode::NotFound, "Session not found")))
    }
}

fn before_commit(mut error: ApplicationError) -> ApplicationError {
    error.mutation_outcome = Some(MutationOutcome::NotCommitted);
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_loofah_protocol::TaskInput;
    use serde_json::json;
    use std::sync::Arc;

    fn local_context(root: &std::path::Path) -> VaultContext {
        let vault = root.join("vault");
        VaultContext::new(
            "local".into(),
            Arc::new(hypr_vault_write::SessionStore::with_content_limits(
                vault.clone(),
                hypr_vault_write::ContentLimits::unlimited(),
            )),
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.join("runtime"))
    }

    #[tokio::test]
    async fn local_editor_preserves_large_legacy_notes_and_metadata_without_remote_admission_limits()
     {
        let root = tempfile::tempdir().unwrap();
        let context = local_context(root.path());
        let vault = context.store.vault_base();
        let directory = vault.join("sessions/note");
        std::fs::create_dir_all(&directory).unwrap();
        let legacy = "Legacy note text ".repeat(1024 * 1024);
        let metadata = json!({
            "id":"note", "title":"t".repeat(4097),
            "created_at":"2026-11-05T10:00:00Z", "tags":[],
            "future":{"preserve":"x".repeat(8 * 1024 * 1024)}
        });
        std::fs::write(
            directory.join("_meta.json"),
            serde_json::to_vec(&metadata).unwrap(),
        )
        .unwrap();
        std::fs::write(directory.join("_memo.md"), &legacy).unwrap();
        std::fs::write(directory.join("unknown.txt"), "User attachment").unwrap();
        std::fs::write(directory.join(".private"), "Private attachment").unwrap();
        let before = context.read_note_editor("note").await.unwrap();
        assert!(before.note.markdown.as_deref() == Some(legacy.as_str()));
        assert_eq!(before.note.title.len(), 4097);
        let markdown = format!("{legacy}\nEdited through shared IPC operations.");
        let input = SaveNoteEditorRequest {
            markdown: markdown.clone(),
            title: Some("s".repeat(4098)),
            expected_revision: before.note.revision,
            expected_tasks_revision: before.tasks.revision,
            tasks: vec![],
        };
        let NoteEditorSaveOutcome::Saved(saved) = context
            .save_note_editor("note", input.clone())
            .await
            .unwrap()
        else {
            panic!("local legacy note conflicted");
        };
        assert!(saved.note.markdown.as_deref() == Some(markdown.as_str()));
        assert!(std::fs::read_to_string(directory.join("notes.md")).unwrap() == markdown);
        assert_eq!(saved.note.title.len(), 4098);
        assert!(!directory.join("_memo.md").exists());
        let after: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("_meta.json")).unwrap()).unwrap();
        assert!(after["future"] == metadata["future"]);
        assert_eq!(
            std::fs::read_to_string(directory.join("unknown.txt")).unwrap(),
            "User attachment"
        );
        assert_eq!(
            std::fs::read_to_string(directory.join(".private")).unwrap(),
            "Private attachment"
        );
        assert!(matches!(
            context.save_note_editor("note", input).await.unwrap(),
            NoteEditorSaveOutcome::Conflict(_)
        ));
        drop(context);
        let reopened = local_context(root.path())
            .read_note_editor("note")
            .await
            .unwrap();
        assert_eq!(reopened.note.revision, saved.note.revision);
        assert!(reopened.note.markdown.as_deref() == Some(markdown.as_str()));
    }

    #[tokio::test]
    async fn local_editor_accepts_existing_large_task_sets_while_remote_defaults_stay_bounded() {
        let root = tempfile::tempdir().unwrap();
        let context = local_context(root.path());
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Title","created_at":"2026-11-05T10:00:00Z","tags":[]}),
        )
        .unwrap();
        context
            .store
            .create_note_session(&meta, "Original")
            .await
            .unwrap();
        let before = context.read_note_editor("note").await.unwrap();
        let mut tasks = (0..2001)
            .map(|index| TaskInput {
                id: format!("task-{index}"),
                source_order: index,
                status: "todo".into(),
                text: format!("Task {index}"),
                body: json!([]),
                due_at: String::new(),
            })
            .collect::<Vec<_>>();
        tasks[0].text = "Long existing task ".repeat(1024);
        tasks[0].body = json!([{"type":"paragraph","content":[{"type":"text","text":"x".repeat(9 * 1024 * 1024)}]}]);
        let input = SaveNoteEditorRequest {
            markdown: "Edited note".into(),
            title: None,
            expected_revision: before.note.revision,
            expected_tasks_revision: before.tasks.revision,
            tasks,
        };
        let NoteEditorSaveOutcome::Saved(saved) = context
            .save_note_editor("note", input.clone())
            .await
            .unwrap()
        else {
            panic!("local task set conflicted");
        };
        assert_eq!(saved.tasks.tasks.len(), 2001);
        assert!(
            std::fs::metadata(context.store.vault_base().join("sessions/note/tasks.json"))
                .unwrap()
                .len()
                > 8 * 1024 * 1024
        );
        let reopened = local_context(root.path())
            .read_note_editor("note")
            .await
            .unwrap();
        assert_eq!(reopened.tasks.revision, saved.tasks.revision);
        assert!(reopened.tasks.tasks == saved.tasks.tasks);

        let vault = context.store.vault_base();
        let bounded = VaultContext::new(
            "remote".into(),
            Arc::new(hypr_vault_write::SessionStore::new(vault.into())),
            StartupState::new(Some(vault)),
            Arc::new(ConfigState::load_or_default(vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.path().join("runtime"));
        let error = bounded.save_note_editor("note", input).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimit);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
        assert_eq!(
            context
                .read_note_editor("note")
                .await
                .unwrap()
                .tasks
                .revision,
            saved.tasks.revision
        );
    }

    #[tokio::test]
    async fn shared_editor_saves_validate_both_revisions_and_task_intent_before_file_changes() {
        let root = tempfile::tempdir().unwrap();
        let vault = root.path().join("vault");
        let context = VaultContext::new(
            "local".into(),
            Arc::new(hypr_vault_write::SessionStore::new(vault.clone())),
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.path().join("runtime"));
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Title","created_at":"2026-11-05T10:00:00Z","tags":[]}),
        )
        .unwrap();
        context
            .store
            .create_note_session(&meta, "Original")
            .await
            .unwrap();
        let before = context.read_note_editor("note").await.unwrap();
        let input = SaveNoteEditorRequest {
            markdown: "- [ ] Renamed task".into(),
            title: None,
            expected_revision: before.note.revision,
            expected_tasks_revision: before.tasks.revision,
            tasks: vec![TaskInput {
                id: "task".into(),
                source_order: 0,
                status: "in_progress".into(),
                text: "Renamed task".into(),
                body: json!([{"type":"paragraph","content":[{"type":"text","text":"Renamed task"}]}]),
                due_at: "2026-12-01".into(),
            }],
        };
        let NoteEditorSaveOutcome::Saved(saved) = context
            .save_note_editor("note", input.clone())
            .await
            .unwrap()
        else {
            panic!("expected saved editor")
        };
        assert_eq!(saved.tasks.tasks[0].status, "in_progress");
        assert!(matches!(
            context
                .save_note_editor("note", input.clone())
                .await
                .unwrap(),
            NoteEditorSaveOutcome::Conflict(_)
        ));
        let mut input = input;
        input.expected_revision = saved.note.revision;
        input.expected_tasks_revision = saved.tasks.revision;
        let directory = vault.join("sessions/note");
        let paths = ["notes.md", "_meta.json", "tasks.json"];
        let bytes: Vec<_> = paths
            .iter()
            .map(|path| std::fs::read(directory.join(path)).unwrap())
            .collect();
        let mut cases = Vec::new();
        let mut invalid = input.clone();
        invalid.expected_revision.clear();
        cases.push((invalid, ErrorCode::Validation));
        let mut invalid = input.clone();
        invalid.expected_tasks_revision.clear();
        cases.push((invalid, ErrorCode::Validation));
        let mut invalid = input.clone();
        invalid.tasks.push(invalid.tasks[0].clone());
        cases.push((invalid, ErrorCode::Validation));
        let mut invalid = input.clone();
        invalid.tasks[0].status.clear();
        cases.push((invalid, ErrorCode::Validation));
        let mut invalid = input.clone();
        invalid.tasks[0].text = "x".repeat(16 * 1024 + 1);
        cases.push((invalid, ErrorCode::ResourceLimit));
        let mut invalid = input.clone();
        invalid.markdown = "x".repeat(4 * 1024 * 1024 + 1);
        cases.push((invalid, ErrorCode::ResourceLimit));
        let mut invalid = input.clone();
        invalid.title = Some("x".repeat(4097));
        cases.push((invalid, ErrorCode::ResourceLimit));
        let mut invalid = input.clone();
        invalid.tasks[0].body = json!("x".repeat(4 * 1024 * 1024));
        cases.push((invalid, ErrorCode::ResourceLimit));
        let mut invalid = input.clone();
        invalid.markdown = "x".repeat(2 * 1024 * 1024);
        invalid.tasks[0].body = json!("x".repeat(3 * 1024 * 1024));
        cases.push((invalid, ErrorCode::ResourceLimit));
        let mut invalid = input.clone();
        invalid.tasks = (0..2001)
            .map(|index| TaskInput {
                id: index.to_string(),
                ..input.tasks[0].clone()
            })
            .collect();
        cases.push((invalid, ErrorCode::ResourceLimit));
        for (input, code) in cases {
            let error = context.save_note_editor("note", input).await.unwrap_err();
            assert_eq!(error.code, code);
            assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
            for (index, path) in paths.iter().enumerate() {
                assert_eq!(std::fs::read(directory.join(path)).unwrap(), bytes[index]);
            }
        }
        assert_eq!(
            context
                .save_note_editor("../outside", input.clone())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        context.store.set_startup_pending(true);
        assert_eq!(
            context
                .save_note_editor("note", input)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
    }
}
