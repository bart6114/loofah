use std::{
    io::Read,
    path::{Path, PathBuf},
};

use hypr_loofah_protocol::{DeleteDocumentOutcome, DeleteDocumentRequest, DocumentTarget};
use serde::{Deserialize, Serialize};

use crate::{
    SessionStore, StoreError, WriteGuard,
    file_snapshot::FileSnapshot,
    media::open_regular_file,
    operation_io::{operation_key, sync_parent},
};

#[derive(Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Binding {
    namespace: String,
    request_id: uuid::Uuid,
    session_id: String,
    document_id: String,
    input: DeleteDocumentRequest,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Intent {
    pub schema_version: u32,
    pub session_id: String,
    binding: Binding,
    source_sha256: String,
    trash_date: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    binding: Binding,
    intent: Option<Intent>,
    outcome: Option<DeleteDocumentOutcome>,
    #[serde(default)]
    review_required: bool,
}

impl Binding {
    fn key(&self) -> String {
        operation_key(&self.namespace, self.request_id)
    }
    fn target(&self) -> DocumentTarget {
        DocumentTarget::TemplateOutput {
            document_id: self.document_id.clone(),
        }
    }
    fn receipt_path(&self, root: &Path) -> PathBuf {
        root.join(".deletions").join(format!("{}.json", self.key()))
    }
    fn validate(&self) -> Result<(), StoreError> {
        crate::validate_session_id(&self.session_id)?;
        crate::validate_doc_id(&self.document_id)?;
        if self.namespace.is_empty()
            || self.namespace.len() > 1024
            || self.namespace.chars().any(char::is_control)
            || self.request_id.is_nil()
        {
            return Err(invalid("Invalid document deletion namespace or request"));
        }
        hypr_loofah_protocol::validate_delete_document_request(&self.input).map_err(invalid)?;
        Ok(())
    }
}

impl Intent {
    fn trash_relative(&self) -> PathBuf {
        PathBuf::from(".trash")
            .join(&self.trash_date)
            .join("sessions")
            .join(&self.session_id)
            .join("enhanced")
            .join(format!(
                "{}-{}.md",
                self.binding.document_id,
                self.binding.key()
            ))
    }
    fn outcome(&self) -> DeleteDocumentOutcome {
        DeleteDocumentOutcome::Deleted {
            revision: self.binding.input.expected_revision.clone(),
            trash_token: self.binding.key(),
        }
    }
}

pub(crate) fn validate(intent: &Intent) -> Result<(), StoreError> {
    intent.binding.validate()?;
    if intent.schema_version != 6
        || intent.session_id != intent.binding.session_id
        || !hypr_loofah_protocol::is_session_lifecycle_revision(&intent.source_sha256)
    {
        return Err(invalid("Invalid document deletion intent"));
    }
    let date = chrono::NaiveDate::parse_from_str(&intent.trash_date, "%Y-%m-%d")
        .map_err(|_| invalid("Invalid deletion trash date"))?;
    if date.format("%Y-%m-%d").to_string() != intent.trash_date {
        return Err(invalid("Invalid deletion trash date"));
    }
    Ok(())
}

