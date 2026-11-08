use std::sync::Arc;

use hypr_loofah_protocol::{ApplicationError, ErrorCode, TranscriptOverview};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn transcript_overview(
        self: &Arc<Self>,
        session_id: &str,
        budget: Option<(usize, usize)>,
    ) -> Result<TranscriptOverview, ApplicationError> {
        self.validate(session_id)?;
        let context = self.clone();
        let session_id = session_id.to_owned();
        tokio::task::spawn_blocking(move || {
            context
                .store
                .indexed_transcript_overview(&session_id, budget)
                .map_err(|cause| storage_error(cause, false))?
                .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
        })
        .await
        .map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "Transcript overview could not be read",
            )
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    #[tokio::test]
    async fn missing_empty_and_invalid_sessions_use_shared_read_errors_without_word_io() {
        let vault = tempfile::tempdir().unwrap();
        let store = Arc::new(hypr_vault_write::SessionStore::new(vault.path().into()));
        let meta = serde_json::from_value(serde_json::json!({"id":"s","title":"Title","created_at":"2026-11-08T00:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "PRIVATE NOTE BODY")
            .await
            .unwrap();
        let context = Arc::new(VaultContext::new(
            "test".into(),
            store,
            StartupState::new(Some(vault.path())),
            Arc::new(ConfigState::load_or_default(vault.path())),
            Arc::new(SearchService::default()),
        ));
        assert_eq!(
            context.transcript_overview("s", None).await.unwrap(),
            TranscriptOverview::default()
        );
        std::fs::write(vault.path().join("sessions/s/transcript.json"), br#"{"transcripts":[{"id":"t","started_at":5,"words":[{"ignored":"not a materializable word"}]}]}"#).unwrap();
        context.store.refresh_session("s").await.unwrap();
        let overview = context.transcript_overview("s", None).await.unwrap();
        assert!(overview.has_words);
        assert_eq!(overview.started_at, Some(5.0));
        assert_eq!(overview.ended_at, None);
        assert_eq!(
            context
                .transcript_overview("missing", None)
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            context
                .transcript_overview("../private", None)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
}
