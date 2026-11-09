use crate::{
    VaultContext,
    vault::{error, storage_error},
};
use hypr_loofah_protocol::{
    ApplicationError, DocumentEditorSaveOutcome, DocumentEditorSnapshot, DocumentTarget, ErrorCode,
    MutationOutcome, SaveDocumentEditorRequest,
};
use std::{future::Future, sync::Arc};

impl VaultContext {
    pub async fn read_document_editor(
        self: &Arc<Self>,
        id: &str,
        target: &DocumentTarget,
    ) -> Result<DocumentEditorSnapshot, ApplicationError> {
        self.read_document_editor_authorized(id, target, || async { Ok(()) })
            .await
    }

    pub async fn read_document_editor_authorized<F, U, G>(
        self: &Arc<Self>,
        id: &str,
        target: &DocumentTarget,
        authorize: F,
    ) -> Result<DocumentEditorSnapshot, ApplicationError>
    where
        F: FnOnce() -> U + Send + 'static,
        U: Future<Output = Result<G, ApplicationError>> + Send + 'static,
        G: Send + 'static,
    {
        self.validate_editor_target(id, target)?;
        let context = self.clone();
        let id = id.to_owned();
        let target = target.clone();
        tokio::spawn(async move {
            let _grant = authorize().await?;
            let snapshot = context
                .store
                .read_document_editor(context.document_operation_root().as_deref(), &id, &target)
                .await
                .map_err(|cause| storage_error(cause, false))?
                .ok_or_else(|| error(ErrorCode::NotFound, "Document editor target not found"))?;
            Ok(snapshot)
        })
        .await
        .map_err(|cause| {
            error(
                ErrorCode::Internal,
                &format!("Document editor reader failed: {cause}"),
            )
        })?
    }

    pub async fn save_document_editor(
        self: &Arc<Self>,
        id: &str,
        target: &DocumentTarget,
        input: SaveDocumentEditorRequest,
    ) -> Result<DocumentEditorSaveOutcome, ApplicationError> {
        self.save_document_editor_authorized(id, target, input, || async { Ok(()) })
            .await
    }

    pub async fn save_document_editor_authorized<F, U, G>(
        self: &Arc<Self>,
        id: &str,
        target: &DocumentTarget,
        input: SaveDocumentEditorRequest,
        authorize: F,
    ) -> Result<DocumentEditorSaveOutcome, ApplicationError>
    where
        F: FnOnce() -> U + Send + 'static,
        U: Future<Output = Result<G, ApplicationError>> + Send + 'static,
        G: Send + 'static,
    {
        self.validate_editor_target(id, target).map_err(before)?;
        let context = self.clone();
        let id = id.to_owned();
        let target = target.clone();
        tokio::spawn(async move {
            let limits = context.store.content_limits();
            let input = tokio::task::spawn_blocking(move || {
                crate::vault::validate_note_fields(
                    &input.markdown,
                    input.title.as_deref(),
                    &input.expected_revision,
                    limits,
                )
                .map_err(before)?;
                crate::tasks::validate_task_fields(
                    &input.tasks,
                    &input.expected_tasks_revision,
                    limits,
                )
                .map_err(before)?;
                if input.expected_metadata_revision.is_empty()
                    || input.expected_metadata_revision.len() > 128
                {
                    return Err(before(error(
                        ErrorCode::Validation,
                        "Expected metadata revision is required",
                    )));
                }
                if limits.task_request_bytes != usize::MAX {
                    serde_json::to_writer(
                        crate::tasks::RequestBudget(limits.task_request_bytes),
                        &input.tasks,
                    )
                    .map_err(|_| {
                        before(error(
                            ErrorCode::ResourceLimit,
                            "Editor tasks exceed request limit",
                        ))
                    })?;
                }
                if limits.editor_request_bytes != usize::MAX {
                    serde_json::to_writer(
                        crate::tasks::RequestBudget(limits.editor_request_bytes),
                        &input,
                    )
                    .map_err(|_| {
                        before(error(
                            ErrorCode::ResourceLimit,
                            "Editor exceeds request limit",
                        ))
                    })?;
                }
                Ok::<_, ApplicationError>(input)
            })
            .await
            .map_err(|cause| {
                before(error(
                    ErrorCode::Internal,
                    &format!("Editor validation worker failed: {cause}"),
                ))
            })??;
            let root = context.document_operation_root().ok_or_else(|| {
                before(error(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                ))
            })?;
            // Match other HTTP writers: catalog admission precedes the store lock.
            // A queued catalog writer must never be waited on while holding that lock.
            let _grant = authorize().await.map_err(before)?;
            let preparation = context
                .store
                .prepare_document_editor(&root, &id, &target, input)
                .await
                .map_err(|cause| storage_error(cause, true))?
                .ok_or_else(|| {
                    before(error(
                        ErrorCode::NotFound,
                        "Document editor target not found",
                    ))
                })?;
            match preparation {
                hypr_vault_write::DocumentEditorPreparation::Conflict(snapshot) => {
                    Ok(DocumentEditorSaveOutcome::Conflict(*snapshot))
                }
                hypr_vault_write::DocumentEditorPreparation::Prepared(plan) => {
                    let snapshot = plan
                        .commit()
                        .await
                        .map_err(|cause| storage_error(cause, true))?;
                    Ok(DocumentEditorSaveOutcome::Saved(snapshot))
                }
            }
        })
        .await
        .map_err(|cause| {
            let mut failure = error(
                ErrorCode::Internal,
                &format!("Editor save worker failed: {cause}"),
            );
            failure.mutation_outcome = Some(MutationOutcome::Unknown);
            failure
        })?
    }

    pub(crate) fn validate_editor_target(
        &self,
        id: &str,
        target: &DocumentTarget,
    ) -> Result<(), ApplicationError> {
        self.validate(id)?;
        if let DocumentTarget::TemplateOutput { document_id } = target {
            hypr_vault_read::paths::validate_session_id(document_id)
                .map_err(|_| error(ErrorCode::Validation, "Invalid document identifier"))?;
        }
        Ok(())
    }
}

fn before(mut error: ApplicationError) -> ApplicationError {
    error.mutation_outcome = Some(MutationOutcome::NotCommitted);
    error
}

#[cfg(test)]
mod tests;
