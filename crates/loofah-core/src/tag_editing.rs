use std::sync::Arc;

use hypr_loofah_protocol::{
    ApplicationError, EnsureTagRequest, ErrorCode, MetadataSaveOutcome, MutationOutcome, TagItem,
    TagSuggestionActionRequest, TagSuggestionsRequest,
};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn tag_suggestions(
        self: &Arc<Self>,
        input: TagSuggestionsRequest,
    ) -> Result<Vec<String>, ApplicationError> {
        hypr_loofah_protocol::validate_tag_suggestions_request(&input)
            .map_err(|message| error(ErrorCode::ResourceLimit, message))?;
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is not ready"))?;
        let context = self.clone();
        tokio::spawn(async move {
            context
                .store
                .tag_suggestions(&input.query, &input.excluded, input.limit as usize)
                .await
                .map_err(|cause| storage_error(cause, false))
        })
        .await
        .map_err(|cause| join_error(cause, false))?
    }

    pub async fn ensure_tag(
        self: &Arc<Self>,
        input: EnsureTagRequest,
    ) -> Result<TagItem, ApplicationError> {
        validate_name(&input.name).map_err(not_committed)?;
        self.store
            .ensure_ready()
            .map_err(|_| not_committed(error(ErrorCode::Unavailable, "Vault is not ready")))?;
        let context = self.clone();
        tokio::spawn(async move {
            context
                .store
                .ensure_tag_checked(&input.name)
                .await
                .map_err(|cause| storage_error(cause, true))
        })
        .await
        .map_err(|cause| join_error(cause, true))?
    }

    pub async fn accept_tag_suggestion(
        self: &Arc<Self>,
        session_id: &str,
        input: TagSuggestionActionRequest,
    ) -> Result<MetadataSaveOutcome, ApplicationError> {
        self.edit_tag_suggestion(session_id, input, true).await
    }

    pub async fn dismiss_tag_suggestion(
        self: &Arc<Self>,
        session_id: &str,
        input: TagSuggestionActionRequest,
    ) -> Result<MetadataSaveOutcome, ApplicationError> {
        self.edit_tag_suggestion(session_id, input, false).await
    }

    async fn edit_tag_suggestion(
        self: &Arc<Self>,
        session_id: &str,
        input: TagSuggestionActionRequest,
        accept: bool,
    ) -> Result<MetadataSaveOutcome, ApplicationError> {
        self.validate(session_id).map_err(not_committed)?;
        validate_name(&input.name).map_err(not_committed)?;
        hypr_loofah_protocol::validate_tag_suggestion_action(&input)
            .map_err(|message| not_committed(error(ErrorCode::Validation, message)))?;
        let context = self.clone();
        let session_id = session_id.to_owned();
        tokio::spawn(async move {
            let root = context.document_operation_root();
            let outcome = if accept {
                context
                    .store
                    .accept_tag_suggestion_with_recovery(
                        root.as_deref(),
                        &session_id,
                        &input.name,
                        &input.expected_revision,
                    )
                    .await
            } else {
                context
                    .store
                    .dismiss_tag_suggestion_with_recovery(
                        root.as_deref(),
                        &session_id,
                        &input.name,
                        &input.expected_revision,
                    )
                    .await
            };
            outcome
                .map_err(|cause| storage_error(cause, true))?
                .ok_or_else(|| not_committed(error(ErrorCode::NotFound, "Session not found")))
        })
        .await
        .map_err(|cause| join_error(cause, true))?
    }
}

fn validate_name(name: &str) -> Result<(), ApplicationError> {
    hypr_loofah_protocol::validate_tag_name(name).map_err(|message| {
        error(
            if name.len() > hypr_loofah_protocol::TAG_NAME_BYTES {
                ErrorCode::ResourceLimit
            } else {
                ErrorCode::Validation
            },
            message,
        )
    })
}

fn not_committed(mut failure: ApplicationError) -> ApplicationError {
    failure.mutation_outcome = Some(MutationOutcome::NotCommitted);
    failure
}

fn join_error(cause: tokio::task::JoinError, mutation: bool) -> ApplicationError {
    tracing::error!(%cause, "tag operation worker stopped");
    let mut failure = error(ErrorCode::Internal, "Tag operation could not be confirmed");
    if mutation {
        failure.mutation_outcome = Some(MutationOutcome::Unknown);
    }
    failure
}

#[cfg(test)]
mod tests;
