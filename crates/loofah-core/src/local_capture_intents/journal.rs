use super::*;
use std::path::Path;

pub(super) fn load(root: &Path, vault: &str) -> Result<Vec<Record>, ApplicationError> {
    ledger::ensure_directory(root)?;
    let mut records = Vec::new();
    let mut bytes = 0usize;
    for (count, entry) in std::fs::read_dir(root).map_err(review)?.enumerate() {
        if count >= 4096 {
            return Err(review("Local intent inventory exceeds its budget"));
        }
        let entry = entry.map_err(review)?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| review("Unknown Local intent path"))?;
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
        validate(&record.status.declaration)?;
        if record.version != 1
            || record.vault_id != vault
            || record.status.declaration.request_id != name
            || record.status.transcription_owner != TRANSCRIPTION_OWNER
            || record.status.summary_owner != SUMMARY_OWNER
        {
            return Err(review("Local intent identity changed"));
        }
        if unresolved(&record) {
            record.status.state = LocalCaptureState::Review;
            record.status.message=Some("An earlier process did not confirm completion; retained output requires explicit recovery".into());
            persist(root, &record, false)?;
        }
        records.push(record);
    }
    Ok(records)
}
pub(super) fn persist(root: &Path, record: &Record, create: bool) -> Result<(), ApplicationError> {
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
