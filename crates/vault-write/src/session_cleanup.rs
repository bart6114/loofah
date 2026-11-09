use std::{
    io::Read,
    path::{Component, Path, PathBuf},
};

use hypr_loofah_protocol::{DeleteEmptySessionOutcome, DeleteEmptySessionRequest};
use serde::{Deserialize, Serialize};

use crate::{SessionStore, StoreError, VaultSnapshotGuard, operation_io, paths};

const MAX_DIRECTORY_ENTRIES: usize = 4096;

#[derive(Clone, Debug)]
pub struct SessionCleanupKey {
    pub vault_id: String,
    pub namespace: String,
    pub request_id: uuid::Uuid,
}

#[derive(Debug)]
pub enum SessionCleanupError {
    Busy,
    Rejected(StoreError),
    Uncertain(StoreError),
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    vault_id: String,
    namespace: String,
    request_id: String,
    session_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    binding: Binding,
    state: State,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum State {
    Pending { trash: PathBuf, meta_sha256: String },
    Completed { outcome: DeleteEmptySessionOutcome },
    Uncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Checked,
    Intent,
    Renamed,
    Synced,
    Completed,
}

impl SessionStore {
    /// Replays completed decisions without touching a restored/recreated session. Only a new
    /// confirmed directory move (or proof of an interrupted move) publishes removal events.
    pub async fn delete_empty_session(
        &self,
        operations: &Path,
        documents: Option<&Path>,
        key: SessionCleanupKey,
        input: DeleteEmptySessionRequest,
    ) -> Result<DeleteEmptySessionOutcome, SessionCleanupError> {
        self.delete_empty_session_with(operations, documents, key, input, |_| async { Ok(()) })
            .await
    }