impl SessionStore {
    pub async fn delete_document_checked(
        &self,
        root: &Path,
        namespace: &str,
        request_id: uuid::Uuid,
        id: &str,
        target: &DocumentTarget,
        input: DeleteDocumentRequest,
    ) -> Result<DeleteDocumentOutcome, StoreError> {
        let DocumentTarget::TemplateOutput { document_id } = target else {
            return Err(invalid("Only template outputs can be deleted"));
        };
        let binding = Binding {
            namespace: namespace.into(),
            request_id,
            session_id: id.into(),
            document_id: document_id.clone(),
            input,
        };
        binding.validate()?;
        self.validate_document_root(root)?;
        let root = root.to_owned();
        let store = self.clone();
        tokio::spawn(async move {
            let guard = store
                .lock_recovered_snapshot(Some(&root), &binding.session_id)
                .await?;
            store.ensure_ready()?;
            let receipt_path = binding.receipt_path(&root);
            let existing = store.read_deletion_receipt(&receipt_path).await?;
            if let Some(receipt) = &existing {
                if receipt.binding != binding {
                    return Err(StoreError::Conflict(
                        "This deletion UUID is bound to a different request".into(),
                    ));
                }
                if let Some(outcome) = &receipt.outcome {
                    return Ok(outcome.clone());
                }
                if receipt.review_required {
                    return Err(StoreError::Conflict(
                        "Deletion may already have moved this output; review required".into(),
                    ));
                }
            }
            let snapshot = store
                .document_editor_snapshot_locked(
                    &guard._guard,
                    &binding.session_id,
                    &binding.target(),
                )
                .await?;
            if snapshot.is_none()
                && existing
                    .as_ref()
                    .is_some_and(|receipt| receipt.intent.is_some())
            {
                return Err(StoreError::after_commit(StoreError::Conflict(
                    "Original deletion source and retained result are unavailable; review required"
                        .into(),
                )));
            }
            let outcome = match &snapshot {
                None => Some(DeleteDocumentOutcome::NotFound),
                Some(snapshot)
                    if snapshot.document.revision != binding.input.expected_revision
                        || snapshot.metadata.meta.created_at
                            != binding.input.expected_session_created_at =>
                {
                    Some(DeleteDocumentOutcome::Conflict {
                        snapshot: Box::new(snapshot.clone()),
                    })
                }
                _ => None,
            };
            if let Some(outcome) = outcome {
                store
                    .persist_deletion_receipt(
                        &receipt_path,
                        Receipt {
                            schema_version: 1,
                            binding,
                            intent: existing.and_then(|receipt| receipt.intent),
                            outcome: Some(outcome.clone()),
                            review_required: false,
                        },
                    )
                    .await?;
                return Ok(outcome);
            }
            let relative = crate::paths::enhanced_doc_path_in(
                &store
                    .session_dir_locked(&guard._guard, &binding.session_id)
                    .await?,
                &binding.document_id,
            );
            let source = store.vault_base.join(relative);
            let vault = store.vault_base.clone();
            let check_binding = binding.clone();
            let check_source = source.clone();
            let limit = store.content_limits.note_read_bytes;
            let initial = store
                .spawn_blocking(move || {
                    checked_source(&vault, &check_source, &check_binding, limit)
                })
                .await
                .map_err(worker)??;
            let intent = if let Some(intent) = existing.and_then(|receipt| receipt.intent) {
                validate(&intent)?;
                if initial.1 != intent.source_sha256 {
                    return Err(StoreError::Conflict(
                        "Original deletion source changed".into(),
                    ));
                }
                intent
            } else {
                Intent {
                    schema_version: 6,
                    session_id: binding.session_id.clone(),
                    binding: binding.clone(),
                    source_sha256: initial.1.clone(),
                    trash_date: chrono::Utc::now().format("%Y-%m-%d").to_string(),
                }
            };
            validate(&intent)?;
            let receipt = Receipt {
                schema_version: 1,
                binding: binding.clone(),
                intent: Some(intent.clone()),
                outcome: None,
                review_required: false,
            };
            store
                .persist_deletion_receipt(&receipt_path, receipt)
                .await?;
            let pending = store
                .begin_document_deletion_locked(&guard._guard, &root, intent.clone())
                .await
                .map_err(StoreError::after_commit)?;
            #[cfg(test)]
            tests::checkpoint("intent");
            let vault = store.vault_base.clone();
            let move_intent = intent.clone();
            store
                .spawn_blocking(move || {
                    let current = checked_source(&vault, &source, &move_intent.binding, limit)?;
                    if !initial.0.unchanged(&current.0)? || current.1 != move_intent.source_sha256 {
                        return Err(StoreError::Conflict(
                            "Deletion source changed before trash move".into(),
                        ));
                    }
                    let trash = vault.join(move_intent.trash_relative());
                    ensure_descendants(&vault, trash.parent().unwrap(), true)?;
                    hypr_storage::fs::rename_no_replace(&source, &trash).map_err(io_error)?;
                    sync_parent(&source)?;
                    sync_parent(&trash)?;
                    Ok(())
                })
                .await
                .map_err(worker)?
                .map_err(StoreError::after_commit)?;
            #[cfg(test)]
            tests::checkpoint("moved");
            recover(&store, &guard._guard, pending.path(), &intent)
                .await
                .map_err(StoreError::after_commit)?;
            Ok(intent.outcome())
        })
        .await
        .map_err(worker)?
    }

