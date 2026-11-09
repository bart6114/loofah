use super::*;
use std::path::Path;

pub(super) fn load(root: &Path, vault: &str) -> Result<Vec<Record>, ApplicationError> {
    ledger::ensure_directory(root)?;
    let mut records = Vec::new();
    let mut bytes = 0usize;
    let mut count = 0;
    for entry in std::fs::read_dir(root).map_err(review)? {
        if count > 4096 {
            return Err(review("Local intent inventory exceeds its budget"));
        }
        let entry = entry.map_err(review)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| review("Unknown Local intent path"))?;
        if name == "history" {
            if !history::validate_layout(root, vault)? {
                return Err(review("Local history layout disappeared"));
            }
            continue;
        }
        count += 1;
        if count > 4096 {
            return Err(review("Local intent inventory exceeds its budget"));
        }
        uuid(&name)?;
        if !entry.file_type().map_err(review)?.is_dir() {
            return Err(review("Unsafe Local intent directory"));
        }
        let path = entry.path().join("intent.json");
        let raw = ledger::read(&path, 128 * 1024)?;
        bytes = bytes.saturating_add(raw.len());
        if bytes > 64 * 1024 * 1024 {
            return Err(review("Local intent inventory exceeds its byte budget"));
        }
        let mut record: Record = serde_json::from_slice(&raw).map_err(review)?;
        validate_record(&record)?;
        if !matches!(record.version, 1 | 2 | 3)
            || record.vault_id != vault
            || record.status.declaration.request_id != name
            || record.status.transcription_owner != TRANSCRIPTION_OWNER
            || record.status.summary_owner != SUMMARY_OWNER
        {
            return Err(review("Local intent identity changed"));
        }
        if unresolved(&record) {
            record.version = 2;
            record.status.state = LocalCaptureState::Review;
            record.status.message=Some("An earlier process did not confirm completion; retained output requires explicit recovery".into());
            persist(root, &record, false)?;
        }
        records.push(record);
    }
    Ok(records)
}
pub(super) fn persist(root: &Path, record: &Record, create: bool) -> Result<(), ApplicationError> {
    validate_record(record)?;
    let directory = root.join(&record.status.declaration.request_id);
    ledger::ensure_directory(&directory)?;
    let path = directory.join("intent.json");
    let bytes = ledger::encode_bounded_result(record, 128 * 1024)?;
    if create {
        return ledger::write_new(&path, &bytes);
    }
    if !ledger::check_file(&path)? {
        return Err(review("Original Local intent disappeared"));
    }
    hypr_storage::fs::atomic_write(&path, std::str::from_utf8(&bytes).map_err(review)?)
        .map_err(review)?;
    ledger::sync_parent(&path)
}

pub(super) fn validate_record(record: &Record) -> Result<(), ApplicationError> {
    validate(&record.status.declaration)?;
    if !matches!(record.version, 1 | 2 | 3)
        || record.status.transcription_owner != TRANSCRIPTION_OWNER
        || record.status.summary_owner != SUMMARY_OWNER
        || chrono::DateTime::parse_from_rfc3339(&record.session_created_at).is_err()
    {
        return Err(review("Invalid Local intent binding"));
    }
    if record.version < 3
        && (record.resolution.is_some() || record.status.state == LocalCaptureState::Closed)
    {
        return Err(review(
            "Resolution requires the version 3 Local intent schema",
        ));
    }
    if let Some(resolution) = &record.resolution {
        resolution::validate_resolution(resolution)?;
        if record.status.state != LocalCaptureState::Closed
            || !resolution::confirmed_drain(record)
            || record.cancel_requested
        {
            return Err(review("Closed processing has no confirmed original drain"));
        }
    } else if record.status.state == LocalCaptureState::Closed {
        return Err(review("Closed processing has no reviewed resolution"));
    }
    if record.version == 1
        && (record.cancel_requested
            || record.processing_drained
            || record.inference_drained
            || record.summary_dispatch_pending
            || record.summary_drained
            || matches!(
                record.status.state,
                LocalCaptureState::Cancelling | LocalCaptureState::Cancelled
            ))
    {
        return Err(review(
            "Cancellation requires the version 2 Local intent schema",
        ));
    }
    if record.processing_drained
        && ((record.inference_dispatched && !record.inference_drained)
            || record.summary_dispatch_pending
            || (record.status.summary_job_id.is_some() && !record.summary_drained))
    {
        return Err(review("Processing drain evidence is incomplete"));
    }
    if record.cancel_requested
        && !matches!(
            record.status.state,
            LocalCaptureState::Cancelling
                | LocalCaptureState::Cancelled
                | LocalCaptureState::Review
        )
    {
        return Err(review("Cancelled work cannot reenter processing"));
    }
    if record.status.state == LocalCaptureState::Cancelled
        && (!record.cancel_requested
            || !record.processing_drained
            || record.physical != Some(Ok(()))
            || !matches!(record.persistence, Some(Ok(_))))
    {
        return Err(review("Cancellation has no confirmed original drain"));
    }
    let declaration = &record.status.declaration;
    match (&declaration.summary, &record.summary) {
        (None, None) => {}
        (Some(wanted), Some(actual))
            if actual.session_id == declaration.session_id
                && actual.target == wanted.target
                && actual.title_policy == wanted.title_policy
                && actual.only_if_empty == !declaration.append
                && hypr_loofah_protocol::is_transcription_sha256(&actual.expected_revision) => {}
        _ => return Err(review("Original summary declaration changed")),
    }
    for id in [&record.status.transcript_id, &record.status.summary_job_id]
        .into_iter()
        .flatten()
    {
        uuid(id)?;
    }
    if let Some(source) = &record.source {
        if record.physical != Some(Ok(()))
            || !matches!(record.persistence, Some(Ok(_)))
            || !hypr_loofah_protocol::is_transcription_sha256(&source.recording_sha256)
            || !hypr_loofah_protocol::is_transcription_sha256(&source.transcript_revision)
        {
            return Err(review("Local source has no confirmed native completion"));
        }
    }
    if record.inference_dispatched
        && (record.source.is_none() || declaration.transcription.is_none())
    {
        return Err(review("Local inference has no original source"));
    }
    if let Some(result) = &record.result_sha256 {
        if !record.inference_dispatched || !hypr_loofah_protocol::is_transcription_sha256(result) {
            return Err(review("Invalid retained Local inference identity"));
        }
    }
    if let Some(revision) = &record.published_revision {
        if record.result_sha256.is_none()
            || !hypr_loofah_protocol::is_transcription_sha256(revision)
            || record.source.as_ref().is_none_or(|source| {
                !source.transcript_present || source.transcript_revision != *revision
            })
        {
            return Err(review("Invalid Local publication proof"));
        }
    }
    Ok(())
}