    async fn delete_empty_session_with<C, F>(
        &self,
        operations: &Path,
        documents: Option<&Path>,
        key: SessionCleanupKey,
        input: DeleteEmptySessionRequest,
        mut checkpoint: C,
    ) -> Result<DeleteEmptySessionOutcome, SessionCleanupError>
    where
        C: FnMut(Stage) -> F + Send + 'static,
        F: std::future::Future<Output = Result<(), StoreError>> + Send + 'static,
    {
        hypr_loofah_protocol::validate_delete_empty_session_request(&input).map_err(|message| {
            SessionCleanupError::Rejected(StoreError::Serialize(message.into()))
        })?;
        if key.namespace.is_empty()
            || key.namespace.len() > 4096
            || key.vault_id.is_empty()
            || key.vault_id.len() > 4096
        {
            return Err(SessionCleanupError::Rejected(StoreError::Serialize(
                "Invalid cleanup identity".into(),
            )));
        }
        let binding = Binding {
            vault_id: key.vault_id.clone(),
            namespace: key.namespace.clone(),
            request_id: key.request_id.to_string(),
            session_id: input.session_id,
        };
        let namespace = serde_json::to_string(&(&key.vault_id, &key.namespace)).unwrap();
        let receipt_path = operations.join(format!(
            "{}.json",
            operation_io::operation_key(&namespace, key.request_id)
        ));
        let documents = documents.map(Path::to_path_buf);
        let store = self.clone();
        tokio::spawn(async move {
            store
                .ensure_ready()
                .map_err(SessionCleanupError::Rejected)?;
            // Completed decisions are historical, even if the session was restored or is now busy.
            if let Some(receipt) = read(&store, &receipt_path).await? {
                validate_receipt(&receipt, &binding)?;
                if let State::Completed { outcome } = receipt.state {
                    return Ok(outcome);
                }
                if matches!(receipt.state, State::Uncertain) {
                    return Err(uncertain("Cleanup needs review"));
                }
            }
            let _audio = store
                .lock_session_audio(&binding.session_id)
                .await
                .map_err(|cause| match cause {
                    StoreError::Conflict(_) => SessionCleanupError::Busy,
                    other => SessionCleanupError::Rejected(other),
                })?;
            let _transcript = store.transcript_operations.lock(&binding.session_id).await;
            let snapshot = store
                .lock_recovered_snapshot(documents.as_deref(), &binding.session_id)
                .await
                .map_err(SessionCleanupError::Rejected)?;
            store
                .ensure_ready()
                .map_err(SessionCleanupError::Rejected)?;
            if let Some(receipt) = read(&store, &receipt_path).await? {
                validate_receipt(&receipt, &binding)?;
                return store
                    .recover_cleanup(&snapshot, receipt, &receipt_path)
                    .await;
            }
            let baseline = match store
                .inspect_cleanup(&snapshot, &binding.session_id)
                .await?
            {
                Inspection::Empty(candidate) => candidate,
                Inspection::Skip(outcome) => {
                    let receipt = Receipt {
                        schema_version: 1,
                        binding,
                        state: State::Completed { outcome },
                    };
                    persist(&store, &receipt_path, &receipt)
                        .await
                        .map_err(SessionCleanupError::Rejected)?;
                    return Ok(outcome);
                }
            };
            checkpoint(Stage::Checked)
                .await
                .map_err(SessionCleanupError::Rejected)?;
            let vault = store.vault_base.clone();
            let source = vault.join(
                session_directory(&binding.session_id).map_err(SessionCleanupError::Rejected)?,
            );
            let planned_source = source.clone();
            let planned_vault = vault.clone();
            let trash = store
                .spawn_blocking(move || {
                    hypr_fs_sync_core::export::prepare_trash_destination(
                        &planned_vault,
                        &planned_source,
                    )
                    .map_err(io_error)
                })
                .await
                .map_err(worker_error)?
                .map_err(SessionCleanupError::Rejected)?;
            let mut receipt = Receipt {
                schema_version: 1,
                binding,
                state: State::Pending {
                    trash: trash
                        .strip_prefix(&vault)
                        .map_err(|_| uncertain("Invalid cleanup trash destination"))?
                        .to_path_buf(),
                    meta_sha256: baseline.meta_sha256.clone(),
                },
            };
            persist(&store, &receipt_path, &receipt)
                .await
                .map_err(SessionCleanupError::Uncertain)?;
            checkpoint(Stage::Intent)
                .await
                .map_err(SessionCleanupError::Uncertain)?;
            let rechecked = store
                .inspect_cleanup(&snapshot, &receipt.binding.session_id)
                .await
                .map_err(admitted)?;
            if !matches!(rechecked, Inspection::Empty(ref current) if current == &baseline) {
                receipt.state = State::Uncertain;
                persist(&store, &receipt_path, &receipt)
                    .await
                    .map_err(SessionCleanupError::Uncertain)?;
                return Err(uncertain("Session changed before cleanup"));
            }
            let move_source = source.clone();
            let move_trash = trash.clone();
            store
                .spawn_blocking(move || {
                    hypr_storage::fs::rename_no_replace(&move_source, &move_trash).map_err(io_error)
                })
                .await
                .map_err(worker_error)?
                .map_err(SessionCleanupError::Uncertain)?;
            store.live.lock().await.remove(&receipt.binding.session_id);
            store.recent_deletions.lock().unwrap().insert(
                receipt.binding.session_id.clone(),
                crate::session_path::DeletedSession {
                    trash_path: trash.clone(),
                },
            );
            store
                .deleted_sessions
                .lock()
                .unwrap()
                .insert(receipt.binding.session_id.clone());
            store.index_remove_session_and_notify(&receipt.binding.session_id);
            checkpoint(Stage::Renamed)
                .await
                .map_err(SessionCleanupError::Uncertain)?;
            sync_parents(&store, source, trash)
                .await
                .map_err(SessionCleanupError::Uncertain)?;
            checkpoint(Stage::Synced)
                .await
                .map_err(SessionCleanupError::Uncertain)?;
            receipt.state = State::Completed {
                outcome: DeleteEmptySessionOutcome::Deleted,
            };
            persist(&store, &receipt_path, &receipt)
                .await
                .map_err(SessionCleanupError::Uncertain)?;
            checkpoint(Stage::Completed)
                .await
                .map_err(SessionCleanupError::Uncertain)?;
            Ok(DeleteEmptySessionOutcome::Deleted)
        })
        .await
        .map_err(worker_error)?
    }

