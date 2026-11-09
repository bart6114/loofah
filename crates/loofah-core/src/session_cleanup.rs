use std::sync::Arc;

use hypr_loofah_protocol::{
    ApplicationError, DeleteEmptySessionOutcome, DeleteEmptySessionRequest, ErrorCode,
    MutationOutcome,
};
use hypr_vault_write::{SessionCleanupError, SessionCleanupKey, StoreError};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    /// Outcomes describe this request's historical decision; callers must not infer current
    /// absence or trigger deletion side effects from a replayed `Deleted` response.
    pub async fn delete_empty_session(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        input: DeleteEmptySessionRequest,
    ) -> Result<DeleteEmptySessionOutcome, ApplicationError> {
        self.delete_empty_session_authorized(namespace, request_id, input, || async { Ok(()) })
            .await
    }

    pub async fn delete_empty_session_authorized<
        G: Send + 'static,
        F: std::future::Future<Output = Result<G, ApplicationError>> + Send + 'static,
    >(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        input: DeleteEmptySessionRequest,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<DeleteEmptySessionOutcome, ApplicationError> {
        hypr_loofah_protocol::validate_delete_empty_session_request(&input)
            .map_err(|message| not_committed(error(ErrorCode::Validation, message)))?;
        self.validate(&input.session_id).map_err(not_committed)?;
        let request_id = uuid::Uuid::parse_str(&request_id).map_err(|_| {
            not_committed(error(
                ErrorCode::Validation,
                "A UUID request ID is required",
            ))
        })?;
        if namespace.is_empty() || namespace.len() > 4096 {
            return Err(not_committed(error(
                ErrorCode::Validation,
                "Invalid cleanup identity",
            )));
        }
        let root = self
            .operations
            .as_ref()
            .map(|operations| operations.root.join("session-cleanups"))
            .ok_or_else(|| {
                not_committed(error(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                ))
            })?;
        let context = self.clone();
        tokio::spawn(async move {
            // The grant and context must outlive a dropped caller, including blocked filesystem I/O.
            let _grant = authorize().await.map_err(not_committed)?;
            let key = SessionCleanupKey {
                vault_id: context.events.vault_id(),
                namespace,
                request_id,
            };
            context
                .store
                .delete_empty_session(
                    &root,
                    context.document_operation_root().as_deref(),
                    key,
                    input,
                )
                .await
                .map_err(cleanup_error)
        })
        .await
        .map_err(|cause| {
            tracing::error!(%cause, "session cleanup worker stopped");
            let mut error = error(
                ErrorCode::Internal,
                "Cleanup outcome could not be confirmed",
            );
            error.mutation_outcome = Some(MutationOutcome::Unknown);
            error
        })?
    }
}

fn not_committed(mut cause: ApplicationError) -> ApplicationError {
    cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
    cause
}

fn cleanup_error(cause: SessionCleanupError) -> ApplicationError {
    match cause {
        SessionCleanupError::Busy => not_committed(error(
            ErrorCode::Unavailable,
            "Session is busy; retry after recording, processing or transfers finish",
        )),
        SessionCleanupError::Rejected(StoreError::Conflict(_)) => not_committed(error(
            ErrorCode::Conflict,
            "Cleanup request conflicts with an existing operation",
        )),
        SessionCleanupError::Rejected(cause) => not_committed(storage_error(cause, false)),
        SessionCleanupError::Uncertain(cause) => {
            tracing::error!(%cause, "session cleanup requires review");
            let mut error = error(
                ErrorCode::Conflict,
                "Cleanup outcome could not be confirmed",
            );
            error.mutation_outcome = Some(MutationOutcome::Unknown);
            error
        }
    }
}

#[cfg(test)]
mod tests;
