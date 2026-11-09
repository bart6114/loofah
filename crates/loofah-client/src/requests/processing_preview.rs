use super::*;

pub(crate) fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

pub(super) fn validate_operation(operation: &RemoteOperation) -> Result<()> {
    if let RemoteOperation::ProcessingJobPreview { vault_id, job_id } = operation
        && (!valid_uuid(vault_id) || !valid_uuid(job_id))
    {
        return Err(invalid("Invalid processing preview identity"));
    }
    Ok(())
}

pub(super) fn validate_reply(
    snapshot: &ProcessingJobPreview,
    vault_id: &str,
    job_id: &str,
) -> Result<()> {
    let job = &snapshot.job;
    let malformed = || {
        failure(
            ErrorCode::IncompatibleVersion,
            "Invalid processing preview response",
        )
    };
    if snapshot.vault_id != vault_id
        || job.id != job_id
        || validate_session_id(&job.session_id).is_err()
        || job.created_at.is_empty()
        || job.created_at.len() > 128
        || job.updated_at.is_empty()
        || job.updated_at.len() > 128
        || job.created_at.chars().any(char::is_control)
        || job.updated_at.chars().any(char::is_control)
    {
        return Err(malformed());
    }
    if let DocumentTarget::TemplateOutput { document_id } = &job.target
        && validate_session_id(document_id).is_err()
    {
        return Err(malformed());
    }
    if let Some(preview) = &snapshot.preview
        && (preview.markdown.len() > MAX_PROCESSING_PREVIEW_BYTES
            || preview.cursor.len() > 64
            || !preview
                .cursor
                .split_once(':')
                .is_some_and(|(epoch, sequence)| {
                    valid_uuid(epoch) && sequence.parse::<u64>().is_ok_and(|sequence| sequence > 0)
                }))
    {
        return Err(malformed());
    }
    Ok(())
}