    async fn inspect_cleanup(
        &self,
        snapshot: &VaultSnapshotGuard,
        id: &str,
    ) -> Result<Inspection, SessionCleanupError> {
        if self.is_recording(id) || self.attachment_stages.lock().unwrap().contains_key(id) {
            return Err(SessionCleanupError::Busy);
        }
        if self.live.lock().await.get(id).is_some_and(|buffer| {
            buffer.dirty
                || !buffer.words.is_empty()
                || !buffer.hints.is_empty()
                || !buffer.transcript_id.is_empty()
        }) {
            return Err(SessionCleanupError::Busy);
        }
        let mut bounded = self.clone();
        bounded.content_limits.metadata_bytes =
            bounded.content_limits.metadata_bytes.min(8 * 1024 * 1024);
        bounded.content_limits.note_read_bytes =
            bounded.content_limits.note_read_bytes.min(16 * 1024 * 1024);
        let Some(metadata) = bounded
            .metadata_snapshot_locked(&snapshot._guard, id)
            .await
            .map_err(SessionCleanupError::Rejected)?
        else {
            let directory = self
                .vault_base
                .join(session_directory(id).map_err(SessionCleanupError::Rejected)?);
            return self
                .spawn_blocking(move || match directory.symlink_metadata() {
                    Ok(_) => Err(SessionCleanupError::Rejected(StoreError::Io(
                        "Session metadata is missing".into(),
                    ))),
                    Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
                        Ok(Inspection::Skip(DeleteEmptySessionOutcome::NotFound))
                    }
                    Err(cause) => Err(SessionCleanupError::Rejected(io_error(cause))),
                })
                .await
                .map_err(worker_error)?;
        };
        if metadata
            .meta
            .extra
            .get("audio_import_pending")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        {
            return Err(SessionCleanupError::Busy);
        }
        if !metadata.meta.title.trim().is_empty() || !metadata.meta.tags.is_empty() {
            return Ok(Inspection::Skip(DeleteEmptySessionOutcome::NotEmpty));
        }
        let note = bounded
            .read_note_snapshot_locked(&snapshot._guard, id)
            .await
            .map_err(SessionCleanupError::Rejected)?
            .ok_or_else(|| {
                SessionCleanupError::Rejected(StoreError::Io(
                    "Session disappeared during cleanup".into(),
                ))
            })?;
        if note
            .markdown
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty() && text.trim() != "&nbsp;")
            || !note.title.trim().is_empty()
        {
            return Ok(Inspection::Skip(DeleteEmptySessionOutcome::NotEmpty));
        }
        let vault = self.vault_base.clone();
        let directory = vault.join(session_directory(id).map_err(SessionCleanupError::Rejected)?);
        let meta_limit = self.content_limits.metadata_bytes.min(8 * 1024 * 1024);
        let other_content = self
            .spawn_blocking(move || -> Result<Option<String>, StoreError> {
                if has_attachments(&directory)? || has_documents(&directory)? {
                    return Ok(None);
                }
                if let Some(bytes) =
                    bounded_file(&directory.join("transcript.json"), 32 * 1024 * 1024)?
                {
                    let transcripts: hypr_fs_format::TranscriptJsonStats =
                        serde_json::from_slice(&bytes)
                            .map_err(|cause| StoreError::Serialize(cause.to_string()))?;
                    if !transcripts.transcripts.is_empty() {
                        return Ok(None);
                    }
                }
                if let Some(bytes) = bounded_file(&directory.join("tasks.json"), 8 * 1024 * 1024)? {
                    serde_json::from_slice::<hypr_vault_read::TasksFile>(&bytes)
                        .map_err(|cause| StoreError::Serialize(cause.to_string()))?;
                }
                let meta = bounded_file(&directory.join("_meta.json"), meta_limit)?
                    .ok_or_else(|| StoreError::Io("Session metadata disappeared".into()))?;
                Ok(Some(crate::sha256(&meta)))
            })
            .await
            .map_err(worker_error)?
            .map_err(SessionCleanupError::Rejected)?;
        Ok(other_content
            .map(|meta_sha256| {
                Inspection::Empty(Candidate {
                    meta_sha256,
                    note_revision: note.revision,
                    metadata_revision: metadata.revision,
                })
            })
            .unwrap_or(Inspection::Skip(DeleteEmptySessionOutcome::NotEmpty)))
    }

    async fn recover_cleanup(
        &self,
        snapshot: &VaultSnapshotGuard,
        mut receipt: Receipt,
        receipt_path: &Path,
    ) -> Result<DeleteEmptySessionOutcome, SessionCleanupError> {
        match &receipt.state {
            State::Completed { outcome } => return Ok(*outcome),
            State::Uncertain => return Err(uncertain("Cleanup needs review")),
            State::Pending { .. } => {}
        }
        let State::Pending { trash, meta_sha256 } = &receipt.state else {
            unreachable!()
        };
        let relative = validate_trash(trash, &receipt.binding.session_id)
            .map_err(SessionCleanupError::Uncertain)?;
        let vault = self.vault_base.clone();
        let source = vault.join(
            session_directory(&receipt.binding.session_id)
                .map_err(SessionCleanupError::Uncertain)?,
        );
        let trash = vault.join(relative);
        let check_source = source.clone();
        let check_trash = trash.clone();
        let expected = meta_sha256.clone();
        let id = receipt.binding.session_id.clone();
        let limit = self.content_limits.metadata_bytes.min(8 * 1024 * 1024);
        let proven = self
            .spawn_blocking(move || {
                match check_source.symlink_metadata() {
                    Ok(_) => return Ok(false),
                    Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                    Err(cause) => return Err(io_error(cause)),
                }
                safe_trash_directory(&vault, &check_trash)?;
                let Some(raw) = bounded_file(&check_trash.join("_meta.json"), limit)? else {
                    return Ok(false);
                };
                let meta: crate::SessionMeta = serde_json::from_slice(&raw)
                    .map_err(|cause| StoreError::Serialize(cause.to_string()))?;
                Ok(meta.id == id && crate::sha256(&raw) == expected)
            })
            .await
            .map_err(worker_error)?
            .map_err(SessionCleanupError::Uncertain)?;
        if !proven {
            receipt.state = State::Uncertain;
            persist(self, receipt_path, &receipt)
                .await
                .map_err(SessionCleanupError::Uncertain)?;
            return Err(uncertain(
                "Interrupted cleanup is ambiguous; current files were preserved",
            ));
        }
        let _guard = &snapshot._guard;
        self.deleted_sessions
            .lock()
            .unwrap()
            .insert(receipt.binding.session_id.clone());
        self.index_remove_session_and_notify(&receipt.binding.session_id);
        sync_parents(self, source, trash)
            .await
            .map_err(SessionCleanupError::Uncertain)?;
        receipt.state = State::Completed {
            outcome: DeleteEmptySessionOutcome::Deleted,
        };
        persist(self, receipt_path, &receipt)
            .await
            .map_err(SessionCleanupError::Uncertain)?;
        Ok(DeleteEmptySessionOutcome::Deleted)
    }
}

