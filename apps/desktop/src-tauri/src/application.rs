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
pub async fn application_delete_recording<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    session_id: String,
    input: hypr_loofah_protocol::DeleteRecordingRequest,
) -> Result<bool, ApplicationError> {
    context(&app)?.delete_recording(&session_id, input).await
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

#[derive(Clone, serde::Serialize, serde::Deserialize, specta::Type, tauri_specta::Event)]
pub struct ApplicationChanged {
    pub event: hypr_loofah_protocol::ChangeEvent,
}

pub fn publish<R: tauri::Runtime>(app: &tauri::AppHandle<R>, change: hypr_loofah_protocol::Change) {
    use tauri_specta::Event;
    if let Some(context) = app.try_state::<Arc<VaultContext>>() {
        let event = context.events.publish(change);
        if let Err(error) = (ApplicationChanged { event }).emit(app) {
            tracing::warn!(%error, "failed to emit application change");
        }
    }
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
