use hypr_loofah_protocol::ApplicationError;

use crate::VaultContext;

impl VaultContext {
    pub(crate) async fn ensure_session_jobs_idle(&self, id: &str) -> Result<(), ApplicationError> {
        if let Some(service) = self.processing.get() {
            service.journal.ensure_session_idle(id.to_owned()).await?;
        } else {
            let root = self
                .operations
                .as_ref()
                .ok_or_else(crate::session_lifecycle_gate::session_lifecycle_busy)?
                .root
                .join("jobs");
            crate::jobs::lifecycle::ensure_stored_session_idle(
                root,
                self.events.vault_id(),
                id.to_owned(),
            )
            .await?;
        }
        self.ensure_transcription_session_idle(id).await
    }
}

use crate::vault::{error, storage_error};
use hypr_loofah_protocol::{
    DeleteSessionOutcome, DeleteSessionRequest, ErrorCode, MutationOutcome, RestoreSessionOutcome,
    RestoreSessionRequest, SessionLifecycleSnapshot,
};
use hypr_vault_write::{LifecycleError, SessionLifecycleKey, SessionStore};
use std::{future::Future, sync::Arc};

impl VaultContext {
    pub async fn read_session_lifecycle(
        self: &Arc<Self>,
        id: &str,
    ) -> Result<SessionLifecycleSnapshot, ApplicationError> {
        self.read_session_lifecycle_authorized(id, || async { Ok(()) })
            .await
    }
    pub async fn read_session_lifecycle_authorized<G, F, U>(
        self: &Arc<Self>,
        id: &str,
        authorize: F,
    ) -> Result<SessionLifecycleSnapshot, ApplicationError>
    where
        G: Send + 'static,
        F: FnOnce() -> U + Send + 'static,
        U: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        self.validate(id)?;
        let context = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let _grant = authorize().await?;
            let _capacity = context
                .media_readers
                .clone()
                .try_acquire_owned()
                .map_err(|_| crate::session_lifecycle_gate::session_lifecycle_busy())?;
            let _admission = context.admit_session_work(&id).await?;
            context
                .store
                .read_session_lifecycle(context.document_operation_root().as_deref(), &id)
                .await
                .map_err(lifecycle_error)?
                .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
        })
        .await
        .map_err(|_| unknown("Lifecycle reader stopped"))?
    }
    pub async fn delete_session(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        input: DeleteSessionRequest,
    ) -> Result<DeleteSessionOutcome, ApplicationError> {
        self.delete_session_authorized(namespace, request_id, input, || async { Ok(()) })
            .await
    }
    pub async fn delete_session_authorized<G, F, U>(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        input: DeleteSessionRequest,
        authorize: F,
    ) -> Result<DeleteSessionOutcome, ApplicationError>
    where
        G: Send + 'static,
        F: FnOnce() -> U + Send + 'static,
        U: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        hypr_loofah_protocol::validate_delete_session_request(&input)
            .map_err(|message| before(error(ErrorCode::Validation, message)))?;
        self.validate(&input.session_id).map_err(before)?;
        let key = self.lifecycle_key(namespace, request_id)?;
        let root = self.lifecycle_root()?;
        let context = self.clone();
        tokio::spawn(async move {
            let _grant = authorize().await.map_err(before)?;
            if let Some(outcome) = SessionStore::session_delete_replay(&root, &key, &input)
                .await
                .map_err(lifecycle_error)?
            {
                return Ok(outcome);
            }
            let _request = context.admit_session_lifecycle_request(
                "delete",
                &key.namespace,
                key.request_id,
            )?;
            if let Some(outcome) = SessionStore::session_delete_replay(&root, &key, &input)
                .await
                .map_err(lifecycle_error)?
            {
                return Ok(outcome);
            }
            let has_intent = SessionStore::session_delete_has_intent(&root, &key, &input)
                .await
                .map_err(lifecycle_error)?;
            let result = async {
                let _capacity = context
                    .media_readers
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| crate::session_lifecycle_gate::session_lifecycle_busy())?;
                let _change = context.try_session_lifecycle_change(&input.session_id)?;
                context
                    .ensure_session_jobs_idle(&input.session_id)
                    .await
                    .map_err(before)?;
                context
                    .ensure_capture_session_lifecycle_idle(&input.session_id)
                    .await
                    .map_err(before)?;
                context
                    .store
                    .delete_session_checked(
                        &root,
                        context.document_operation_root().as_deref(),
                        key,
                        input,
                    )
                    .await
                    .map_err(lifecycle_error)
            }
            .await;
            result.map_err(|cause| pending_failure(cause, has_intent))
        })
        .await
        .map_err(|_| unknown("Deletion outcome could not be confirmed"))?
    }
    /// Trusted same-user CLI compatibility projection; never expose vault paths over HTTPS.
    pub async fn local_session_deletion_details(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        input: DeleteSessionRequest,
    ) -> Result<Option<hypr_vault_write::LocalSessionDeletionDetails>, ApplicationError> {
        hypr_loofah_protocol::validate_delete_session_request(&input)
            .map_err(|message| before(error(ErrorCode::Validation, message)))?;
        let key = self.lifecycle_key(namespace, request_id)?;
        self.store
            .session_delete_details(&self.lifecycle_root()?, &key, &input)
            .await
            .map_err(lifecycle_error)
    }
    pub async fn restore_session(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        input: RestoreSessionRequest,
    ) -> Result<RestoreSessionOutcome, ApplicationError> {
        self.restore_session_authorized(namespace, request_id, input, || async { Ok(()) })
            .await
    }
    pub async fn restore_session_authorized<G, F, U>(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        input: RestoreSessionRequest,
        authorize: F,
    ) -> Result<RestoreSessionOutcome, ApplicationError>
    where
        G: Send + 'static,
        F: FnOnce() -> U + Send + 'static,
        U: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        hypr_loofah_protocol::validate_restore_session_request(&input)
            .map_err(|message| before(error(ErrorCode::Validation, message)))?;
        let key = self.lifecycle_key(namespace, request_id)?;
        let root = self.lifecycle_root()?;
        let context = self.clone();
        tokio::spawn(async move {
            let _grant = authorize().await.map_err(before)?;
            context
                .store
                .ensure_ready()
                .map_err(|cause| before(storage_error(cause, false)))?;
            if let Some(outcome) = SessionStore::session_restore_replay(&root, &key, &input)
                .await
                .map_err(lifecycle_error)?
            {
                return Ok(outcome);
            }
            let _request = context.admit_session_lifecycle_request(
                "restore",
                &key.namespace,
                key.request_id,
            )?;
            if let Some(outcome) = SessionStore::session_restore_replay(&root, &key, &input)
                .await
                .map_err(lifecycle_error)?
            {
                return Ok(outcome);
            }
            let has_intent = SessionStore::session_restore_has_intent(&root, &key, &input)
                .await
                .map_err(lifecycle_error)?;
            let result = async {
                let target =
                    SessionStore::session_restore_target(&root, &key.vault_id, &input.trash_token)
                        .await
                        .map_err(lifecycle_error)?;
                let _capacity = context
                    .media_readers
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| crate::session_lifecycle_gate::session_lifecycle_busy())?;
                let _change = if let Some(id) = &target {
                    let guard = context.try_session_lifecycle_change(id)?;
                    context.ensure_session_jobs_idle(id).await.map_err(before)?;
                    context
                        .ensure_capture_session_lifecycle_idle(id)
                        .await
                        .map_err(before)?;
                    Some(guard)
                } else {
                    None
                };
                context
                    .store
                    .restore_session_checked(
                        &root,
                        context.document_operation_root().as_deref(),
                        key,
                        input,
                        target,
                    )
                    .await
                    .map_err(lifecycle_error)
            }
            .await;
            result.map_err(|cause| pending_failure(cause, has_intent))
        })
        .await
        .map_err(|_| unknown("Restore outcome could not be confirmed"))?
    }
    fn lifecycle_key(
        &self,
        namespace: String,
        request_id: String,
    ) -> Result<SessionLifecycleKey, ApplicationError> {
        let request_id = uuid::Uuid::parse_str(&request_id).map_err(|_| {
            before(error(
                ErrorCode::Validation,
                "A UUID request ID is required",
            ))
        })?;
        if namespace.is_empty() || namespace.len() > 4096 || namespace.chars().any(char::is_control)
        {
            return Err(before(error(
                ErrorCode::Validation,
                "Invalid lifecycle identity",
            )));
        }
        Ok(SessionLifecycleKey {
            vault_id: self.events.vault_id(),
            namespace,
            request_id,
        })
    }
    fn lifecycle_root(&self) -> Result<std::path::PathBuf, ApplicationError> {
        self.operations
            .as_ref()
            .map(|operations| operations.root.join("session-lifecycle"))
            .ok_or_else(|| {
                before(error(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                ))
            })
    }
}
fn pending_failure(mut cause: ApplicationError, has_intent: bool) -> ApplicationError {
    if has_intent {
        cause.mutation_outcome = Some(MutationOutcome::Unknown);
        cause.message =
            "An earlier lifecycle attempt may have committed; review its original request".into();
    }
    cause
}
fn before(mut cause: ApplicationError) -> ApplicationError {
    cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
    cause
}
fn unknown(message: &str) -> ApplicationError {
    let mut cause = error(ErrorCode::Conflict, message);
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}
fn lifecycle_error(cause: LifecycleError) -> ApplicationError {
    match cause {
        LifecycleError::Busy => crate::session_lifecycle_gate::session_lifecycle_busy(),
        LifecycleError::Rejected(hypr_vault_write::StoreError::Conflict(_)) => before(error(
            ErrorCode::Conflict,
            "Lifecycle request conflicts with current files or a retained operation",
        )),
        LifecycleError::Rejected(cause) => before(storage_error(cause, false)),
        LifecycleError::Uncertain(cause) => {
            tracing::error!(%cause,"session lifecycle requires review");
            unknown(
                "Lifecycle outcome could not be confirmed; review the original files and request",
            )
        }
    }
}

#[cfg(test)]
mod tests;
