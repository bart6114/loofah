use std::sync::Arc;

use hypr_loofah_core::VaultContext;
use hypr_loofah_protocol::{
    ApplicationError, CreateSessionRequest, ErrorCode, NoteSaveOutcome, NoteSnapshot,
    SaveNoteRequest, SessionDocument, SessionPage,
};
use tauri::Manager;

fn context<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Arc<VaultContext>, ApplicationError> {
    app.try_state::<Arc<VaultContext>>()
        .map(|state| state.inner().clone())
        .ok_or_else(|| {
            hypr_loofah_client::failure(
                ErrorCode::Unavailable,
                "Vault startup is still in progress",
            )
        })
}

#[tauri::command]
#[specta::specta]
pub async fn application_upload_attachment_source<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    request_id: String,
    session_id: String,
    input: hypr_loofah_protocol::UploadAttachmentRequest,
    source: crate::attachment_upload::DeviceAttachmentSource,
) -> Result<hypr_loofah_protocol::FileMetadata, ApplicationError> {
    let context = context(&app)?;
    let source = app
        .state::<crate::attachment_upload::DeviceUploadIo>()
        .reader(window.label(), source, &input)?;
    let _admission = source.admission();
    context
        .upload_attachment("desktop".into(), request_id, &session_id, input, source)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_attachment_upload_status<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request_id: String,
    session_id: String,
    input: hypr_loofah_protocol::UploadAttachmentRequest,
) -> Result<hypr_loofah_protocol::AttachmentUploadStatus, ApplicationError> {
    context(&app)?
        .attachment_upload_status("desktop".into(), request_id, &session_id, input)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_delete_recording<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    input: hypr_loofah_protocol::DeleteRecordingRequest,
) -> Result<bool, ApplicationError> {
    context(&app)?.delete_recording(&session_id, input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_remove_attachment<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request_id: String,
    session_id: String,
    attachment_id: String,
    input: hypr_loofah_protocol::RemoveAttachmentRequest,
) -> Result<bool, ApplicationError> {
    context(&app)?
        .remove_attachment(
            "desktop".into(),
            request_id,
            &session_id,
            &attachment_id,
            input,
        )
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_attachment_metadata<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    attachment_id: String,
) -> Result<Option<hypr_loofah_protocol::FileMetadata>, ApplicationError> {
    context(&app)?
        .attachment_metadata(&session_id, &attachment_id)
        .await
}

#[tauri::command]
#[specta::specta]
pub fn application_sessions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    limit: u32,
    after: Option<String>,
) -> Result<SessionPage, ApplicationError> {
    context(&app)?.session_page(limit as usize, after.as_deref())
}

