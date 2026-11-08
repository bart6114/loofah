use std::{future::Future, sync::Arc};

use hypr_loofah_protocol::{
    ApplicationError, DeleteDocumentOutcome, DeleteDocumentRequest, DocumentTarget, ErrorCode,
    MutationOutcome,
};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn delete_document(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        session_id: &str,
        target: &DocumentTarget,
        input: DeleteDocumentRequest,
    ) -> Result<DeleteDocumentOutcome, ApplicationError> {
        self.delete_document_authorized(
            namespace,
            request_id,
            session_id,
            target,
            input,
            || async { Ok(()) },
        )
        .await
    }

    pub async fn delete_document_authorized<F, U, G>(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        session_id: &str,
        target: &DocumentTarget,
        input: DeleteDocumentRequest,
        authorize: F,
    ) -> Result<DeleteDocumentOutcome, ApplicationError>
    where
        F: FnOnce() -> U + Send + 'static,
        U: Future<Output = Result<G, ApplicationError>> + Send + 'static,
        G: Send + 'static,
    {
        self.validate_editor_target(session_id, target)
            .map_err(before)?;
        if !matches!(target, DocumentTarget::TemplateOutput { .. }) {
            return Err(before(error(
                ErrorCode::Validation,
                "Only template outputs can be deleted",
            )));
        }
        hypr_loofah_protocol::validate_delete_document_request(&input)
            .map_err(|message| before(error(ErrorCode::Validation, message)))?;
        let request = uuid::Uuid::parse_str(&request_id)
            .ok()
            .filter(|id| !id.is_nil())
            .ok_or_else(|| {
                before(error(
                    ErrorCode::Validation,
                    "A deletion request UUID is required",
                ))
            })?;
        let context = self.clone();
        let session = session_id.to_owned();
        let target = target.clone();
        tokio::spawn(async move {
            let _grant = authorize().await.map_err(before)?;
            let _admission = context.admit_session_work(&session).await.map_err(before)?;
            let root = context.document_operation_root().ok_or_else(|| {
                before(error(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                ))
            })?;
            context
                .store
                .delete_document_checked(&root, &namespace, request, &session, &target, input)
                .await
                .map_err(|cause| storage_error(cause, true))
        })
        .await
        .map_err(|_| {
            with_outcome(
                error(ErrorCode::Internal, "Document deletion worker stopped"),
                MutationOutcome::Unknown,
            )
        })?
    }
}
fn before(error: ApplicationError) -> ApplicationError {
    with_outcome(error, MutationOutcome::NotCommitted)
}

fn with_outcome(mut error: ApplicationError, outcome: MutationOutcome) -> ApplicationError {
    error.mutation_outcome = Some(outcome);
    error
}
