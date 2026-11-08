use super::*;

pub(super) fn is_operation(operation: &RemoteOperation) -> bool {
    matches!(
        operation,
        RemoteOperation::SubmitTranscription { .. }
            | RemoteOperation::TranscriptionJobs { .. }
            | RemoteOperation::TranscriptionJob { .. }
            | RemoteOperation::TranscriptionJobResult { .. }
            | RemoteOperation::CancelTranscriptionJob { .. }
            | RemoteOperation::ResumeTranscriptionResult { .. }
    )
}

pub(super) fn is_write(operation: &RemoteOperation) -> bool {
    matches!(
        operation,
        RemoteOperation::SubmitTranscription { .. }
            | RemoteOperation::CancelTranscriptionJob { .. }
            | RemoteOperation::ResumeTranscriptionResult { .. }
    )
}

fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

pub(super) fn validate_operation(operation: &RemoteOperation) -> Result<()> {
    let vault = match operation {
        RemoteOperation::SubmitTranscription { vault_id, input } => {
            validate_transcription_job_request(input).map_err(invalid)?;
            vault_id
        }
        RemoteOperation::TranscriptionJobs { vault_id, input } => {
            if input.limit.is_some_and(|limit| !(1..=200).contains(&limit))
                || input.after.as_deref().is_some_and(|after| !uuid(after))
            {
                return Err(invalid("Invalid transcription page size or cursor"));
            }
            vault_id
        }
        RemoteOperation::TranscriptionJob { vault_id, job_id }
        | RemoteOperation::TranscriptionJobResult { vault_id, job_id }
        | RemoteOperation::CancelTranscriptionJob { vault_id, job_id }
        | RemoteOperation::ResumeTranscriptionResult { vault_id, job_id } => {
            if !uuid(job_id) {
                return Err(invalid("A canonical transcription job UUID is required"));
            }
            vault_id
        }
        _ => return Ok(()),
    };
    if !uuid(vault) {
        return Err(invalid("A canonical vault UUID is required"));
    }
    Ok(())
}

fn malformed() -> ApplicationError {
    failure(
        ErrorCode::IncompatibleVersion,
        "Invalid transcription response",
    )
}

fn text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn snapshot(job: &TranscriptionJobSnapshot) -> Result<()> {
    if !uuid(&job.id)
        || validate_session_id(&job.session_id).is_err()
        || !text(&job.created_at, 128)
        || !text(&job.updated_at, 128)
        || (job.state.is_terminal() && job.phase.is_some())
        || (matches!(
            job.state,
            TranscriptionJobState::Succeeded | TranscriptionJobState::Empty
        ) && !job.result_available)
        || (job.state == TranscriptionJobState::Succeeded && job.publication.is_none())
        || job.publication.as_ref().is_some_and(|publication| {
            job.state != TranscriptionJobState::Succeeded
                || !is_transcription_sha256(&publication.revision)
                || !uuid(&publication.transcript_id)
        })
    {
        return Err(malformed());
    }
    Ok(())
}

fn result(value: &TranscriptionJobResult, job: &str) -> Result<()> {
    let transcript = &value.transcript;
    if value.job_id != job
        || validate_session_id(&value.session_id).is_err()
        || value.session_id != transcript.session_id
        || !uuid(&transcript.id)
        || !uuid(&transcript.user_id)
        || !text(&transcript.created_at, 128)
        || !transcript.started_at.is_finite()
        || transcript.started_at < 0.0
        || transcript.ended_at.is_some()
        || transcript.words.len() > 250_000
        || transcript.speaker_hints.len() > 500_000
    {
        return Err(malformed());
    }
    let mut words = std::collections::HashSet::new();
    for word in &transcript.words {
        let Some(id) = &word.id else {
            return Err(malformed());
        };
        if !text(id, 512)
            || !words.insert(id)
            || word.text.len() > 64 * 1024
            || !word.start_ms.is_finite()
            || word.start_ms < 0.0
            || !word.end_ms.is_finite()
            || word.end_ms < word.start_ms
            || !matches!(word.channel, 0.0 | 1.0 | 2.0)
        {
            return Err(malformed());
        }
    }
    let mut hints = std::collections::HashSet::new();
    for hint in &transcript.speaker_hints {
        let Some(id) = &hint.id else {
            return Err(malformed());
        };
        if !text(id, 512)
            || !hints.insert(id)
            || !words.contains(&hint.word_id)
            || !text(&hint.hint_type, 256)
        {
            return Err(malformed());
        }
    }
    Ok(())
}

pub(super) async fn decode(
    response: reqwest::Response,
    operation: &RemoteOperation,
) -> Result<RemoteReply> {
    match operation {
        RemoteOperation::TranscriptionJobResult { job_id, .. } => {
            let value: Option<TranscriptionJobResult> =
                json_body(response, TRANSCRIPTION_RESULT_RESPONSE_LIMIT).await?;
            if let Some(value) = &value {
                result(value, job_id)?;
            }
            Ok(RemoteReply::TranscriptionResult(value))
        }
        RemoteOperation::TranscriptionJobs { input, .. } => {
            let page: TranscriptionJobPage = json_body(response, 1024 * 1024).await?;
            let limit = input.limit.unwrap_or(50) as usize;
            if page.jobs.len() > limit
                || page.jobs.windows(2).any(|pair| pair[0].id >= pair[1].id)
                || page
                    .jobs
                    .iter()
                    .any(|job| input.after.as_ref().is_some_and(|after| &job.id <= after))
                || page.next_cursor.as_ref().is_some_and(|cursor| {
                    page.jobs.len() != limit || page.jobs.last().is_none_or(|job| &job.id != cursor)
                })
            {
                return Err(malformed());
            }
            for job in &page.jobs {
                snapshot(job)?;
            }
            Ok(RemoteReply::TranscriptionJobs(page))
        }
        _ => {
            let job: TranscriptionJobSnapshot =
                json_body(response, TRANSCRIPTION_SNAPSHOT_LIMIT).await?;
            snapshot(&job)?;
            let matches = match operation {
                RemoteOperation::SubmitTranscription { input, .. } => {
                    job.session_id == input.session_id
                }
                RemoteOperation::TranscriptionJob { job_id, .. }
                | RemoteOperation::CancelTranscriptionJob { job_id, .. }
                | RemoteOperation::ResumeTranscriptionResult { job_id, .. } => &job.id == job_id,
                _ => false,
            };
            if !matches {
                return Err(malformed());
            }
            Ok(RemoteReply::TranscriptionJob(job))
        }
    }
}
