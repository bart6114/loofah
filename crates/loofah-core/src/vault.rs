use std::sync::Arc;

use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, MutationOutcome, NoteSaveOutcome, NoteSnapshot, SaveNoteRequest,
};
use hypr_vault_write::{EnhancedDocPatch, ScoredTagSuggestion, SessionStore, StoreError};

use crate::{
    events::EventHub, search::SearchService, settings::ConfigState, startup::StartupState,
};

pub struct VaultContext {
    pub store: Arc<SessionStore>,
    pub startup: StartupState,
    pub settings: Arc<ConfigState>,
    pub search: Arc<SearchService>,
    pub events: EventHub,
    pub(crate) lease: Option<std::fs::File>,
}

impl VaultContext {
    pub fn new(
        id: String,
        store: Arc<SessionStore>,
        startup: StartupState,
        settings: Arc<ConfigState>,
        search: Arc<SearchService>,
    ) -> Self {
        Self {
            store,
            startup,
            settings,
            search,
            events: EventHub::new(id, 256),
            lease: None,
        }
    }

    fn validate(&self, session_id: &str) -> Result<(), ApplicationError> {
        hypr_vault_read::paths::validate_session_id(session_id)
            .map_err(|_| error(ErrorCode::Validation, "Invalid session identifier"))?;
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault startup is still in progress"))
    }

    pub async fn read_note(&self, session_id: &str) -> Result<NoteSnapshot, ApplicationError> {
        self.validate(session_id)?;
        self.store
            .read_note_snapshot(session_id)
            .await
            .map_err(|cause| storage_error(cause, false))?
            .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
    }

    pub async fn save_note(
        &self,
        session_id: &str,
        request: SaveNoteRequest,
    ) -> Result<NoteSaveOutcome, ApplicationError> {
        self.validate(session_id)?;
        if request.expected_revision.is_empty() {
            return Err(error(
                ErrorCode::Validation,
                "Expected note revision is required",
            ));
        }
        // Existence is rechecked under the store's write lock before any bytes are replaced.
        self.read_note(session_id).await?;
        self.store
            .save_note_if_revision(
                session_id,
                &request.markdown,
                request.title.as_deref(),
                &request.expected_revision,
            )
            .await
            .map_err(|cause| storage_error(cause, true))
    }

    pub async fn update_summary(
        &self,
        session_id: &str,
        markdown: &str,
        expected_markdown: Option<&str>,
        reconcile_tasks: bool,
        suggested_tags: Option<Vec<ScoredTagSuggestion>>,
    ) -> Result<(), StoreError> {
        self.store.ensure_ready()?;
        self.store
            .update_summary_with_suggestions(
                session_id,
                markdown,
                expected_markdown,
                reconcile_tasks,
                suggested_tags,
                self.settings.snapshot().auto_apply_high_confidence_tags,
            )
            .await
    }

    pub async fn save_enhanced_doc(
        &self,
        session_id: &str,
        doc_id: &str,
        patch: EnhancedDocPatch,
        title: Option<&str>,
    ) -> Result<(), StoreError> {
        self.store.ensure_ready()?;
        if patch.kind.as_deref() == Some("summary") {
            return Err(StoreError::Serialize(
                "Use session_ensure_summary for session summaries".into(),
            ));
        }
        self.store
            .save_enhanced_doc_with_auto_apply(
                session_id,
                doc_id,
                patch,
                title,
                self.settings.snapshot().auto_apply_high_confidence_tags,
            )
            .await
    }
}

fn error(code: ErrorCode, message: &str) -> ApplicationError {
    ApplicationError {
        code,
        message: message.into(),
        request_id: None,
        retryable: code == ErrorCode::Unavailable,
        current_revision: None,
        mutation_outcome: None,
    }
}

fn storage_error(cause: StoreError, mutation: bool) -> ApplicationError {
    tracing::error!(%cause, "vault operation failed");
    let mut result = error(ErrorCode::Internal, "Vault operation failed");
    if mutation {
        result.mutation_outcome = Some(MutationOutcome::Unknown);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn headless_note_operations_enforce_readiness_validate_and_report_conflicts() {
        let vault = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(vault.path().into()));
        let context = VaultContext::new(
            "private".into(),
            store.clone(),
            StartupState::new(Some(vault.path())),
            Arc::new(ConfigState::load_or_default(vault.path())),
            Arc::new(SearchService::default()),
        );
        store.set_startup_pending(true);
        assert_eq!(
            context.read_note("note").await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        store.set_startup_pending(false);
        assert_eq!(
            context.read_note("../private").await.unwrap_err().code,
            ErrorCode::Validation
        );
        assert_eq!(
            context.read_note("missing").await.unwrap_err().code,
            ErrorCode::NotFound
        );
        let meta = serde_json::from_value(serde_json::json!({
            "id": "note", "title": "Original", "created_at": "2026-11-05T10:00:00Z", "tags": []
        }))
        .unwrap();
        store.write_meta(&meta).await.unwrap();
        let initial = context.read_note("note").await.unwrap();
        let request = SaveNoteRequest {
            markdown: "saved".into(),
            title: Some("New".into()),
            expected_revision: initial.revision,
        };
        assert!(matches!(
            context.save_note("note", request.clone()).await.unwrap(),
            NoteSaveOutcome::Saved(_)
        ));
        let NoteSaveOutcome::Conflict(latest) = context.save_note("note", request).await.unwrap()
        else {
            panic!("stale write succeeded");
        };
        assert_eq!(latest.markdown.as_deref(), Some("saved"));
        assert_eq!(latest.title, "New");
        assert_eq!(context.read_note("note").await.unwrap(), latest);
    }
}