    async fn read_deletion_receipt(&self, path: &Path) -> Result<Option<Receipt>, StoreError> {
        let path = path.to_owned();
        let limit = self.content_limits.recovery_bytes;
        self.spawn_blocking(move || {
            ensure_descendants(
                path.parent().unwrap().parent().unwrap(),
                path.parent().unwrap(),
                false,
            )?;
            let Some(file) = open_regular_file(&path)? else {
                return Ok(None);
            };
            let mut raw = Vec::new();
            file.take(limit.saturating_add(1) as u64)
                .read_to_end(&mut raw)
                .map_err(io_error)?;
            if raw.len() > limit {
                return Err(invalid("Deletion receipt exceeds its size limit"));
            }
            let unique = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&raw)
                .map_err(|_| invalid("Invalid deletion receipt"))?;
            let receipt: Receipt = serde_json::from_value(unique.0)
                .map_err(|_| invalid("Invalid deletion receipt"))?;
            receipt.binding.validate()?;
            if receipt.schema_version != 1
                || (receipt.review_required
                    && (receipt.intent.is_none() || receipt.outcome.is_some()))
                || receipt
                    .binding
                    .receipt_path(path.parent().unwrap().parent().unwrap())
                    != path
            {
                return Err(invalid("Invalid deletion receipt identity"));
            }
            if let Some(intent) = &receipt.intent {
                validate(intent)?;
                if intent.binding != receipt.binding {
                    return Err(invalid("Deletion receipt intent changed"));
                }
            }
            if let Some(DeleteDocumentOutcome::Deleted { .. }) = &receipt.outcome
                && receipt
                    .intent
                    .as_ref()
                    .is_none_or(|intent| Some(intent.outcome()) != receipt.outcome)
            {
                return Err(invalid(
                    "Deletion result does not match its original intent",
                ));
            }
            Ok(Some(receipt))
        })
        .await
        .map_err(worker)?
    }

    async fn persist_deletion_receipt(
        &self,
        path: &Path,
        receipt: Receipt,
    ) -> Result<(), StoreError> {
        let path = path.to_owned();
        let limit = self.content_limits.recovery_bytes;
        self.spawn_blocking(move || {
            let bytes = serde_json::to_string(&receipt).map_err(|e| invalid(&e.to_string()))?;
            if bytes.len() > limit {
                return Err(StoreError::ResourceLimit(
                    "Deletion receipt exceeds its size limit".into(),
                ));
            }
            let root = path.parent().unwrap().parent().unwrap();
            crate::operation_io::ensure_directory(root)?;
            ensure_descendants(root, path.parent().unwrap(), true)?;
            if std::fs::symlink_metadata(&path)
                .is_ok_and(|meta| !meta.is_file() || meta.file_type().is_symlink())
            {
                return Err(invalid("Deletion receipt is not a regular file"));
            }
            hypr_storage::fs::atomic_write(&path, &bytes).map_err(io_error)?;
            sync_parent(&path)
        })
        .await
        .map_err(worker)?
    }
}

