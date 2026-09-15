use std::sync::Arc;

use tauri::{AppHandle, Manager};

use hypr_fs_format::TranscriptWithData;

use super::{
    EnhancedDoc, EnhancedDocPatch, PersonItem, RebuildReport, SessionListHeader, SessionMeta,
    SessionMetaPatch, SessionRecord, SessionStore, TagItem, TaskInput, TaskItem, TranscriptDelta,
    VaultStats,
};
use crate::related_tags::RelatedTagQueue;

/// Every command below is a thin wrapper: fetch the managed store, call the matching
/// `SessionStore` method, map `StoreError` to `String` for the IPC boundary. `SessionStore` is
/// `.manage()`d in `lib.rs`'s `setup()`; `try_state` (not `state`, which panics if unmanaged)
/// keeps a startup ordering mistake from crashing the whole IPC handler instead of surfacing a
/// normal `Err` the frontend already has to handle.
fn store<R: tauri::Runtime>(
    app: &AppHandle<R>,
) -> Result<tauri::State<'_, Arc<SessionStore>>, String> {
    app.try_state::<Arc<SessionStore>>()
        .ok_or_else(|| "session store is not initialized".to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_write_meta<R: tauri::Runtime>(
    app: AppHandle<R>,
    meta: SessionMeta,
) -> Result<(), String> {
    store(&app)?
        .write_meta(&meta)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_update_meta<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    patch: SessionMetaPatch,
) -> Result<(), String> {
    store(&app)?
        .update_meta(&session_id, patch)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_accept_tag_suggestion<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    name: String,
) -> Result<bool, String> {
    let store = store(&app)?;
    let accepted = store
        .accept_tag_suggestion(&session_id, &name)
        .await
        .map_err(|error| error.to_string())?;
    if accepted {
        if let Err(error) = store.ensure_tag(&name).await {
            tracing::warn!(tag = %name, %error, "related tags: registry sync failed");
        }
    }
    Ok(accepted)
}

#[tauri::command]
#[specta::specta]
pub async fn session_dismiss_tag_suggestion<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    name: String,
) -> Result<bool, String> {
    store(&app)?
        .dismiss_tag_suggestion(&session_id, &name)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_write_note<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    markdown: String,
) -> Result<(), String> {
    store(&app)?
        .write_note(&session_id, &markdown)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(queue) = app.try_state::<RelatedTagQueue>() {
        queue.note_changed(session_id);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn session_read_note<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<Option<String>, String> {
    store(&app)?
        .read_note(&session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_write_enhanced_doc<R: tauri::Runtime>(
    app: AppHandle<R>,
    doc: EnhancedDoc,
) -> Result<(), String> {
    let session_id = doc.session_id.clone();
    store(&app)?
        .write_enhanced_doc(&doc)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(queue) = app.try_state::<RelatedTagQueue>() {
        queue.enqueue(session_id);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn session_update_enhanced_doc<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    doc_id: String,
    patch: EnhancedDocPatch,
) -> Result<(), String> {
    store(&app)?
        .update_enhanced_doc(&session_id, &doc_id, patch)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(queue) = app.try_state::<RelatedTagQueue>() {
        queue.enqueue(session_id);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn session_delete_enhanced_doc<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    doc_id: String,
) -> Result<(), String> {
    store(&app)?
        .delete_enhanced_doc(&session_id, &doc_id)
        .await
        .map_err(|e| e.to_string())?;
    if let Some(queue) = app.try_state::<RelatedTagQueue>() {
        queue.enqueue(session_id);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn session_list_tasks<R: tauri::Runtime>(
    app: AppHandle<R>,
    source_type: String,
    source_id: String,
) -> Result<Vec<TaskItem>, String> {
    store(&app)?
        .list_tasks(&source_type, &source_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_replace_tasks<R: tauri::Runtime>(
    app: AppHandle<R>,
    source_type: String,
    source_id: String,
    tasks: Vec<TaskInput>,
) -> Result<(), String> {
    store(&app)?
        .replace_tasks(&source_type, &source_id, tasks)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_remove_tasks<R: tauri::Runtime>(
    app: AppHandle<R>,
    source_type: String,
    source_id: String,
    task_ids: Vec<String>,
) -> Result<(), String> {
    store(&app)?
        .remove_tasks(&source_type, &source_id, task_ids)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_move_tasks<R: tauri::Runtime>(
    app: AppHandle<R>,
    task_ids: Vec<String>,
    source_type: String,
    source_id: String,
    insertion_order: i32,
) -> Result<(), String> {
    store(&app)?
        .move_tasks(task_ids, &source_type, &source_id, insertion_order)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn people_list<R: tauri::Runtime>(app: AppHandle<R>) -> Result<Vec<PersonItem>, String> {
    store(&app)?.list_people().await.map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn people_ensure<R: tauri::Runtime>(
    app: AppHandle<R>,
    name: String,
) -> Result<PersonItem, String> {
    store(&app)?
        .ensure_person(&name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn tags_list<R: tauri::Runtime>(app: AppHandle<R>) -> Result<Vec<TagItem>, String> {
    store(&app)?.list_tags().await.map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn tags_ensure<R: tauri::Runtime>(
    app: AppHandle<R>,
    name: String,
) -> Result<TagItem, String> {
    store(&app)?
        .ensure_tag(&name)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_append_transcript<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    delta: TranscriptDelta,
) -> Result<(), String> {
    store(&app)?
        .append_transcript(&session_id, delta)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_flush_transcript<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<(), String> {
    store(&app)?
        .flush_transcript(&session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_write_transcript<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    transcript: TranscriptWithData,
) -> Result<(), String> {
    store(&app)?
        .write_transcript(&session_id, transcript)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_assign_transcript_speaker<R: tauri::Runtime>(
    app: AppHandle<R>,
    transcript_id: String,
    channel: i32,
    speaker_index: Option<i32>,
    speaker_label: String,
    anchor_word_id: String,
) -> Result<(), String> {
    store(&app)?
        .assign_transcript_speaker(
            &transcript_id,
            channel,
            speaker_index,
            &speaker_label,
            &anchor_word_id,
        )
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_replace_transcripts<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    transcript: TranscriptWithData,
) -> Result<(), String> {
    store(&app)?
        .replace_session_transcripts(&session_id, transcript)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_delete<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<(), String> {
    store(&app)?
        .delete_session(&session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_restore<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<bool, String> {
    store(&app)?
        .restore_session(&session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_rebuild_index<R: tauri::Runtime>(
    app: AppHandle<R>,
) -> Result<RebuildReport, String> {
    store(&app)?
        .rebuild_index()
        .await
        .map_err(|e| e.to_string())
}

// -- index queries (Phase E1): synchronous reads of the in-memory vault index; the
// frontend pairs them with the coalesced `index-changed` event to replace the SQL
// live queries. Semantics per command live on the matching `SessionStore` method.

#[tauri::command]
#[specta::specta]
pub async fn session_get<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<Option<SessionRecord>, String> {
    Ok(store(&app)?.session_get(&session_id))
}

#[tauri::command]
#[specta::specta]
pub async fn session_list_headers<R: tauri::Runtime>(
    app: AppHandle<R>,
) -> Result<Vec<SessionListHeader>, String> {
    Ok(store(&app)?.session_list_headers())
}

#[tauri::command]
#[specta::specta]
pub async fn vault_stats<R: tauri::Runtime>(app: AppHandle<R>) -> Result<VaultStats, String> {
    store(&app)?.vault_stats().await.map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_ids<R: tauri::Runtime>(app: AppHandle<R>) -> Result<Vec<String>, String> {
    Ok(store(&app)?.session_ids())
}

#[tauri::command]
#[specta::specta]
pub async fn session_is_empty<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<bool, String> {
    store(&app)?
        .session_is_empty(&session_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_has_transcript<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<bool, String> {
    Ok(store(&app)?.session_has_transcript(&session_id))
}

#[tauri::command]
#[specta::specta]
pub async fn session_enhanced_docs<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<Vec<EnhancedDoc>, String> {
    Ok(store(&app)?.session_enhanced_docs(&session_id))
}

#[tauri::command]
#[specta::specta]
pub async fn enhanced_doc_get<R: tauri::Runtime>(
    app: AppHandle<R>,
    doc_id: String,
) -> Result<Option<EnhancedDoc>, String> {
    Ok(store(&app)?.enhanced_doc_get(&doc_id))
}

#[tauri::command]
#[specta::specta]
pub async fn session_transcripts<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<Vec<TranscriptWithData>, String> {
    store(&app)?
        .session_transcripts(&session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn transcript_get<R: tauri::Runtime>(
    app: AppHandle<R>,
    transcript_id: String,
) -> Result<Option<TranscriptWithData>, String> {
    store(&app)?
        .transcript_get(&transcript_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_find_by_tracking_id<R: tauri::Runtime>(
    app: AppHandle<R>,
    tracking_id: String,
) -> Result<Option<SessionMeta>, String> {
    Ok(store(&app)?.session_find_by_tracking_id(&tracking_id))
}

/// Reserves the session's directory for an imminent recording and returns its
/// absolute path -- the stable value the pre-start hook receives. Paired with
/// `session_release_recording_prepare` on start failure; a successful start's
/// lease is cleared by the `Stopped` capture lifecycle.
#[tauri::command]
#[specta::specta]
pub async fn session_prepare_recording<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<String, String> {
    let store = store(&app)?;
    let relative = store
        .prepare_recording(&session_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(store
        .vault_base()
        .join(relative)
        .to_string_lossy()
        .into_owned())
}

#[tauri::command]
#[specta::specta]
pub async fn session_release_recording_prepare<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<(), String> {
    store(&app)?
        .release_recording_prepare(&session_id)
        .await
        .map_err(|e| e.to_string())
}

/// User-invoked "rename folder to match title": renames the session's physical
/// directory to the readable name derived from its current title. Refused while the
/// session holds a recording path lease; returns the resulting directory basename.
#[tauri::command]
#[specta::specta]
pub async fn session_rename_dir_to_title<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<String, String> {
    store(&app)?
        .rename_session_dir_to_title(&session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_store_audio<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    source_path: String,
) -> Result<String, String> {
    store(&app)?
        .store_audio(&session_id, &source_path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_list_audio<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
) -> Result<Vec<String>, String> {
    store(&app)?
        .list_audio(&session_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn session_delete_audio<R: tauri::Runtime>(
    app: AppHandle<R>,
    session_id: String,
    filename: String,
) -> Result<(), String> {
    store(&app)?
        .delete_audio(&session_id, &filename)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str) -> SessionMeta {
        SessionMeta {
            id: id.to_string(),
            title: "Note".to_string(),
            started_at: None,
            ended_at: None,
            created_at: "2026-08-27T00:00:00Z".to_string(),
            tags: Vec::new(),
            tag_suggestions: None,
            tracking_id: None,
            folder: None,
            author: None,
            skill: None,
            extra: Default::default(),
        }
    }

    #[tokio::test]
    async fn writing_a_note_schedules_debounced_related_tag_analysis() {
        let vault = tempfile::tempdir().unwrap();
        let store = Arc::new(super::super::new_test_store(vault.path().to_path_buf()).await);
        store.write_meta(&meta("s1")).await.unwrap();
        let queue = RelatedTagQueue::new_test();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(store);
        app.manage(queue.clone());

        session_write_note(
            app.handle().clone(),
            "s1".to_string(),
            "Atlas rollout note".to_string(),
        )
        .await
        .unwrap();

        assert!(queue.has_debounced_change("s1"));
    }
}