#[derive(PartialEq, Eq)]
struct Candidate {
    meta_sha256: String,
    metadata_revision: String,
    note_revision: String,
}

enum Inspection {
    Empty(Candidate),
    Skip(DeleteEmptySessionOutcome),
}

fn validate_receipt(receipt: &Receipt, expected: &Binding) -> Result<(), SessionCleanupError> {
    if &receipt.binding != expected {
        return Err(SessionCleanupError::Rejected(StoreError::Conflict(
            "Cleanup UUID was used with different input".into(),
        )));
    }
    if receipt.schema_version != 1 {
        return Err(uncertain("Unsupported cleanup receipt version"));
    }
    Ok(())
}
async fn read(store: &SessionStore, path: &Path) -> Result<Option<Receipt>, SessionCleanupError> {
    let path = path.to_path_buf();
    store
        .spawn_blocking(move || operation_io::read_receipt(&path))
        .await
        .map_err(worker_error)?
        .map_err(SessionCleanupError::Uncertain)
}
async fn persist(store: &SessionStore, path: &Path, receipt: &Receipt) -> Result<(), StoreError> {
    let path = path.to_path_buf();
    let receipt = receipt.clone();
    store
        .spawn_blocking(move || operation_io::persist(&path, &receipt))
        .await
        .map_err(|cause| StoreError::Io(cause.to_string()))?
}
async fn sync_parents(
    store: &SessionStore,
    source: PathBuf,
    trash: PathBuf,
) -> Result<(), StoreError> {
    store
        .spawn_blocking(move || {
            operation_io::sync_parent(&source)?;
            operation_io::sync_parent(&trash)
        })
        .await
        .map_err(|cause| StoreError::Io(cause.to_string()))?
}
fn bounded_file(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, StoreError> {
    let Some(file) = crate::media::open_regular_file(path)? else {
        return Ok(None);
    };
    if file.metadata().map_err(io_error)?.len() > limit as u64 {
        return Err(StoreError::ResourceLimit(
            "Cleanup content exceeds inspection limit".into(),
        ));
    }
    let mut raw = Vec::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut raw)
        .map_err(io_error)?;
    if raw.len() > limit {
        return Err(StoreError::ResourceLimit(
            "Cleanup content exceeds inspection limit".into(),
        ));
    }
    Ok(Some(raw))
}
fn regular_directory(path: &Path) -> Result<bool, StoreError> {
    match path.symlink_metadata() {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => Ok(true),
        Ok(_) => Err(StoreError::Io(
            "Cleanup path is not a regular directory".into(),
        )),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(cause) => Err(io_error(cause)),
    }
}
fn bounded_entries(directory: &Path) -> Result<Vec<std::fs::DirEntry>, StoreError> {
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(directory).map_err(io_error)? {
        if entries.len() == MAX_DIRECTORY_ENTRIES {
            return Err(StoreError::ResourceLimit(
                "Cleanup directory exceeds its inspection limit".into(),
            ));
        }
        entries.push(entry.map_err(io_error)?);
    }
    Ok(entries)
}
fn has_attachments(directory: &Path) -> Result<bool, StoreError> {
    let attachments = directory.join("attachments");
    let managed = regular_directory(&attachments)?;
    for entry in bounded_entries(directory)? {
        let name = entry.file_name();
        let owned = name
            .to_str()
            .is_some_and(hypr_vault_read::is_session_owned_name);
        if !owned && entry.file_type().map_err(io_error)?.is_file() {
            return Ok(true);
        }
    }
    if managed {
        return Ok(bounded_entries(&attachments)?
            .into_iter()
            .any(|entry| !entry.file_name().to_string_lossy().starts_with('.')));
    }
    Ok(false)
}
fn has_documents(directory: &Path) -> Result<bool, StoreError> {
    let summary = directory.join("summary.md");
    if crate::media::open_regular_file(&summary)?.is_some() {
        return Ok(true);
    }
    let enhanced = directory.join("enhanced");
    if !regular_directory(&enhanced)? {
        return Ok(false);
    }
    for entry in bounded_entries(&enhanced)? {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        if name.to_string_lossy().contains(".conflict-") {
            return Err(StoreError::Conflict(
                "An enhanced conflict file needs review".into(),
            ));
        }
        if !entry.file_type().map_err(io_error)?.is_file() {
            return Err(StoreError::Io(
                "Enhanced output is not a regular file".into(),
            ));
        }
        if entry.path().extension().is_some_and(|ext| ext == "md") {
            return Ok(true);
        }
        return Err(StoreError::Io(
            "Unknown enhanced output needs review".into(),
        ));
    }
    Ok(false)
}
fn validate_trash(path: &Path, id: &str) -> Result<PathBuf, StoreError> {
    let parts: Vec<_> = path.components().collect();
    if parts.len() != 4
        || parts
            .iter()
            .any(|part| !matches!(part, Component::Normal(_)))
        || parts[0].as_os_str() != ".trash"
        || parts[2].as_os_str() != "sessions"
    {
        return Err(StoreError::Io("Invalid cleanup trash path".into()));
    }
    let Some(date) = parts[1].as_os_str().to_str() else {
        return Err(StoreError::Io("Invalid cleanup date".into()));
    };
    chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")
        .map_err(|cause| StoreError::Io(cause.to_string()))?;
    let name = parts[3]
        .as_os_str()
        .to_str()
        .ok_or_else(|| StoreError::Io("Invalid trash basename".into()))?;
    let source = Path::new(id);
    let stem = source.file_stem().and_then(|value| value.to_str()).unwrap();
    let candidate = match source.extension().and_then(|value| value.to_str()) {
        Some(extension) => name.strip_suffix(&format!(".{extension}")),
        None => Some(name),
    };
    if name != id
        && !candidate
            .and_then(|value| value.strip_prefix(&format!("{stem}-")))
            .is_some_and(|suffix| {
                !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit())
            })
    {
        return Err(StoreError::Io("Cleanup trash identity mismatch".into()));
    }
    Ok(path.to_path_buf())
}
fn safe_trash_directory(vault: &Path, trash: &Path) -> Result<(), StoreError> {
    let mut current = vault.to_path_buf();
    for part in trash
        .strip_prefix(vault)
        .map_err(|cause| StoreError::Io(cause.to_string()))?
        .components()
    {
        if !matches!(part, Component::Normal(_)) {
            return Err(StoreError::Io("Unsafe cleanup path".into()));
        }
        current.push(part.as_os_str());
        if !regular_directory(&current)? {
            return Err(StoreError::Io("Cleanup trash entry is missing".into()));
        }
    }
    Ok(())
}
fn admitted(cause: SessionCleanupError) -> SessionCleanupError {
    match cause {
        SessionCleanupError::Rejected(error) => SessionCleanupError::Uncertain(error),
        SessionCleanupError::Busy => uncertain("Session became busy after cleanup admission"),
        other => other,
    }
}
fn uncertain(message: &str) -> SessionCleanupError {
    SessionCleanupError::Uncertain(StoreError::Conflict(message.into()))
}
fn worker_error(cause: tokio::task::JoinError) -> SessionCleanupError {
    uncertain(&format!("Cleanup worker stopped: {cause}"))
}
fn io_error(cause: impl std::fmt::Display) -> StoreError {
    StoreError::Io(cause.to_string())
}

fn session_directory(id: &str) -> Result<PathBuf, StoreError> {
    paths::validated_session_dir(id).map_err(StoreError::from)
}

impl std::fmt::Display for SessionCleanupError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy => formatter.write_str("Session is busy"),
            Self::Rejected(cause) | Self::Uncertain(cause) => write!(formatter, "{cause}"),
        }
    }
}
impl std::error::Error for SessionCleanupError {}

#[cfg(test)]
mod tests;
