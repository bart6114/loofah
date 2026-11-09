use super::*;

pub(crate) async fn ensure_stored_session_idle(
    root: PathBuf,
    vault: String,
    session: String,
    owner: Option<Arc<std::fs::File>>,
) -> Result<(), ApplicationError> {
    tokio::task::spawn_blocking(move || {
        let _owner = owner;
        if !ledger::existing_directory(&root)? {
            return Ok(());
        }
        let history = super::history::validate_layout(&root, &vault)?;
        let mut bytes = 0usize;
        let mut busy = false;
        for (count, entry) in std::fs::read_dir(&root).map_err(review)?.enumerate() {
            // History has its own bounded index and contains only terminal receipts.
            // Do not turn a per-session guard into an archive or vault content scan.
            if count >= 4096 + usize::from(history) {
                return Err(review("Local intent inventory exceeds its budget"));
            }
            let entry = entry.map_err(review)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| review("Unknown Local intent path"))?;
            if history && name == "history" {
                continue;
            }
            uuid(&name)?;
            if !entry.file_type().map_err(review)?.is_dir()
                || !ledger::existing_directory(&entry.path())?
            {
                return Err(review("Unsafe Local intent directory"));
            }
            let raw = ledger::read(&entry.path().join("intent.json"), 128 * 1024)?;
            bytes = bytes.saturating_add(raw.len());
            if bytes > 64 * 1024 * 1024 {
                return Err(review("Local intent inventory exceeds its byte budget"));
            }
            let record: Record = serde_json::from_slice(&raw).map_err(review)?;
            journal::validate_record(&record)?;
            if record.vault_id != vault || record.status.declaration.request_id != name {
                return Err(review("Local intent identity changed"));
            }
            busy |= record.status.declaration.session_id == session && unresolved(&record);
        }
        if busy {
            return Err(review("Local capture intent requires completion or review"));
        }
        Ok(())
    })
    .await
    .map_err(review)?
}

#[cfg(test)]
mod tests;
