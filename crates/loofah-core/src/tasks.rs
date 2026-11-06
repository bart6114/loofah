use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, ReplaceTasksRequest, TaskInput, TaskSource, TasksSaveOutcome,
    TasksSnapshot,
};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn read_tasks_snapshot(
        &self,
        session_id: &str,
    ) -> Result<TasksSnapshot, ApplicationError> {
        self.validate(session_id)?;
        let root = self.document_operation_root();
        self.store
            .read_tasks_snapshot(root.as_deref(), session_id)
            .await
            .map_err(|cause| storage_error(cause, false))?
            .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
    }

    pub async fn replace_tasks(
        &self,
        session_id: &str,
        input: ReplaceTasksRequest,
    ) -> Result<TasksSaveOutcome, ApplicationError> {
        self.validate(session_id)?;
        let input = tokio::task::spawn_blocking(move || {
            validate(&input)?;
            Ok::<_, ApplicationError>(input)
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Task validation worker stopped"))??;
        let operations = self.operations.as_ref().ok_or_else(|| {
            error(
                ErrorCode::Unavailable,
                "Operation storage is not configured",
            )
        })?;
        self.store
            .replace_tasks_if_revision(
                &operations.root.join("document-saves"),
                session_id,
                &input.source,
                input.tasks,
                &input.expected_revision,
            )
            .await
            .map_err(|cause| storage_error(cause, true))?
            .ok_or_else(|| error(ErrorCode::NotFound, "Task source not found"))
    }
}

fn validate(input: &ReplaceTasksRequest) -> Result<(), ApplicationError> {
    validate_task_fields(&input.tasks, &input.expected_revision)?;
    if let TaskSource::TemplateOutput { document_id } = &input.source {
        hypr_vault_read::paths::validate_session_id(document_id)
            .map_err(|_| error(ErrorCode::Validation, "Invalid task source identifier"))?;
    }
    serde_json::to_writer(RequestBudget(4 * 1024 * 1024), input).map_err(|_| {
        error(
            ErrorCode::ResourceLimit,
            "Task replacement exceeds its size limit",
        )
    })?;
    Ok(())
}

pub(crate) fn validate_task_fields(
    tasks: &[TaskInput],
    expected_revision: &str,
) -> Result<(), ApplicationError> {
    if expected_revision.is_empty() || expected_revision.len() > 128 {
        return Err(error(
            ErrorCode::Validation,
            "Expected task revision is required",
        ));
    }
    if tasks.len() > 2000 {
        return Err(error(
            ErrorCode::ResourceLimit,
            "Too many tasks in one replacement",
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for task in tasks {
        if task.id.is_empty() || task.status.trim().is_empty() || !ids.insert(&task.id) {
            return Err(error(
                ErrorCode::Validation,
                "Task identifiers must be nonempty and unique, with a nonempty status",
            ));
        }
        if task.id.len() > 512
            || task.status.len() > 128
            || task.text.len() > 16 * 1024
            || task.due_at.len() > 256
        {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Task fields exceed their size limits",
            ));
        }
    }
    Ok(())
}

pub(crate) struct RequestBudget(pub(crate) usize);
impl std::io::Write for RequestBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.0 {
            return Err(std::io::Error::other("task request size limit"));
        }
        self.0 -= bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_loofah_protocol::TaskInput;
    use serde_json::json;
    use std::sync::Arc;

    #[tokio::test]
    async fn shared_task_operations_validate_requests_and_use_canonical_file_revisions() {
        let root = tempfile::tempdir().unwrap();
        let vault = root.path().join("vault");
        let context = VaultContext::new(
            "vault".into(),
            Arc::new(hypr_vault_write::SessionStore::new(vault.clone())),
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.path().join("runtime"));
        context.store.set_startup_pending(true);
        assert_eq!(
            context.read_tasks_snapshot("note").await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        context.store.set_startup_pending(false);
        assert_eq!(
            context
                .read_tasks_snapshot("../other")
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        assert_eq!(
            context
                .read_tasks_snapshot("missing")
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Note","created_at":"2026-11-05T10:00:00Z","tags":[]}),
        )
        .unwrap();
        context
            .store
            .create_note_session(&meta, "User content")
            .await
            .unwrap();
        let initial = context.read_tasks_snapshot("note").await.unwrap();
        let task = TaskInput {
            id: "task".into(),
            source_order: 0,
            status: "done".into(),
            text: "Task".into(),
            body: json!([]),
            due_at: "".into(),
        };
        let input = ReplaceTasksRequest {
            source: TaskSource::RawNote,
            tasks: vec![task.clone()],
            expected_revision: initial.revision,
        };
        let first = context.replace_tasks("note", input.clone()).await.unwrap();
        assert!(
            matches!(&first,TasksSaveOutcome::Saved(snapshot) if snapshot.tasks.len()==1 && snapshot.tasks[0].status=="done")
        );
        assert!(matches!(
            context.replace_tasks("note", input.clone()).await.unwrap(),
            TasksSaveOutcome::Conflict(_)
        ));
        let before = std::fs::read(vault.join("sessions/note/tasks.json")).unwrap();
        let mut cases = Vec::new();
        let mut invalid = input.clone();
        invalid.expected_revision.clear();
        cases.push((invalid, ErrorCode::Validation));
        let mut invalid = input.clone();
        invalid.tasks.push(task.clone());
        cases.push((invalid, ErrorCode::Validation));
        let mut invalid = input.clone();
        invalid.tasks[0].status.clear();
        cases.push((invalid, ErrorCode::Validation));
        let mut invalid = input.clone();
        invalid.tasks[0].text = "x".repeat(16 * 1024 + 1);
        cases.push((invalid, ErrorCode::ResourceLimit));
        let mut invalid = input.clone();
        invalid.tasks[0].body = json!("x".repeat(4 * 1024 * 1024));
        cases.push((invalid, ErrorCode::ResourceLimit));
        let mut invalid = input.clone();
        invalid.source = TaskSource::TemplateOutput {
            document_id: "../foreign".into(),
        };
        cases.push((invalid, ErrorCode::Validation));
        let mut invalid = input.clone();
        invalid.tasks = (0..2001)
            .map(|index| TaskInput {
                id: index.to_string(),
                ..task.clone()
            })
            .collect();
        cases.push((invalid, ErrorCode::ResourceLimit));
        for (input, code) in cases {
            assert_eq!(
                context.replace_tasks("note", input).await.unwrap_err().code,
                code
            );
            assert_eq!(
                std::fs::read(vault.join("sessions/note/tasks.json")).unwrap(),
                before
            );
        }
        let oversized = hypr_vault_write::StoreError::ResourceLimit("Limit".into());
        let error = storage_error(oversized, true);
        assert_eq!(error.code, ErrorCode::ResourceLimit);
        assert_eq!(
            error.mutation_outcome,
            Some(hypr_loofah_protocol::MutationOutcome::NotCommitted)
        );
    }
}
