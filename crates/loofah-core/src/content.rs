use hypr_loofah_protocol::{
    ApplicationError, EnhancedDoc, ErrorCode, PersonItem, TagItem, TaskItem, TranscriptWithData,
};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn save_generated_template_output(
        self: &std::sync::Arc<Self>,
        session_id: &str,
        document_id: &str,
        input: hypr_loofah_protocol::SaveDocumentRequest,
        suggestions: Option<Vec<hypr_vault_write::ScoredTagSuggestion>>,
    ) -> Result<hypr_loofah_protocol::DocumentSaveOutcome, ApplicationError> {
        let target = hypr_loofah_protocol::DocumentTarget::TemplateOutput {
            document_id: document_id.into(),
        };
        self.validate_document(session_id, &target)?;
        if input.expected_revision.is_empty() || input.expected_revision.len() > 128 {
            return Err(error(
                ErrorCode::Validation,
                "Expected document revision is required",
            ));
        }
        if input.markdown.len() > self.store.content_limits().note_write_bytes {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Generated template output is too large",
            ));
        }
        let root = self.document_operation_root().ok_or_else(|| {
            error(
                ErrorCode::Unavailable,
                "Operation storage is not configured",
            )
        })?;
        let context = self.clone();
        let session_id = session_id.to_owned();
        let document_id = document_id.to_owned();
        tokio::spawn(async move {
            let current = context.read_document(&session_id, &target).await?;
            if current.document.is_none() {
                return Err(error(ErrorCode::NotFound, "Template output not found"));
            }
            let policy = context.settings.read_current().await.map_err(|_| {
                error(
                    ErrorCode::Unavailable,
                    "Processing settings could not be read",
                )
            })?;
            context
                .store
                .save_generated_template_with_suggestions_if_revision(
                    &root,
                    &session_id,
                    &document_id,
                    input,
                    suggestions,
                    policy.auto_apply_high_confidence_tags,
                )
                .await
                .map_err(|cause| storage_error(cause, true))
        })
        .await
        .map_err(|cause| {
            tracing::error!(%cause, "generated template worker stopped");
            let mut failure = error(
                ErrorCode::Internal,
                "Template outcome could not be confirmed",
            );
            failure.mutation_outcome = Some(hypr_loofah_protocol::MutationOutcome::Unknown);
            failure
        })?
    }

    pub async fn recover_document_writes(&self) -> Result<(), ApplicationError> {
        if let Some(operations) = &self.operations {
            self.store
                .recover_document_writes(&operations.root.join("document-saves"))
                .await
                .map_err(|cause| storage_error(cause, false))?;
        }
        Ok(())
    }

    pub(crate) fn document_operation_root(&self) -> Option<std::path::PathBuf> {
        self.operations
            .as_ref()
            .map(|operations| operations.root.join("document-saves"))
    }

    pub async fn read_document(
        &self,
        session_id: &str,
        target: &hypr_loofah_protocol::DocumentTarget,
    ) -> Result<hypr_loofah_protocol::DocumentSnapshot, ApplicationError> {
        self.validate_document(session_id, target)?;
        self.store
            .read_recovered_document_snapshot(
                self.document_operation_root().as_deref(),
                session_id,
                target,
            )
            .await
            .map_err(|cause| match cause {
                hypr_vault_write::StoreError::Conflict(ref reason)
                    if reason == "document is a legacy summary; use the summary operation" =>
                {
                    error(
                        ErrorCode::Validation,
                        "Use the summary operation for legacy summaries",
                    )
                }
                cause => storage_error(cause, false),
            })?
            .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
    }

    pub async fn save_document(
        &self,
        session_id: &str,
        target: &hypr_loofah_protocol::DocumentTarget,
        input: hypr_loofah_protocol::SaveDocumentRequest,
    ) -> Result<hypr_loofah_protocol::DocumentSaveOutcome, ApplicationError> {
        self.save_document_body(session_id, target, input, false)
            .await
    }

    pub async fn save_generated_document_body(
        &self,
        session_id: &str,
        target: &hypr_loofah_protocol::DocumentTarget,
        input: hypr_loofah_protocol::SaveDocumentRequest,
    ) -> Result<hypr_loofah_protocol::DocumentSaveOutcome, ApplicationError> {
        self.save_document_body(session_id, target, input, true)
            .await
    }

    pub async fn save_generated_summary(
        &self,
        session_id: &str,
        input: hypr_loofah_protocol::SaveDocumentRequest,
        suggestions: Option<Vec<hypr_vault_write::ScoredTagSuggestion>>,
    ) -> Result<hypr_loofah_protocol::DocumentSaveOutcome, ApplicationError> {
        self.validate_document(session_id, &hypr_loofah_protocol::DocumentTarget::Summary)?;
        if input.expected_revision.is_empty() || input.expected_revision.len() > 128 {
            return Err(error(
                ErrorCode::Validation,
                "Expected summary revision is required",
            ));
        }
        if input.markdown.len() > self.store.content_limits().note_write_bytes {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Generated summary is too large",
            ));
        }
        let root = self.document_operation_root().ok_or_else(|| {
            error(
                ErrorCode::Unavailable,
                "Operation storage is not configured",
            )
        })?;
        let policy = self.settings.read_current().await.map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "Processing settings could not be read",
            )
        })?;
        self.store
            .save_generated_summary_with_suggestions_if_revision(
                &root,
                session_id,
                &input.markdown,
                &input.expected_revision,
                suggestions,
                policy.auto_apply_high_confidence_tags,
            )
            .await
            .map_err(|cause| storage_error(cause, true))
    }

    async fn save_document_body(
        &self,
        session_id: &str,
        target: &hypr_loofah_protocol::DocumentTarget,
        input: hypr_loofah_protocol::SaveDocumentRequest,
        generated: bool,
    ) -> Result<hypr_loofah_protocol::DocumentSaveOutcome, ApplicationError> {
        self.validate_document(session_id, target)?;
        if input.expected_revision.is_empty() || input.expected_revision.len() > 128 {
            return Err(error(
                ErrorCode::Validation,
                "Expected document revision is required",
            ));
        }
        if input.markdown.len() > self.store.content_limits().note_write_bytes {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Document content is too large",
            ));
        }
        let current = self.read_document(session_id, target).await?;
        if matches!(
            target,
            hypr_loofah_protocol::DocumentTarget::TemplateOutput { .. }
        ) && current.document.is_none()
        {
            return Err(error(ErrorCode::NotFound, "Template output not found"));
        }
        let operations = self.operations.as_ref().ok_or_else(|| {
            error(
                ErrorCode::Unavailable,
                "Operation storage is not configured",
            )
        })?;
        let root = operations.root.join("document-saves");
        let result = if generated {
            self.store
                .save_generated_document_if_revision(
                    &root,
                    session_id,
                    target,
                    &input.markdown,
                    &input.expected_revision,
                )
                .await
        } else {
            self.store
                .save_document_if_revision(
                    &root,
                    session_id,
                    target,
                    &input.markdown,
                    &input.expected_revision,
                )
                .await
        };
        result.map_err(|cause| storage_error(cause, true))
    }

    fn validate_document(
        &self,
        session_id: &str,
        target: &hypr_loofah_protocol::DocumentTarget,
    ) -> Result<(), ApplicationError> {
        self.validate(session_id)?;
        if let hypr_loofah_protocol::DocumentTarget::TemplateOutput { document_id } = target {
            hypr_vault_read::paths::validate_session_id(document_id)
                .map_err(|_| error(ErrorCode::Validation, "Invalid document identifier"))?;
        }
        Ok(())
    }

    async fn require_session(&self, session_id: &str) -> Result<(), ApplicationError> {
        self.validate(session_id)?;
        self.store
            .read_meta(session_id)
            .await
            .map_err(|cause| storage_error(cause, false))?
            .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))?;
        Ok(())
    }

    pub async fn session_documents(
        &self,
        session_id: &str,
    ) -> Result<Vec<EnhancedDoc>, ApplicationError> {
        self.validate(session_id)?;
        self.store
            .read_recovered_session_documents(self.document_operation_root().as_deref(), session_id)
            .await
            .map_err(|cause| storage_error(cause, false))?
            .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
    }

    pub async fn session_transcripts(
        &self,
        session_id: &str,
    ) -> Result<Vec<TranscriptWithData>, ApplicationError> {
        self.require_session(session_id).await?;
        self.ensure_recording_publication_ready(session_id)?;
        self.store
            .session_transcripts(session_id)
            .await
            .map_err(|cause| storage_error(cause, false))
    }

    pub async fn session_tasks(&self, session_id: &str) -> Result<Vec<TaskItem>, ApplicationError> {
        self.validate(session_id)?;
        self.store
            .read_recovered_session_tasks(self.document_operation_root().as_deref(), session_id)
            .await
            .map_err(|cause| storage_error(cause, false))?
            .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
    }

    pub async fn people(&self) -> Result<Vec<PersonItem>, ApplicationError> {
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is not ready"))?;
        self.store
            .list_people()
            .await
            .map_err(|cause| storage_error(cause, false))
    }

    pub async fn tags(&self) -> Result<Vec<TagItem>, ApplicationError> {
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is not ready"))?;
        self.store
            .list_tags()
            .await
            .map_err(|cause| storage_error(cause, false))
    }
}

