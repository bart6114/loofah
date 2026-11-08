use crate::{Error, Result, local_owner::LocalGateway, output};
use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, LocalCliOperation, LocalCliProgress, LocalCliReply,
};
use std::{path::Path, sync::Arc};

pub async fn run(vault: &Path, id: &str, json: bool) -> Result<()> {
    let response = LocalGateway::new(vault.to_owned(), crate::vault::cache_for_vault(vault)?)
        .execute_with_upload(
            LocalCliOperation::Transcribe {
                session_id: id.into(),
            },
            None,
            progress(),
        )
        .await?;
    let LocalCliReply::Transcribed {
        session_id,
        transcript_id,
        word_count,
    } = response
        .response
        .result
        .map_err(|cause| media_error("transcribe audio", cause))?
    else {
        return Err(crate::profiles::unexpected());
    };
    output::track_write(&session_id)?;
    output::emit(&if json {
        output::json(
            "transcribe",
            &serde_json::json!({ "id": session_id, "transcript": { "id": transcript_id, "words": word_count } }),
            None,
        )?
    } else {
        transcript_id
    });
    Ok(())
}

pub(super) fn progress() -> Arc<dyn Fn(LocalCliProgress) + Send + Sync> {
    Arc::new(|event| {
        if event.total == Some(100) {
            eprintln!("{}... {}%", event.phase, event.processed);
        }
    })
}
pub(super) fn media_error(action: &'static str, cause: ApplicationError) -> Error {
    match cause.code {
        ErrorCode::NotFound => Error::NotFound(cause.message.trim_end_matches(" not found").into()),
        ErrorCode::Validation => Error::operation(action, cause.message),
        _ => Error::from(cause),
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn historical_transcripts_keep_provenance_and_speakers_when_read_and_exported() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("sessions/s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("transcript.json");
        let original = serde_json::json!({"transcripts": [{
            "id": "t1", "session_id": "s1", "words": [{
                "text": "Historical words", "start_ms": 0.0, "end_ms": 1000.0, "channel": 0.0,
                "speaker": "Alice", "metadata": {"model": "am-parakeet-v3", "arch": "ArgmaxSDK"}
            }]
        }]})
        .to_string();
        std::fs::write(&path, &original).unwrap();
        let file = hypr_vault_read::transcript::read_transcript_json(dir.path(), "s1").unwrap();
        let word = &file.transcripts[0].words[0];
        assert_eq!(word.metadata.as_ref().unwrap()["model"], "am-parakeet-v3");
        std::fs::write(
    session_dir.join("_meta.json"),
    r#"{"id":"s1","title":"Historical meeting","created_at":"2026-03-20T00:00:00Z","tags":[]}"#,
).unwrap();
        let exported = hypr_agent_access::get_meeting_export(dir.path(), "s1".into())
            .await
            .unwrap();
        let exported = serde_json::to_value(exported).unwrap();
        let exported_word = &exported["transcripts"][0]["words"][0];
        assert_eq!(exported_word["text"], "Historical words");
        assert_eq!(exported_word["speaker"], "Alice");
        assert_eq!(exported_word["metadata"]["model"], "am-parakeet-v3");
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }
}
