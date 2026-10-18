use crate::SessionStore;
use hypr_vault_access::{Error, Result};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

pub fn arg<T: DeserializeOwned>(args: &Value, key: &str) -> Result<T> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null))
        .map_err(|_| Error::new("invalid_request", format!("invalid {key}")))
}

pub fn text<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("invalid_request", format!("missing {key}")))
}

pub fn is_query(operation: &str) -> bool {
    matches!(
        operation,
        "session_get"
            | "session_list_headers"
            | "session_ids"
            | "session_is_empty"
            | "session_has_transcript"
            | "session_enhanced_docs"
            | "enhanced_doc_get"
            | "session_transcript_metadata"
            | "session_transcripts"
            | "transcript_get"
            | "session_find_by_tracking_id"
            | "session_read_note"
            | "session_summary_get"
            | "session_list_tasks"
            | "people_list"
            | "tags_list"
            | "session_tag_context"
            | "vault_stats"
            | "vault_storage_stats"
            | "session_list_audio"
            | "search"
    )
}

pub async fn execute(store: &SessionStore, operation: &str, args: &Value) -> Result<Value> {
    fn store_error(error: crate::StoreError) -> Error {
        let code = if matches!(error, crate::StoreError::Conflict(_)) {
            "conflict"
        } else {
            "storage"
        };
        Error::new(code, error.to_string())
    }
    let session = || text(args, "session_id");
    macro_rules! output {
        ($expression:expr) => {
            serde_json::to_value($expression).map_err(Into::into)
        };
    }
    match operation {
        "session_prepare_recording" => {
            store
                .prepare_recording(session()?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_begin_audio_import" => {
            store
                .begin_audio_import(session()?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_finish_audio_import" => {
            store
                .finish_audio_import(session()?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_mark_recording_started" => {
            store
                .mark_recording_started(session()?, text(args, "at")?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_mark_recording_ended" => {
            store
                .mark_recording_ended(session()?, text(args, "at")?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_get" => output!(store.session_get(session()?)),
        "session_list_headers" => {
            let mut headers = store.session_list_headers();
            let after: Option<String> = arg(args, "after")?;
            let limit = args["limit"].as_u64().unwrap_or(200).clamp(1, 500) as usize;
            headers.sort_by(|a, b| a.id.cmp(&b.id));
            if let Some(after) = after {
                headers.retain(|entry| entry.id > after);
            }
            headers.truncate(limit);
            output!(headers)
        }
        "session_ids" => output!(store.session_ids()),
        "session_is_empty" => output!(
            store
                .session_is_empty(session()?)
                .await
                .map_err(store_error)?
        ),
        "session_has_transcript" => output!(store.session_has_transcript(session()?)),
        "session_enhanced_docs" => output!(store.session_enhanced_docs(session()?)),
        "enhanced_doc_get" => output!(store.enhanced_doc_get(text(args, "doc_id")?)),
        "session_transcript_metadata" => output!(store.session_transcript_metadata(session()?)),
        "session_transcripts" => output!(
            store
                .session_transcripts(session()?)
                .await
                .map_err(store_error)?
        ),
        "transcript_get" => output!(
            store
                .transcript_get(text(args, "transcript_id")?)
                .await
                .map_err(store_error)?
        ),
        "session_find_by_tracking_id" => {
            output!(store.session_find_by_tracking_id(text(args, "tracking_id")?))
        }
        "session_read_note" => output!(store.read_note(session()?).await.map_err(store_error)?),
        "session_summary_get" => output!(store.summary_get(session()?)),
        "session_list_tasks" => output!(
            store
                .list_tasks(text(args, "source_type")?, text(args, "source_id")?)
                .await
                .map_err(store_error)?
        ),
        "people_list" => output!(store.list_people().await.map_err(store_error)?),
        "tags_list" => output!(store.list_tags().await.map_err(store_error)?),
        "session_tag_context" => output!(
            store
                .session_tag_context(session()?)
                .await
                .map_err(store_error)?
        ),
        "vault_stats" => output!(store.vault_stats().await.map_err(store_error)?),
        "vault_storage_stats" => output!(
            store
                .vault_storage_stats(false)
                .await
                .map_err(store_error)?
        ),
        "session_list_audio" => {
            let filenames = ["audio.mp3", "audio.wav", "audio.ogg"]
                .into_iter()
                .filter(|name| {
                    store
                        .vault_base()
                        .join("sessions")
                        .join(session().unwrap_or(""))
                        .join(name)
                        .is_file()
                })
                .map(str::to_string)
                .collect::<Vec<_>>();
            output!(filenames)
        }
        "session_write_meta" => {
            store
                .write_meta(&arg(args, "meta")?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_update_meta" => {
            store
                .update_meta(session()?, arg(args, "patch")?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_write_note" => {
            store
                .write_note(session()?, text(args, "markdown")?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_save_note" => {
            store
                .save_note(
                    session()?,
                    text(args, "markdown")?,
                    arg::<Option<String>>(args, "title")?.as_deref(),
                )
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_ensure_summary" => output!(
            store
                .ensure_summary(session()?)
                .await
                .map_err(store_error)?
        ),
        "session_update_summary" => {
            store
                .update_summary_with_suggestions(
                    session()?,
                    text(args, "markdown")?,
                    arg::<Option<String>>(args, "expected_markdown")?.as_deref(),
                    args["reconcile_tasks"] == true,
                    arg(args, "suggested_tags")?,
                    args["auto_apply_high_confidence_tags"] == true,
                )
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_save_summary" => {
            store
                .save_summary(
                    session()?,
                    text(args, "markdown")?,
                    arg::<Option<String>>(args, "expected_markdown")?.as_deref(),
                    arg::<Option<String>>(args, "title")?.as_deref(),
                )
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_delete_summary" => {
            store
                .delete_summary(session()?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_write_enhanced_doc" => {
            let doc: crate::EnhancedDoc = arg(args, "doc")?;
            if doc.kind == "summary" {
                return Err(Error::new(
                    "invalid_request",
                    "use the session summary operation",
                ));
            }
            store.write_enhanced_doc(&doc).await.map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_update_enhanced_doc" | "session_save_enhanced_doc" => {
            let patch: crate::EnhancedDocPatch = arg(args, "patch")?;
            if patch.kind.as_deref() == Some("summary") {
                return Err(Error::new(
                    "invalid_request",
                    "use the session summary operation",
                ));
            }
            store
                .save_enhanced_doc_with_auto_apply(
                    session()?,
                    text(args, "doc_id")?,
                    patch,
                    arg::<Option<String>>(args, "title")?.as_deref(),
                    args["auto_apply_high_confidence_tags"] == true,
                )
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_delete_enhanced_doc" => {
            store
                .delete_enhanced_doc(session()?, text(args, "doc_id")?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_replace_tasks" => {
            store
                .replace_tasks(
                    text(args, "source_type")?,
                    text(args, "source_id")?,
                    arg(args, "tasks")?,
                )
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_remove_tasks" => {
            store
                .remove_tasks(
                    text(args, "source_type")?,
                    text(args, "source_id")?,
                    arg(args, "task_ids")?,
                )
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_move_tasks" => {
            store
                .move_tasks(
                    arg(args, "task_ids")?,
                    text(args, "source_type")?,
                    text(args, "source_id")?,
                    arg(args, "insertion_order")?,
                )
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "people_ensure" => output!(
            store
                .ensure_person(text(args, "name")?)
                .await
                .map_err(store_error)?
        ),
        "tags_ensure" => output!(
            store
                .ensure_tag(text(args, "name")?)
                .await
                .map_err(store_error)?
        ),
        "session_accept_tag_suggestion" => output!(
            store
                .accept_tag_suggestion(session()?, text(args, "name")?)
                .await
                .map_err(store_error)?
        ),
        "session_dismiss_tag_suggestion" => output!(
            store
                .dismiss_tag_suggestion(session()?, text(args, "name")?)
                .await
                .map_err(store_error)?
        ),
        "session_append_transcript" => {
            store
                .append_transcript(session()?, arg(args, "delta")?)
                .await
                .map_err(store_error)?;
            store
                .flush_transcript(session()?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_flush_transcript" => {
            store
                .flush_transcript(session()?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_finish_transcript" => {
            store
                .finish_transcript(session()?, text(args, "transcript_id")?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_write_transcript" => {
            store
                .write_transcript(session()?, arg(args, "transcript")?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_replace_transcripts" => {
            store
                .replace_session_transcripts(session()?, arg(args, "transcript")?)
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        "session_assign_transcript_speaker" => {
            store
                .assign_transcript_speaker(
                    text(args, "transcript_id")?,
                    arg(args, "channel")?,
                    arg(args, "speaker_index")?,
                    text(args, "speaker_label")?,
                    text(args, "anchor_word_id")?,
                )
                .await
                .map_err(store_error)?;
            Ok(Value::Null)
        }
        _ => Err(Error::new("invalid_request", "unsupported vault operation")),
    }
}

pub fn is_atomic(operation: &str) -> bool {
    matches!(
        operation,
        "session_prepare_recording"
            | "session_begin_audio_import"
            | "session_finish_audio_import"
            | "session_mark_recording_started"
            | "session_mark_recording_ended"
            | "people_ensure"
            | "tags_ensure"
            | "session_append_transcript"
            | "session_flush_transcript"
            | "session_finish_transcript"
            | "session_update_meta"
            | "session_accept_tag_suggestion"
            | "session_dismiss_tag_suggestion"
            | "session_ensure_summary"
            | "session_remove_tasks"
            | "session_move_tasks"
    )
}

pub fn fingerprint(user_id: &str, call: &hypr_vault_access::Call) -> Result<String> {
    Ok(hypr_vault_access::digest(&serde_json::to_vec(&json!([
        user_id, call
    ]))?))
}

#[derive(Clone)]
pub struct LocalVault(pub std::sync::Arc<SessionStore>);
impl hypr_vault_access::VaultAccess for LocalVault {
    async fn invoke(&self, call: hypr_vault_access::Call) -> Result<hypr_vault_access::Reply> {
        Ok(hypr_vault_access::Reply {
            data: execute(&self.0, &call.operation, &call.args).await?,
            versions: Default::default(),
        })
    }
}