// Recovery acknowledges an already-completed move. It never initiates deletion on startup.
pub(crate) async fn recover(
    store: &SessionStore,
    _guard: &WriteGuard<'_>,
    pending: &Path,
    intent: &Intent,
) -> Result<(), StoreError> {
    validate(intent)?;
    let root = pending.parent().unwrap();
    let receipt_path = intent.binding.receipt_path(root);
    let mut receipt = store
        .read_deletion_receipt(&receipt_path)
        .await?
        .ok_or_else(|| invalid("Deletion intent has no original receipt"))?;
    if receipt.binding != intent.binding
        || receipt.intent.as_ref().is_none_or(|saved| {
            serde_json::to_value(saved).ok() != serde_json::to_value(intent).ok()
        })
    {
        return Err(invalid("Deletion receipt and pending intent disagree"));
    }
    if receipt.outcome.is_none() && !receipt.review_required {
        let vault = store.vault_base.clone();
        let expected = intent.source_sha256.clone();
        let trash = vault.join(intent.trash_relative());
        let moved = store
            .spawn_blocking(move || {
                ensure_descendants(&vault, trash.parent().unwrap(), false)?;
                match open_regular_file(&trash)? {
                    Some(file) => {
                        if FileSnapshot::new(file)?.hash()? != expected {
                            return Err(invalid(
                                "Retained deletion bytes changed; recovery requires review",
                            ));
                        }
                        Ok(true)
                    }
                    None => Ok(false),
                }
            })
            .await
            .map_err(worker)??;
        if moved {
            receipt.outcome = Some(intent.outcome());
            store
                .persist_deletion_receipt(&receipt_path, receipt)
                .await?;
            // A restored file may already be present. Never remove or hide it on replay.
            let current = store
                .document_editor_snapshot_locked(
                    _guard,
                    &intent.session_id,
                    &intent.binding.target(),
                )
                .await?;
            if current.is_none() {
                store.index_remove_doc(&intent.session_id, &intent.binding.document_id);
            }
            store.notify_index_changed(crate::IndexEntity::Docs, vec![intent.session_id.clone()]);
        } else {
            // Missing trash cannot distinguish a never-started move from an operator restore.
            // Persist this ambiguity before clearing the pending slot so this UUID never moves again.
            receipt.review_required = true;
            store
                .persist_deletion_receipt(&receipt_path, receipt)
                .await?;
        }
    }
    crate::document_operation::finish_title_intent(store, pending).await
}

fn checked_source(
    vault: &Path,
    source: &Path,
    binding: &Binding,
    limit: usize,
) -> Result<(FileSnapshot, String), StoreError> {
    ensure_descendants(vault, source.parent().unwrap(), false)?;
    let file = open_regular_file(source)?
        .ok_or_else(|| StoreError::Conflict("Deletion source disappeared".into()))?;
    let snapshot = FileSnapshot::new(file)?;
    let mut bytes = Vec::new();
    snapshot
        .file()
        .take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > limit {
        return Err(StoreError::ResourceLimit(
            "Deletion source exceeds document limit".into(),
        ));
    }
    if crate::document_revision::revision(&binding.session_id, &binding.target(), Some(&bytes))?
        != binding.input.expected_revision
    {
        return Err(StoreError::Conflict("Deletion source changed".into()));
    }
    if !snapshot.lock_for_recheck()? {
        return Err(StoreError::Conflict("Deletion source is busy".into()));
    }
    let hash = crate::sha256(&bytes);
    Ok((snapshot, hash))
}

fn ensure_descendants(root: &Path, path: &Path, create: bool) -> Result<(), StoreError> {
    let relative = path
        .strip_prefix(root)
        .map_err(|_| invalid("Deletion path is outside its owner"))?;
    let mut current = root.to_owned();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(invalid("Invalid deletion path"));
        }
        current.push(component);
        match std::fs::symlink_metadata(&current) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(invalid("Deletion parent is not a regular directory")),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && create => {
                std::fs::create_dir(&current).map_err(io_error)?;
                sync_parent(&current)?;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(io_error(e)),
        }
    }
    Ok(())
}
fn invalid(message: &str) -> StoreError {
    StoreError::Serialize(message.into())
}
fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Io(error.to_string())
}
fn worker(error: tokio::task::JoinError) -> StoreError {
    StoreError::Io(format!("Document deletion worker failed: {error}"))
}

#[cfg(test)]
mod tests;