#[tauri::command]
#[specta::specta]
pub async fn application_delete_empty_session<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request_id: String,
    input: hypr_loofah_protocol::DeleteEmptySessionRequest,
) -> Result<hypr_loofah_protocol::DeleteEmptySessionOutcome, ApplicationError> {
    context(&app)?
        .delete_empty_session("desktop".into(), request_id, input)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_session_summaries<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input: hypr_loofah_protocol::SessionSummaryQuery,
) -> Result<Vec<hypr_loofah_protocol::SessionSummary>, ApplicationError> {
    context(&app)?.session_summaries(input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_session<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<SessionDocument, ApplicationError> {
    context(&app)?.session_document(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_create_session<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request_id: String,
    input: CreateSessionRequest,
) -> Result<SessionDocument, ApplicationError> {
    context(&app)?
        .create_session("desktop".into(), request_id, input)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_read_note<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<NoteSnapshot, ApplicationError> {
    context(&app)?.read_note(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_read_note_editor<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<hypr_loofah_protocol::NoteEditorSnapshot, ApplicationError> {
    context(&app)?.read_note_editor(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_save_note_editor<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    input: hypr_loofah_protocol::SaveNoteEditorRequest,
) -> Result<hypr_loofah_protocol::NoteEditorSaveOutcome, ApplicationError> {
    context(&app)?.save_note_editor(&session_id, input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_save_note<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    input: SaveNoteRequest,
) -> Result<NoteSaveOutcome, ApplicationError> {
    context(&app)?.save_note(&session_id, input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_search<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input: hypr_search_cache::SearchRequest,
) -> Result<hypr_search_cache::SearchResult, ApplicationError> {
    context(&app)?.search_sessions(input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_documents<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<Vec<hypr_loofah_protocol::EnhancedDoc>, ApplicationError> {
    context(&app)?.session_documents(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_attachments<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    limit: u32,
    after: Option<String>,
) -> Result<hypr_loofah_protocol::AttachmentPage, ApplicationError> {
    context(&app)?
        .attachment_page(&session_id, limit as usize, after.as_deref())
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_attachment_info<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    attachment_id: String,
) -> Result<Option<hypr_loofah_protocol::FileInfo>, ApplicationError> {
    context(&app)?
        .attachment_info(&session_id, &attachment_id)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_audio_metadata<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<Option<hypr_loofah_protocol::FileMetadata>, ApplicationError> {
    context(&app)?.audio_metadata(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_transcripts<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<Vec<hypr_loofah_protocol::TranscriptWithData>, ApplicationError> {
    context(&app)?.session_transcripts(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_tasks<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<Vec<hypr_loofah_protocol::TaskItem>, ApplicationError> {
    context(&app)?.session_tasks(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_people<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<hypr_loofah_protocol::PersonItem>, ApplicationError> {
    context(&app)?.people().await
}

#[tauri::command]
#[specta::specta]
pub async fn application_tags<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<Vec<hypr_loofah_protocol::TagItem>, ApplicationError> {
    context(&app)?.tags().await
}

#[tauri::command]
#[specta::specta]
pub async fn application_tag_suggestions<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input: hypr_loofah_protocol::TagSuggestionsRequest,
) -> Result<Vec<String>, ApplicationError> {
    context(&app)?.tag_suggestions(input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_ensure_tag<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input: hypr_loofah_protocol::EnsureTagRequest,
) -> Result<hypr_loofah_protocol::TagItem, ApplicationError> {
    context(&app)?.ensure_tag(input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_accept_tag_suggestion<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    input: hypr_loofah_protocol::TagSuggestionActionRequest,
) -> Result<hypr_loofah_protocol::MetadataSaveOutcome, ApplicationError> {
    context(&app)?
        .accept_tag_suggestion(&session_id, input)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_dismiss_tag_suggestion<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    input: hypr_loofah_protocol::TagSuggestionActionRequest,
) -> Result<hypr_loofah_protocol::MetadataSaveOutcome, ApplicationError> {
    context(&app)?
        .dismiss_tag_suggestion(&session_id, input)
        .await
}

#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct ApplicationChanged {
    pub event: hypr_loofah_protocol::ChangeEvent,
}

pub fn publish<R: tauri::Runtime>(app: &tauri::AppHandle<R>, change: hypr_loofah_protocol::Change) {
    if let Some(context) = app.try_state::<Arc<VaultContext>>() {
        context.events.publish(change);
    }
}

pub fn forward_changes<R: tauri::Runtime>(app: &tauri::AppHandle<R>, context: &Arc<VaultContext>) {
    use tauri_specta::Event;
    let mut settings_changes = context.settings.subscribe_processing();
    let settings_events = context.events.clone();
    let mut changes = context
        .events
        .subscribe_for(crate::processing::PRINCIPAL, None);
    let stopping = app.state::<crate::processing::ProcessingState>().stopping();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let event = tokio::select! {
                biased;
                _ = stopping.cancelled() => break,
                change = settings_changes.changed() => {
                    if change.is_err() { break; }
                    settings_events.publish(hypr_loofah_protocol::Change::ProcessingSettingsChanged);
                    continue;
                },
                event = changes.recv() => match event { Some(event) => event, None => break },
            };
            if let Err(error) = (ApplicationChanged { event }).emit(&app) {
                tracing::warn!(%error, "failed to emit application change");
            }
        }
    });
}

#[tauri::command]
#[specta::specta]
pub fn application_processing_available<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> bool {
    context(&app).is_ok_and(|context| context.processing_jobs().is_ok())
}

#[tauri::command]
#[specta::specta]
pub fn application_processing_policy<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<hypr_loofah_protocol::ProcessingPolicy, ApplicationError> {
    context(&app)?.processing_policy()
}

#[tauri::command]
#[specta::specta]
pub async fn application_processing_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<hypr_loofah_protocol::ProcessingSettingsSnapshot, ApplicationError> {
    context(&app)?.read_processing_settings().await
}

#[tauri::command]
#[specta::specta]
pub async fn application_save_processing_settings<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input: hypr_loofah_protocol::SaveProcessingSettingsRequest,
) -> Result<hypr_loofah_protocol::ProcessingSettingsSaveOutcome, ApplicationError> {
    context(&app)?.save_processing_settings(input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_submit_summary_job<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request_id: String,
    input: hypr_loofah_protocol::SummaryJobRequest,
) -> Result<hypr_loofah_protocol::ProcessingJobSnapshot, ApplicationError> {
    context(&app)?
        .submit_summary_job(crate::processing::PRINCIPAL.into(), request_id, input)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_processing_jobs<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input: hypr_loofah_protocol::ListProcessingJobsInput,
) -> Result<hypr_loofah_protocol::ProcessingJobPage, ApplicationError> {
    context(&app)?
        .list_processing_jobs(crate::processing::PRINCIPAL.into(), input)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_processing_job<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    job_id: String,
) -> Result<hypr_loofah_protocol::ProcessingJobSnapshot, ApplicationError> {
    context(&app)?
        .processing_job(crate::processing::PRINCIPAL.into(), job_id)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_processing_job_result<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    job_id: String,
) -> Result<Option<hypr_loofah_protocol::ProcessingJobResult>, ApplicationError> {
    context(&app)?
        .processing_job_result(crate::processing::PRINCIPAL.into(), job_id)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_cancel_processing_job<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    job_id: String,
) -> Result<hypr_loofah_protocol::ProcessingJobSnapshot, ApplicationError> {
    context(&app)?
        .cancel_processing_job(crate::processing::PRINCIPAL.into(), job_id)
        .await
}

#[tauri::command]
#[specta::specta]
pub fn application_scope<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
) -> Result<String, ApplicationError> {
    Ok(context(&app)?.events.vault_id())
}

#[tauri::command]
#[specta::specta]
pub async fn application_read_document<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    target: hypr_loofah_protocol::DocumentTarget,
) -> Result<hypr_loofah_protocol::DocumentSnapshot, ApplicationError> {
    context(&app)?.read_document(&session_id, &target).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_save_document<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    target: hypr_loofah_protocol::DocumentTarget,
    input: hypr_loofah_protocol::SaveDocumentRequest,
) -> Result<hypr_loofah_protocol::DocumentSaveOutcome, ApplicationError> {
    context(&app)?
        .save_document(&session_id, &target, input)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn application_agent_list<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input: hypr_loofah_protocol::ListMeetingsInput,
) -> Result<hypr_loofah_protocol::AgentPage, ApplicationError> {
    context(&app)?.agent_list(input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_agent_search<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    input: hypr_loofah_protocol::SearchMeetingsInput,
) -> Result<hypr_loofah_protocol::AgentSearchPage, ApplicationError> {
    context(&app)?.agent_search(input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_agent_meeting<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<hypr_loofah_protocol::AgentMeeting, ApplicationError> {
    context(&app)?.agent_meeting(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_agent_transcript<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<hypr_loofah_protocol::AgentTranscript, ApplicationError> {
    context(&app)?.agent_transcript(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_agent_export<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<hypr_loofah_protocol::AgentExport, ApplicationError> {
    context(&app)?.agent_export(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_agent_segments<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<Vec<hypr_loofah_protocol::RenderedTranscriptSegment>, ApplicationError> {
    context(&app)?.agent_segments(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_read_metadata<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<hypr_loofah_protocol::MetadataSnapshot, ApplicationError> {
    context(&app)?.read_metadata(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_save_metadata<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    input: hypr_loofah_protocol::SaveMetadataRequest,
) -> Result<hypr_loofah_protocol::MetadataSaveOutcome, ApplicationError> {
    context(&app)?.save_metadata(&session_id, input).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_tasks_snapshot<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
) -> Result<hypr_loofah_protocol::TasksSnapshot, ApplicationError> {
    context(&app)?.read_tasks_snapshot(&session_id).await
}

#[tauri::command]
#[specta::specta]
pub async fn application_replace_tasks<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    input: hypr_loofah_protocol::ReplaceTasksRequest,
) -> Result<hypr_loofah_protocol::TasksSaveOutcome, ApplicationError> {
    context(&app)?.replace_tasks(&session_id, input).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypr_loofah_core::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_loofah_protocol::{SessionSummaryOrder, SessionSummaryQuery};

    #[tokio::test]
    async fn local_cleanup_is_login_free_and_replay_preserves_a_restored_session() {
        let vault = tempfile::tempdir().unwrap();
        let operations = tempfile::tempdir().unwrap();
        let store = Arc::new(hypr_vault_write::SessionStore::new(vault.path().into()));
        let meta = serde_json::from_value(serde_json::json!({
            "id":"note", "title":"", "created_at":"2026-11-07T10:00:00Z", "tags":[]
        }))
        .unwrap();
        store.create_note_session(&meta, "").await.unwrap();
        let context = Arc::new(
            VaultContext::new(
                "local:test".into(),
                store.clone(),
                StartupState::new(Some(vault.path())),
                Arc::new(ConfigState::load_or_default(vault.path())),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(operations.path().join("creates")),
        );
        let app = tauri::test::mock_builder()
            .manage(context)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        let input = hypr_loofah_protocol::DeleteEmptySessionRequest {
            session_id: "note".into(),
        };
        let outcome = application_delete_empty_session(
            app.handle().clone(),
            request_id.clone(),
            input.clone(),
        )
        .await
        .unwrap();
        assert_eq!(
            outcome,
            hypr_loofah_protocol::DeleteEmptySessionOutcome::Deleted
        );
        assert!(store.restore_session("note").await.unwrap());
        store
            .write_note("note", "Restored local text")
            .await
            .unwrap();
        assert_eq!(
            application_delete_empty_session(app.handle().clone(), request_id, input)
                .await
                .unwrap(),
            hypr_loofah_protocol::DeleteEmptySessionOutcome::Deleted
        );
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/note/notes.md")).unwrap(),
            "Restored local text"
        );
    }

    #[tokio::test]
    async fn local_summary_queries_use_shared_headers_without_login() {
        let vault = tempfile::tempdir().unwrap();
        let store = Arc::new(hypr_vault_write::SessionStore::new(vault.path().into()));
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Local note","created_at":"2026-11-07T10:00:00Z","tags":[],"future":"PRIVATE META"})).unwrap();
        store
            .create_note_session(&meta, "PRIVATE BODY")
            .await
            .unwrap();
        let context = Arc::new(VaultContext::new(
            "local:test".into(),
            store.clone(),
            StartupState::new(Some(vault.path())),
            Arc::new(ConfigState::load_or_default(vault.path())),
            Arc::new(SearchService::default()),
        ));
        let app = tauri::test::mock_builder()
            .manage(context)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let query = SessionSummaryQuery::List {
            limit: 55,
            order: SessionSummaryOrder::NewestFirst,
            titled_only: false,
        };
        let rows = application_session_summaries(app.handle().clone(), query.clone())
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].title, "Local note");
        assert_eq!(
            serde_json::to_value(&rows[0])
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            3
        );
        std::fs::remove_file(vault.path().join("sessions/note/_meta.json")).unwrap();
        let rows = application_session_summaries(
            app.handle().clone(),
            SessionSummaryQuery::Ids {
                ids: vec!["note".into()],
            },
        )
        .await
        .unwrap();
        assert!(rows.is_empty());
        store.set_startup_pending(true);
        assert_eq!(
            application_session_summaries(app.handle().clone(), query)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
    }

    #[tokio::test]
    async fn local_tag_editing_uses_shared_checked_operations_without_login() {
        use hypr_loofah_protocol::{
            EnsureTagRequest, MetadataSaveOutcome, TagSuggestionActionRequest,
            TagSuggestionsRequest,
        };
        let vault = tempfile::tempdir().unwrap();
        let store = Arc::new(hypr_vault_write::SessionStore::new(vault.path().into()));
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Keep title","created_at":"2026-11-08T00:00:00Z","tags":["in-use"],"tag_suggestions":{"items":["import/audio","other"],"dismissed":[]}})).unwrap();
        store.create_note_session(&meta, "Keep note").await.unwrap();
        let context = Arc::new(VaultContext::new(
            "local:test".into(),
            store,
            hypr_loofah_core::startup::StartupState::new(Some(vault.path())),
            Arc::new(hypr_loofah_core::settings::ConfigState::load_or_default(
                vault.path(),
            )),
            Arc::new(hypr_loofah_core::search::SearchService::default()),
        ));
        let before = context.read_metadata("note").await.unwrap();
        let app = tauri::test::mock_builder()
            .manage(context)
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        assert_eq!(
            application_ensure_tag(
                app.handle().clone(),
                EnsureTagRequest {
                    name: " #IMPORT/manual ".into()
                }
            )
            .await
            .unwrap()
            .id,
            "import/manual"
        );
        assert_eq!(
            application_tag_suggestions(
                app.handle().clone(),
                TagSuggestionsRequest {
                    query: "".into(),
                    excluded: vec!["in-use".into()],
                    limit: 8
                }
            )
            .await
            .unwrap(),
            ["import/manual"]
        );
        let input = TagSuggestionActionRequest {
            name: "import/audio".into(),
            expected_revision: before.revision,
        };
        let MetadataSaveOutcome::Saved(saved) =
            application_accept_tag_suggestion(app.handle().clone(), "note".into(), input.clone())
                .await
                .unwrap()
        else {
            panic!("saved")
        };
        assert_eq!(saved.meta.tags, ["import/audio", "in-use"]);
        assert!(
            matches!(application_dismiss_tag_suggestion(app.handle().clone(),"note".into(),input).await.unwrap(),MetadataSaveOutcome::Conflict(current) if current==saved)
        );
        let MetadataSaveOutcome::Saved(dismissed) = application_dismiss_tag_suggestion(
            app.handle().clone(),
            "note".into(),
            TagSuggestionActionRequest {
                name: "other".into(),
                expected_revision: saved.revision,
            },
        )
        .await
        .unwrap() else {
            panic!("dismissed")
        };
        assert_eq!(dismissed.meta.tag_suggestions.unwrap().dismissed, ["other"]);
        assert_eq!(
            std::fs::read_to_string(vault.path().join("sessions/note/notes.md")).unwrap(),
            "Keep note"
        );
    }
}
