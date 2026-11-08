use super::*;

pub(super) const RESPONSE_LIMIT: usize = 16 * 1024;

pub(super) fn is_operation(operation: &RemoteOperation) -> bool {
    matches!(
        operation,
        RemoteOperation::SessionLifecycle { .. }
            | RemoteOperation::DeleteSession { .. }
            | RemoteOperation::RestoreSession { .. }
    )
}
pub(super) fn is_write(operation: &RemoteOperation) -> bool {
    matches!(
        operation,
        RemoteOperation::DeleteSession { .. } | RemoteOperation::RestoreSession { .. }
    )
}
pub(super) fn validate_operation(operation: &RemoteOperation) -> Result<()> {
    let vault = match operation {
        RemoteOperation::SessionLifecycle {
            vault_id,
            session_id,
        } => {
            validate_session_id(session_id).map_err(|_| invalid("Invalid session identifier"))?;
            vault_id
        }
        RemoteOperation::DeleteSession { vault_id, input } => {
            validate_delete_session_request(input).map_err(invalid)?;
            vault_id
        }
        RemoteOperation::RestoreSession { vault_id, input } => {
            validate_restore_session_request(input).map_err(invalid)?;
            vault_id
        }
        _ => return Ok(()),
    };
    if !uuid::Uuid::parse_str(vault).is_ok_and(|id| !id.is_nil() && id.to_string() == *vault) {
        return Err(invalid("A canonical vault UUID is required"));
    }
    Ok(())
}
fn malformed() -> ApplicationError {
    failure(
        ErrorCode::IncompatibleVersion,
        "Invalid session lifecycle response",
    )
}
fn snapshot(value: &SessionLifecycleSnapshot, session: &str) -> Result<()> {
    if value.session_id != session || !is_session_lifecycle_revision(&value.revision) {
        return Err(malformed());
    }
    Ok(())
}
pub(super) async fn decode(
    response: reqwest::Response,
    operation: &RemoteOperation,
    conflict: bool,
) -> Result<RemoteReply> {
    let raw: Value = json_body(response, RESPONSE_LIMIT).await?;
    if conflict && raw.get("code").is_some() {
        return Err(serde_json::from_value::<ApplicationError>(raw).map_err(|_| malformed())?);
    }
    match operation {
        RemoteOperation::SessionLifecycle { session_id, .. } if !conflict => {
            let value: SessionLifecycleSnapshot =
                serde_json::from_value(raw).map_err(|_| malformed())?;
            snapshot(&value, session_id)?;
            Ok(RemoteReply::SessionLifecycle(value))
        }
        RemoteOperation::DeleteSession { input, .. } => {
            let outcome: DeleteSessionOutcome =
                serde_json::from_value(raw).map_err(|_| malformed())?;
            match &outcome {
                DeleteSessionOutcome::Trashed {
                    trash_token,
                    revision,
                } if !conflict
                    && is_session_lifecycle_revision(trash_token)
                    && revision == &input.expected_revision => {}
                DeleteSessionOutcome::Conflict { snapshot: value } if conflict => {
                    snapshot(value, &input.session_id)?
                }
                DeleteSessionOutcome::NotFound if !conflict => {}
                _ => return Err(malformed()),
            }
            Ok(RemoteReply::SessionDelete(outcome))
        }
        RemoteOperation::RestoreSession { input, .. } => {
            let outcome: RestoreSessionOutcome =
                serde_json::from_value(raw).map_err(|_| malformed())?;
            match &outcome {
                RestoreSessionOutcome::Restored {
                    session_id,
                    revision,
                } if !conflict
                    && validate_session_id(session_id).is_ok()
                    && revision == &input.expected_revision => {}
                RestoreSessionOutcome::Conflict {
                    reason: RestoreSessionConflictReason::DestinationOccupied,
                    current_revision: None,
                } if conflict => {}
                RestoreSessionOutcome::Conflict {
                    reason: RestoreSessionConflictReason::TrashChanged,
                    current_revision: Some(revision),
                } if conflict && is_session_lifecycle_revision(revision) => {}
                RestoreSessionOutcome::NotFound if !conflict => {}
                _ => return Err(malformed()),
            }
            Ok(RemoteReply::SessionRestore(outcome))
        }
        _ => Err(malformed()),
    }
}
