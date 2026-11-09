use super::*;
pub(super) fn is_operation(op: &RemoteOperation) -> bool {
    matches!(
        op,
        RemoteOperation::TranscriptEditor { .. }
            | RemoteOperation::AssignTranscriptSpeakers { .. }
            | RemoteOperation::EnsurePerson { .. }
    )
}
pub(super) fn is_write(op: &RemoteOperation) -> bool {
    matches!(
        op,
        RemoteOperation::AssignTranscriptSpeakers { .. } | RemoteOperation::EnsurePerson { .. }
    )
}
pub(super) fn validate_operation(op: &RemoteOperation) -> Result<()> {
    match op {
        RemoteOperation::TranscriptEditor { session_id, .. } => {
            validate_session_id(session_id).map_err(|_| invalid("Invalid transcript session"))
        }
        RemoteOperation::AssignTranscriptSpeakers {
            session_id, input, ..
        } => {
            validate_session_id(session_id).map_err(|_| invalid("Invalid transcript session"))?;
            validate_transcript_speakers(input).map_err(invalid)
        }
        RemoteOperation::EnsurePerson { input, .. } => {
            validate_person_name(&input.name).map_err(invalid)
        }
        _ => Ok(()),
    }
}
fn snapshot(value: &TranscriptEditorSnapshot, session: &str) -> Result<()> {
    let mut ids = std::collections::HashSet::new();
    if value.session_id != session
        || value.session_created_at.is_empty()
        || !is_transcription_sha256(&value.revision)
        || value
            .transcripts
            .iter()
            .any(|t| t.session_id != session || !ids.insert(&t.id))
    {
        return Err(failure(
            ErrorCode::IncompatibleVersion,
            "Transcript editor response changed its original session",
        ));
    }
    Ok(())
}
pub(super) async fn decode(
    response: reqwest::Response,
    op: &RemoteOperation,
    conflict: bool,
) -> Result<RemoteReply> {
    match op {
        RemoteOperation::TranscriptEditor { session_id, .. } => {
            let value: TranscriptEditorSnapshot =
                json_body(response, TRANSCRIPT_EDITOR_RESPONSE_LIMIT).await?;
            snapshot(&value, session_id)?;
            Ok(RemoteReply::TranscriptEditor(value))
        }
        RemoteOperation::AssignTranscriptSpeakers { session_id, .. } => {
            let raw: serde_json::Value =
                json_body(response, TRANSCRIPT_EDITOR_RESPONSE_LIMIT).await?;
            let value: TranscriptEditorSaveOutcome = match serde_json::from_value(raw.clone()) {
                Ok(value) => value,
                Err(_) => {
                    return Err(
                        serde_json::from_value::<ApplicationError>(raw).unwrap_or_else(|_| {
                            failure(
                                ErrorCode::IncompatibleVersion,
                                "Invalid transcript speaker result",
                            )
                        }),
                    );
                }
            };
            match (&value, conflict) {
                (TranscriptEditorSaveOutcome::Saved(value), false)
                | (TranscriptEditorSaveOutcome::Conflict(value), true) => {
                    snapshot(value, session_id)?
                }
                _ => {
                    return Err(failure(
                        ErrorCode::IncompatibleVersion,
                        "Unexpected transcript speaker outcome",
                    ));
                }
            };
            Ok(RemoteReply::TranscriptEditorSave(value))
        }
        RemoteOperation::EnsurePerson { input, .. } => {
            let person: PersonItem = json_body(response, 64 * 1024).await?;
            if person.id.is_empty()
                || person.id.len() > 4096
                || person.name.to_lowercase() != input.name.trim().to_lowercase()
            {
                return Err(failure(
                    ErrorCode::IncompatibleVersion,
                    "Person response differs from the requested name",
                ));
            }
            Ok(RemoteReply::Person(person))
        }
        _ => Err(invalid("Not a transcript editor operation")),
    }
}