#[cfg(test)]
mod template_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_vault_write::{SessionStore, TaskInput};
    use serde_json::json;
    use std::sync::Arc;

    fn context(path: &std::path::Path) -> VaultContext {
        VaultContext::new(
            "test".into(),
            Arc::new(SessionStore::new(path.into())),
            StartupState::new(Some(path)),
            Arc::new(ConfigState::load_or_default(path)),
            Arc::new(SearchService::default()),
        )
    }

    #[tokio::test]
    async fn content_reads_preserve_canonical_models_and_ignore_unknown_attachments() {
        let vault = tempfile::tempdir().unwrap();
        let context = context(vault.path());
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Meeting","created_at":"2026-11-05T10:00:00Z","tags":[]}),
        )
        .unwrap();
        context
            .store
            .create_note_session(&meta, "My note")
            .await
            .unwrap();
        context.store.ensure_summary("note").await.unwrap();
        context
            .store
            .update_summary("note", "Summary content", Some(""))
            .await
            .unwrap();
        context
            .store
            .write_enhanced_doc(&EnhancedDoc {
                id: "template-result".into(),
                session_id: "note".into(),
                kind: "template_output".into(),
                title: "Output".into(),
                template_id: "template".into(),
                sort_order: 2,
                markdown: "Template content".into(),
            })
            .await
            .unwrap();
        let transcript: TranscriptWithData = serde_json::from_value(json!({"id":"transcript","session_id":"note","created_at":"2026-11-05T10:00:00Z","words":[{"text":"Hello","start_ms":0,"end_ms":250,"channel":0}]})).unwrap();
        context
            .store
            .write_transcript("note", transcript.clone())
            .await
            .unwrap();
        context
            .store
            .replace_tasks(
                "session_raw_note",
                "note",
                vec![TaskInput {
                    id: "task".into(),
                    source_order: 0,
                    status: "todo".into(),
                    text: "Do this".into(),
                    body: json!([]),
                    due_at: String::new(),
                }],
            )
            .await
            .unwrap();
        let person = context.store.ensure_person("Alice").await.unwrap();
        let tag = context.store.ensure_tag("Project").await.unwrap();
        let unknown = vault.path().join("sessions/note/unknown.md");
        std::fs::write(&unknown, "foreign content").unwrap();
        std::fs::write(
            vault.path().join("sessions/note/.hidden.md"),
            "hidden content",
        )
        .unwrap();
        let documents = context.session_documents("note").await.unwrap();
        assert_eq!(documents.len(), 2);
        assert_eq!(documents[0].markdown, "Summary content");
        assert_eq!(documents[1].markdown, "Template content");
        assert_eq!(
            context.session_transcripts("note").await.unwrap(),
            hypr_vault_read::transcript::read_transcript_json(vault.path(), "note")
                .unwrap()
                .transcripts
        );
        assert_eq!(
            context.session_transcripts("note").await.unwrap()[0].words,
            transcript.words
        );
        let tasks = context.session_tasks("note").await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, "Do this");
        assert_eq!(context.people().await.unwrap(), vec![person]);
        assert_eq!(context.tags().await.unwrap(), vec![tag]);
        assert_eq!(std::fs::read_to_string(unknown).unwrap(), "foreign content");
    }

    #[tokio::test]
    async fn missing_invalid_and_damaged_content_never_looks_like_an_empty_success() {
        let vault = tempfile::tempdir().unwrap();
        let context = context(vault.path());
        assert_eq!(
            context
                .session_transcripts("../private")
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        assert_eq!(
            context.session_documents("missing").await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert_eq!(
            context.session_tasks("missing").await.unwrap_err().code,
            ErrorCode::NotFound
        );
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Meeting","created_at":"2026-11-05T10:00:00Z","tags":[]}),
        )
        .unwrap();
        context
            .store
            .create_note_session(&meta, "Note")
            .await
            .unwrap();
        assert!(
            context
                .session_transcripts("note")
                .await
                .unwrap()
                .is_empty()
        );
        assert!(context.session_tasks("note").await.unwrap().is_empty());
        for name in ["transcript.json", "tasks.json"] {
            std::fs::write(vault.path().join("sessions/note").join(name), "damaged").unwrap();
        }
        assert_eq!(
            context.session_transcripts("note").await.unwrap_err().code,
            ErrorCode::Internal
        );
        assert_eq!(
            context.session_tasks("note").await.unwrap_err().code,
            ErrorCode::Internal
        );
        context.store.set_startup_pending(true);
        assert_eq!(
            context.people().await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        assert_eq!(
            context.tags().await.unwrap_err().code,
            ErrorCode::Unavailable
        );
    }
}
