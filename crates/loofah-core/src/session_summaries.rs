use std::sync::Arc;

use hypr_loofah_protocol::{ApplicationError, ErrorCode, SessionSummary, SessionSummaryQuery};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn session_summaries(
        self: &Arc<Self>,
        input: SessionSummaryQuery,
    ) -> Result<Vec<SessionSummary>, ApplicationError> {
        hypr_loofah_protocol::validate_session_summary_query(&input)
            .map_err(|message| error(ErrorCode::Validation, message))?;
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is not ready"))?;
        if let SessionSummaryQuery::Ids { ids } = &input {
            return self
                .store
                .canonical_session_summaries(self.document_operation_root().as_deref(), ids)
                .await
                .map_err(|cause| storage_error(cause, false));
        }
        let context = self.clone();
        tokio::task::spawn_blocking(move || {
            context
                .store
                .indexed_session_summaries(&input)
                .map_err(|cause| storage_error(cause, false))
        })
        .await
        .map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "Session summaries could not be read",
            )
        })?
    }
}
