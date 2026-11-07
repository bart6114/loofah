use std::sync::{Arc, Weak};

use hypr_loofah_protocol::{ApplicationError, ErrorCode, MutationOutcome};
pub use hypr_vault_write::{GeneratedTitleOutcome, RawNoteTitleOutcome};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

#[derive(Debug)]
pub enum GeneratedTitlePreparation {
    AlreadyNamed,
    Ready(GeneratedTitleBaseline),
}

#[derive(Debug)]
pub struct GeneratedTitleBaseline {
    owner: Weak<VaultContext>,
    storage: hypr_vault_write::GeneratedTitleBaseline,
}

impl VaultContext {
    pub async fn prepare_generated_title(
        self: &Arc<Self>,
        session_id: &str,
    ) -> Result<GeneratedTitlePreparation, ApplicationError> {
        self.validate(session_id)?;
        let root = self.document_operation_root().ok_or_else(|| {
            error(
                ErrorCode::Unavailable,
                "Operation storage is not configured",
            )
        })?;
        let context = self.clone();
        let id = session_id.to_owned();
        tokio::spawn(async move {
            match context
                .store
                .prepare_generated_title(&root, &id)
                .await
                .map_err(|cause| storage_error(cause, false))?
            {
                hypr_vault_write::GeneratedTitlePreparation::AlreadyNamed => {
                    Ok(GeneratedTitlePreparation::AlreadyNamed)
                }
                hypr_vault_write::GeneratedTitlePreparation::Ready(storage) => {
                    Ok(GeneratedTitlePreparation::Ready(GeneratedTitleBaseline {
                        owner: Arc::downgrade(&context),
                        storage,
                    }))
                }
            }
        })
        .await
        .map_err(|cause| join_error(cause, false))?
    }

    pub async fn apply_generated_title(
        self: &Arc<Self>,
        baseline: GeneratedTitleBaseline,
        candidate: String,
    ) -> Result<GeneratedTitleOutcome, ApplicationError> {
        if !Weak::ptr_eq(&baseline.owner, &Arc::downgrade(self)) {
            let mut failure = error(
                ErrorCode::Validation,
                "Title baseline belongs to another vault context",
            );
            failure.mutation_outcome = Some(MutationOutcome::NotCommitted);
            return Err(failure);
        }
        let root = self.document_operation_root().ok_or_else(|| {
            error(
                ErrorCode::Unavailable,
                "Operation storage is not configured",
            )
        })?;
        let context = self.clone();
        tokio::spawn(async move {
            context
                .store
                .apply_generated_title(&root, baseline.storage, candidate)
                .await
                .map_err(|cause| storage_error(cause, true))
        })
        .await
        .map_err(|cause| join_error(cause, true))?
    }
}

fn join_error(cause: tokio::task::JoinError, mutation: bool) -> ApplicationError {
    tracing::error!(%cause,"generated title worker stopped");
    let mut failure = error(
        ErrorCode::Internal,
        "Title operation could not be confirmed",
    );
    if mutation {
        failure.mutation_outcome = Some(MutationOutcome::Unknown);
    }
    failure
}

#[cfg(test)]
mod tests;
