use std::path::{Path, PathBuf};

use hypr_loofah_protocol::{
    DeleteSessionOutcome, DeleteSessionRequest, RestoreSessionConflictReason,
    RestoreSessionOutcome, RestoreSessionRequest, SessionLifecycleSnapshot,
};

use crate::{SessionStore, StoreError, VaultSnapshotGuard, operation_io};
use receipts::{DeleteReceipt, Receipt, RestoreReceipt, State, Target};

mod local_proof;
pub use local_proof::LocalSessionLifecycleProof;
mod receipts;
mod tree;

#[derive(Clone, Debug)]
pub struct LocalSessionDeletionDetails {
    pub path: PathBuf,
    pub trash_path: PathBuf,
    pub deleted_at: String,
}
#[derive(Clone, Debug)]
pub struct SessionLifecycleKey {
    pub vault_id: String,
    pub namespace: String,
    pub request_id: uuid::Uuid,
}
#[derive(Debug)]
pub enum LifecycleError {
    Busy,
    Rejected(StoreError),
    Uncertain(StoreError),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Intent,
    Renamed,
    Synced,
    Completed,
}
struct Owner {
    _audio: tokio::sync::OwnedMutexGuard<()>,
    _transcript: crate::transcript_operation::TranscriptOperation,
}

fn rejected(cause: StoreError) -> LifecycleError {
    LifecycleError::Rejected(cause)
}
fn uncertain(message: &str) -> LifecycleError {
    LifecycleError::Uncertain(StoreError::Io(message.into()))
}
fn io_error(cause: std::io::Error) -> StoreError {
    StoreError::Io(cause.to_string())
}
fn admitted(cause: LifecycleError) -> LifecycleError {
    match cause {
        LifecycleError::Rejected(cause) => LifecycleError::Uncertain(cause),
        LifecycleError::Busy => {
            uncertain("Retained lifecycle request is busy; its original outcome requires review")
        }
        other => other,
    }
}

impl SessionStore {
    async fn lifecycle_owner(
        &self,
        id: &str,
        documents: Option<&Path>,
    ) -> Result<Owner, LifecycleError> {
        let audio = self
            .lock_session_audio(id)
            .await
            .map_err(|cause| match cause {
                StoreError::Conflict(_) => LifecycleError::Busy,
                other => rejected(other),
            })?;
        let transcript = self.transcript_operations.lock(id).await;
        let snapshot = self
            .lock_recovered_snapshot(documents, id)
            .await
            .map_err(rejected)?;
        self.lifecycle_idle(id).await?;
        drop(snapshot);
        Ok(Owner {
            _audio: audio,
            _transcript: transcript,
        })
    }
    async fn lifecycle_idle(&self, id: &str) -> Result<(), LifecycleError> {
        self.ensure_ready().map_err(rejected)?;
        if self.is_recording(id)
            || self.attachment_stages.lock().unwrap().contains_key(id)
            || self.live.lock().await.get(id).is_some_and(|buffer| {
                buffer.dirty
                    || !buffer.words.is_empty()
                    || !buffer.hints.is_empty()
                    || !buffer.transcript_id.is_empty()
            })
        {
            return Err(LifecycleError::Busy);
        }
        Ok(())
    }
    async fn lifecycle_tree(
        &self,
        relative: PathBuf,
        id: String,
        proof: Option<std::sync::Arc<local_proof::Inner>>,
    ) -> Result<Option<tree::Tree>, LifecycleError> {
        let vault = self.vault_base.clone();
        let limits = self.content_limits;
        self.spawn_blocking(move || tree::prepare(&vault, &relative, &id, limits, proof))
            .await
            .map_err(|_| uncertain("Lifecycle reader stopped"))?
    }
    async fn lifecycle_recheck(
        &self,
        id: &str,
        relative: PathBuf,
        tree: tree::Tree,
    ) -> Result<(VaultSnapshotGuard, tree::Tree), LifecycleError> {
        let snapshot = self.lock_snapshot().await;
        self.lifecycle_idle(id).await?;
        let vault = self.vault_base.clone();
        let limits = self.content_limits;
        self.spawn_blocking(move || {
            if !tree
                .unchanged(&vault, &relative, limits)
                .map_err(rejected)?
            {
                return Err(rejected(StoreError::Conflict(
                    "Session changed before lifecycle commit".into(),
                )));
            }
            Ok((snapshot, tree))
        })
        .await
        .map_err(|_| uncertain("Lifecycle recheck stopped"))?
    }

    pub async fn read_session_lifecycle(
        &self,
        documents: Option<&Path>,
        id: &str,
    ) -> Result<Option<SessionLifecycleSnapshot>, LifecycleError> {
        self.read_session_lifecycle_with_proof(documents, id, None)
            .await
    }
    pub async fn read_local_session_lifecycle(
        &self,
        documents: Option<&Path>,
        id: &str,
        proof: LocalSessionLifecycleProof,
    ) -> Result<Option<SessionLifecycleSnapshot>, LifecycleError> {
        let proof = self.check_local_lifecycle_proof(id, proof)?;
        self.read_session_lifecycle_with_proof(documents, id, Some(proof))
            .await
    }
    async fn read_session_lifecycle_with_proof(
        &self,
        documents: Option<&Path>,
        id: &str,
        proof: Option<std::sync::Arc<local_proof::Inner>>,
    ) -> Result<Option<SessionLifecycleSnapshot>, LifecycleError> {
        crate::validate_session_id(id).map_err(rejected)?;
        let store = self.clone();
        let documents = documents.map(Path::to_path_buf);
        let id = id.to_owned();
        tokio::spawn(async move {
            let _owner = store.lifecycle_owner(&id, documents.as_deref()).await?;
            let source =
                crate::paths::validated_session_dir(&id).map_err(|cause| rejected(cause.into()))?;
            let Some(tree) = store
                .lifecycle_tree(source.clone(), id.clone(), proof.clone())
                .await?
            else {
                return Ok(None);
            };
            if store.deleted_sessions.lock().unwrap().contains(&id) {
                return Err(rejected(StoreError::Conflict(
                    "Session requires explicit restore or review".into(),
                )));
            }
            let (_snapshot, tree) = store.lifecycle_recheck(&id, source, tree).await?;
            Ok(Some(SessionLifecycleSnapshot {
                session_id: id,
                revision: tree.revision,
            }))
        })
        .await
        .map_err(|_| uncertain("Lifecycle snapshot worker stopped"))?
    }

    pub async fn session_delete_replay(
        root: &Path,
        key: &SessionLifecycleKey,
        input: &DeleteSessionRequest,
    ) -> Result<Option<DeleteSessionOutcome>, LifecycleError> {
        hypr_loofah_protocol::validate_delete_session_request(input)
            .map_err(|message| rejected(StoreError::Serialize(message.into())))?;
        let binding = key.binding()?;
        let token = key.token();
        let Some(receipt) =
            receipts::read::<DeleteReceipt>(receipts::deletion_path(root, &token)?).await?
        else {
            return Ok(None);
        };
        receipts::validate(&receipt, &binding, input)?;
        receipts::validate_delete(&receipt, &key.vault_id, &token)?;
        match receipt.state {
            State::Completed { outcome } => Ok(Some(outcome)),
            State::Uncertain => Err(uncertain("Deletion requires review")),
            State::Prepared => Ok(None),
        }
    }
    pub async fn session_delete_has_intent(
        root: &Path,
        key: &SessionLifecycleKey,
        input: &DeleteSessionRequest,
    ) -> Result<bool, LifecycleError> {
        let token = key.token();
        let Some(receipt) =
            receipts::read::<DeleteReceipt>(receipts::deletion_path(root, &token)?).await?
        else {
            return Ok(false);
        };
        receipts::validate(&receipt, &key.binding()?, input)?;
        receipts::validate_delete(&receipt, &key.vault_id, &token)?;
        Ok(matches!(receipt.state, State::Prepared))
    }
    pub async fn session_restore_has_intent(
        root: &Path,
        key: &SessionLifecycleKey,
        input: &RestoreSessionRequest,
    ) -> Result<bool, LifecycleError> {
        let Some(receipt) =
            receipts::read::<RestoreReceipt>(receipts::restoration_path(root, key)).await?
        else {
            return Ok(false);
        };
        receipts::validate_restore(&receipt, &key.binding()?, input)?;
        Ok(matches!(receipt.state, State::Prepared))
    }
    pub async fn session_restore_replay(
        root: &Path,
        key: &SessionLifecycleKey,
        input: &RestoreSessionRequest,
    ) -> Result<Option<RestoreSessionOutcome>, LifecycleError> {
        hypr_loofah_protocol::validate_restore_session_request(input)
            .map_err(|message| rejected(StoreError::Serialize(message.into())))?;
        let binding = key.binding()?;
        let Some(receipt) =
            receipts::read::<RestoreReceipt>(receipts::restoration_path(root, key)).await?
        else {
            return Ok(None);
        };
        receipts::validate_restore(&receipt, &binding, input)?;
        match receipt.state {
            State::Completed { outcome } => Ok(Some(outcome)),
            State::Uncertain => Err(uncertain("Restore requires review")),
            State::Prepared => Ok(None),
        }
    }
    pub async fn session_delete_details(
        &self,
        root: &Path,
        key: &SessionLifecycleKey,
        input: &DeleteSessionRequest,
    ) -> Result<Option<LocalSessionDeletionDetails>, LifecycleError> {
        if !matches!(
            Self::session_delete_replay(root, key, input).await?,
            Some(DeleteSessionOutcome::Trashed { .. })
        ) {
            return Ok(None);
        }
        let receipt = receipts::read::<DeleteReceipt>(receipts::deletion_path(root, &key.token())?)
            .await?
            .ok_or_else(|| uncertain("Deletion receipt disappeared"))?;
        receipts::validate(&receipt, &key.binding()?, input)?;
        receipts::validate_delete(&receipt, &key.vault_id, &key.token())?;
        let State::Completed {
            outcome: DeleteSessionOutcome::Trashed { .. },
        } = receipt.state
        else {
            return Err(uncertain("Deletion receipt changed"));
        };
        let target = receipt
            .target
            .ok_or_else(|| uncertain("Deletion target is missing"))?;
        Ok(Some(LocalSessionDeletionDetails {
            path: self.vault_base.join(target.source),
            trash_path: self.vault_base.join(target.trash),
            deleted_at: receipt
                .deleted_at
                .ok_or_else(|| uncertain("Deletion timestamp is missing"))?,
        }))
    }
    pub async fn session_restore_target(
        root: &Path,
        vault: &str,
        token: &str,
    ) -> Result<Option<String>, LifecycleError> {
        let Some(receipt) =
            receipts::read::<DeleteReceipt>(receipts::deletion_path(root, token)?).await?
        else {
            return Ok(None);
        };
        receipts::validate_delete(&receipt, vault, token)?;
        if matches!(receipt.state, State::Uncertain) {
            return Err(uncertain("Deletion requires review"));
        }
        Ok(receipt.target.map(|target| target.session_id))
    }

    pub async fn delete_session_checked(
        &self,
        root: &Path,
        documents: Option<&Path>,
        key: SessionLifecycleKey,
        input: DeleteSessionRequest,
    ) -> Result<DeleteSessionOutcome, LifecycleError> {
        self.delete_session_with(root, documents, key, input, |_| async { Ok(()) })
            .await
    }
    async fn delete_session_with<C, F>(
        &self,
        root: &Path,
        documents: Option<&Path>,
        key: SessionLifecycleKey,
        input: DeleteSessionRequest,
        checkpoint: C,
    ) -> Result<DeleteSessionOutcome, LifecycleError>
    where
        C: FnMut(Stage) -> F + Send + 'static,
        F: std::future::Future<Output = Result<(), StoreError>> + Send + 'static,
    {
        self.delete_session_with_proof(root, documents, key, input, None, checkpoint)
            .await
    }
    pub async fn delete_local_session_checked(
        &self,
        root: &Path,
        documents: Option<&Path>,
        key: SessionLifecycleKey,
        input: DeleteSessionRequest,
        proof: LocalSessionLifecycleProof,
    ) -> Result<DeleteSessionOutcome, LifecycleError> {
        let proof = self.check_local_lifecycle_proof(&input.session_id, proof)?;
        self.delete_session_with_proof(root, documents, key, input, Some(proof), |_| async {
            Ok(())
        })
        .await
    }
    async fn delete_session_with_proof<C, F>(
        &self,
        root: &Path,
        documents: Option<&Path>,
        key: SessionLifecycleKey,
        input: DeleteSessionRequest,
        proof: Option<std::sync::Arc<local_proof::Inner>>,
        mut checkpoint: C,
    ) -> Result<DeleteSessionOutcome, LifecycleError>
    where
        C: FnMut(Stage) -> F + Send + 'static,
        F: std::future::Future<Output = Result<(), StoreError>> + Send + 'static,
    {
        if let Some(outcome) = Self::session_delete_replay(root, &key, &input).await? {
            return Ok(outcome);
        }
        let has_intent = Self::session_delete_has_intent(root, &key, &input).await?;
        let store = self.clone();
        let root = root.to_path_buf();
        let documents = documents.map(Path::to_path_buf);
        tokio::spawn(async move {
            let _owner = store
                .lifecycle_owner(&input.session_id, documents.as_deref())
                .await?;
            let binding = key.binding()?;
            let token = key.token();
            let receipt_path = receipts::deletion_path(&root, &token)?;
            if let Some(receipt) = receipts::read::<DeleteReceipt>(receipt_path.clone()).await? {
                receipts::validate(&receipt, &binding, &input)?;
                receipts::validate_delete(&receipt, &key.vault_id, &token)?;
                return store
                    .recover_session_delete(receipt, &receipt_path, proof)
                    .await;
            }
            let source = crate::paths::validated_session_dir(&input.session_id)
                .map_err(|cause| rejected(cause.into()))?;
            let tree = store
                .lifecycle_tree(source.clone(), input.session_id.clone(), proof.clone())
                .await?;
            let mut receipt = Receipt {
                schema_version: 1,
                binding,
                input: input.clone(),
                target: None,
                prepared_revision: None,
                deleted_at: None,
                state: State::Prepared,
            };
            let (snapshot, tree) = if let Some(tree) = tree {
                if store
                    .deleted_sessions
                    .lock()
                    .unwrap()
                    .contains(&input.session_id)
                {
                    return Err(rejected(StoreError::Conflict(
                        "Session requires explicit restore or review".into(),
                    )));
                }
                let (snapshot, tree) = store
                    .lifecycle_recheck(&input.session_id, source.clone(), tree)
                    .await?;
                (snapshot, Some(tree))
            } else {
                (store.lock_snapshot().await, None)
            };
            if let Some(outcome) = Self::session_delete_replay(&root, &key, &input).await? {
                return Ok(outcome);
            }
            if tree.is_none() {
                let vault = store.vault_base.clone();
                let relative = source.clone();
                let absent = store
                    .spawn_blocking(move || {
                        tree::safe_directory(&vault, &relative).map(|exists| !exists)
                    })
                    .await
                    .map_err(|_| uncertain("Lifecycle reader stopped"))?
                    .map_err(rejected)?;
                if !absent {
                    return Err(rejected(StoreError::Conflict(
                        "Session changed during deletion admission".into(),
                    )));
                }
                receipt.state = State::Completed {
                    outcome: DeleteSessionOutcome::NotFound,
                };
                receipts::persist(&store, &receipt_path, &receipt)
                    .await
                    .map_err(rejected)?;
                return Ok(DeleteSessionOutcome::NotFound);
            }
            let tree = tree.unwrap();
            if tree.revision != input.expected_revision {
                let outcome = DeleteSessionOutcome::Conflict {
                    snapshot: SessionLifecycleSnapshot {
                        session_id: input.session_id.clone(),
                        revision: tree.revision,
                    },
                };
                receipt.state = State::Completed {
                    outcome: outcome.clone(),
                };
                receipts::persist(&store, &receipt_path, &receipt)
                    .await
                    .map_err(rejected)?;
                return Ok(outcome);
            }
            let vault = store.vault_base.clone();
            let planned = vault.join("sessions").join(&token);
            let plan_vault = vault.clone();
            let trash = store
                .spawn_blocking(move || {
                    hypr_fs_sync_core::export::prepare_trash_destination(&plan_vault, &planned)
                        .map_err(|cause| StoreError::Io(cause.to_string()))
                })
                .await
                .map_err(|_| uncertain("Trash preparation stopped"))?
                .map_err(rejected)?;
            let sync_source = vault.join(&source);
            let sync_trash = trash.clone();
            store
                .spawn_blocking(move || {
                    #[cfg(unix)]
                    if sync_source
                        .parent()
                        .ok_or_else(|| StoreError::Io("Missing session parent".into()))?
                        .metadata()
                        .map_err(io_error)?
                        .permissions()
                        .readonly()
                    {
                        return Err(StoreError::Io("Session parent is not writable".into()));
                    }
                    operation_io::sync_parent(&sync_source)?;
                    operation_io::sync_parent(&sync_trash)
                })
                .await
                .map_err(|_| uncertain("Lifecycle sync admission stopped"))?
                .map_err(rejected)?;
            let target = Target {
                session_id: input.session_id.clone(),
                source: source.clone(),
                trash: trash
                    .strip_prefix(&vault)
                    .map_err(|_| uncertain("Invalid trash destination"))?
                    .into(),
            };
            receipts::validate_target(&target, &token)?;
            receipt.target = Some(target.clone());
            receipt.prepared_revision = Some(tree.revision.clone());
            receipt.deleted_at = Some(chrono::Utc::now().to_rfc3339());
            receipts::persist(&store, &receipt_path, &receipt)
                .await
                .map_err(LifecycleError::Uncertain)?;
            checkpoint(Stage::Intent)
                .await
                .map_err(LifecycleError::Uncertain)?;
            let source_path = vault.join(&source);
            let moved = source_path.clone();
            let move_trash = trash.clone();
            let check_vault = vault.clone();
            let limits = store.content_limits;
            let _snapshot = store
                .spawn_blocking(move || {
                    if !tree.unchanged(&check_vault, &source, limits)? {
                        return Err(StoreError::Conflict(
                            "Session changed after deletion intent".into(),
                        ));
                    }
                    drop(tree);
                    hypr_storage::fs::rename_no_replace(&moved, &move_trash).map_err(io_error)?;
                    Ok(snapshot)
                })
                .await
                .map_err(|_| uncertain("Session move stopped"))?
                .map_err(LifecycleError::Uncertain)?;
            store.lifecycle_removed(&input.session_id).await;
            checkpoint(Stage::Renamed)
                .await
                .map_err(LifecycleError::Uncertain)?;
            sync_parents(&store, source_path, trash)
                .await
                .map_err(LifecycleError::Uncertain)?;
            checkpoint(Stage::Synced)
                .await
                .map_err(LifecycleError::Uncertain)?;
            let outcome = DeleteSessionOutcome::Trashed {
                trash_token: token,
                revision: input.expected_revision,
            };
            receipt.state = State::Completed {
                outcome: outcome.clone(),
            };
            receipts::persist(&store, &receipt_path, &receipt)
                .await
                .map_err(LifecycleError::Uncertain)?;
            checkpoint(Stage::Completed)
                .await
                .map_err(LifecycleError::Uncertain)?;
            Ok(outcome)
        })
        .await
        .map_err(|_| uncertain("Deletion worker stopped"))?
        .map_err(|cause| if has_intent { admitted(cause) } else { cause })
    }

    async fn lifecycle_removed(&self, id: &str) {
        self.live.lock().await.remove(id);
        self.deleted_sessions.lock().unwrap().insert(id.into());
        self.index_remove_session_and_notify(id);
    }
    async fn recover_session_delete(
        &self,
        mut receipt: DeleteReceipt,
        path: &Path,
        proof: Option<std::sync::Arc<local_proof::Inner>>,
    ) -> Result<DeleteSessionOutcome, LifecycleError> {
        match &receipt.state {
            State::Completed { outcome } => return Ok(outcome.clone()),
            State::Uncertain => return Err(uncertain("Deletion requires review")),
            State::Prepared => {}
        }
        let target = receipt
            .target
            .clone()
            .ok_or_else(|| uncertain("Incomplete deletion intent"))?;
        let source_exists = exists(self, self.vault_base.join(&target.source)).await?;
        let tree = if !source_exists {
            self.lifecycle_tree(
                target.trash.clone(),
                target.session_id.clone(),
                proof.clone(),
            )
            .await
            .map_err(admitted)?
        } else {
            None
        };
        if let Some(tree) =
            tree.filter(|tree| Some(&tree.revision) == receipt.prepared_revision.as_ref())
        {
            let (_snapshot, _tree) = self
                .lifecycle_recheck(&target.session_id, target.trash.clone(), tree)
                .await
                .map_err(admitted)?;
            if exists(self, self.vault_base.join(&target.source)).await? {
                receipt.state = State::Uncertain;
                receipts::persist(self, path, &receipt)
                    .await
                    .map_err(LifecycleError::Uncertain)?;
                return Err(uncertain(
                    "Original session destination changed during recovery",
                ));
            }
            self.lifecycle_removed(&target.session_id).await;
            sync_parents(
                self,
                self.vault_base.join(&target.source),
                self.vault_base.join(&target.trash),
            )
            .await
            .map_err(LifecycleError::Uncertain)?;
            let token = SessionLifecycleKey {
                vault_id: receipt.binding.vault_id.clone(),
                namespace: receipt.binding.namespace.clone(),
                request_id: uuid::Uuid::parse_str(&receipt.binding.request_id)
                    .map_err(|_| uncertain("Invalid deletion identity"))?,
            }
            .token();
            let outcome = DeleteSessionOutcome::Trashed {
                trash_token: token,
                revision: tree_revision(&receipt)?,
            };
            receipt.state = State::Completed {
                outcome: outcome.clone(),
            };
            receipts::persist(self, path, &receipt)
                .await
                .map_err(LifecycleError::Uncertain)?;
            return Ok(outcome);
        }
        receipt.state = State::Uncertain;
        receipts::persist(self, path, &receipt)
            .await
            .map_err(LifecycleError::Uncertain)?;
        Err(uncertain(
            "Interrupted deletion is ambiguous; original files were preserved",
        ))
    }
}

fn tree_revision<I, O>(receipt: &Receipt<I, O>) -> Result<String, LifecycleError> {
    receipt
        .prepared_revision
        .clone()
        .ok_or_else(|| uncertain("Lifecycle revision is missing"))
}
async fn exists(store: &SessionStore, path: PathBuf) -> Result<bool, LifecycleError> {
    store
        .spawn_blocking(move || match path.symlink_metadata() {
            Ok(_) => Ok(true),
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(cause) => Err(rejected(io_error(cause))),
        })
        .await
        .map_err(|_| uncertain("Lifecycle existence reader stopped"))?
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
        .map_err(|_| StoreError::Io("Lifecycle directory sync stopped".into()))?
}

impl SessionStore {
    pub async fn restore_session_checked(
        &self,
        root: &Path,
        documents: Option<&Path>,
        key: SessionLifecycleKey,
        input: RestoreSessionRequest,
        expected_target: Option<String>,
    ) -> Result<RestoreSessionOutcome, LifecycleError> {
        self.restore_session_with(root, documents, key, input, expected_target, |_| async {
            Ok(())
        })
        .await
    }
    async fn restore_session_with<C, F>(
        &self,
        root: &Path,
        documents: Option<&Path>,
        key: SessionLifecycleKey,
        input: RestoreSessionRequest,
        expected_target: Option<String>,
        mut checkpoint: C,
    ) -> Result<RestoreSessionOutcome, LifecycleError>
    where
        C: FnMut(Stage) -> F + Send + 'static,
        F: std::future::Future<Output = Result<(), StoreError>> + Send + 'static,
    {
        if let Some(outcome) = Self::session_restore_replay(root, &key, &input).await? {
            return Ok(outcome);
        }
        let has_intent = Self::session_restore_has_intent(root, &key, &input).await?;
        let store = self.clone();
        let root = root.to_path_buf();
        let documents = documents.map(Path::to_path_buf);
        tokio::spawn(async move {
            let binding = key.binding()?;
            let path = receipts::restoration_path(&root, &key);
            let token_path = receipts::deletion_path(&root, &input.trash_token)?;
            let deletion = receipts::read::<DeleteReceipt>(token_path.clone()).await?;
            if let Some(deletion) = &deletion {
                receipts::validate_delete(deletion, &key.vault_id, &input.trash_token)?;
            }
            let target = deletion.as_ref().and_then(|receipt| receipt.target.clone());
            if target.as_ref().map(|target| target.session_id.as_str())
                != expected_target.as_deref()
            {
                return Err(rejected(StoreError::Conflict(
                    "Restore target changed before admission".into(),
                )));
            }
            let mut receipt = Receipt {
                schema_version: 1,
                binding: binding.clone(),
                input: input.clone(),
                target: target.clone(),
                prepared_revision: None,
                deleted_at: None,
                state: State::Prepared,
            };
            let Some(target) = target else {
                let _snapshot = store.lock_snapshot().await;
                if let Some(outcome) = Self::session_restore_replay(&root, &key, &input).await? {
                    return Ok(outcome);
                }
                if Self::session_restore_target(&root, &key.vault_id, &input.trash_token)
                    .await?
                    .is_some()
                {
                    return Err(rejected(StoreError::Conflict(
                        "Restore target changed before admission".into(),
                    )));
                }
                receipt.state = State::Completed {
                    outcome: RestoreSessionOutcome::NotFound,
                };
                receipts::persist(&store, &path, &receipt)
                    .await
                    .map_err(rejected)?;
                return Ok(RestoreSessionOutcome::NotFound);
            };
            let _owner = store
                .lifecycle_owner(&target.session_id, documents.as_deref())
                .await?;
            if let Some(stored) = receipts::read::<RestoreReceipt>(path.clone()).await? {
                receipts::validate_restore(&stored, &binding, &input)?;
                if stored.target.as_ref() != Some(&target) {
                    return Err(uncertain("Restore target changed"));
                }
                return store.recover_session_restore(stored, &path).await;
            }
            let deletion = receipts::read::<DeleteReceipt>(token_path.clone())
                .await?
                .ok_or_else(|| uncertain("Deletion receipt disappeared"))?;
            receipts::validate_delete(&deletion, &key.vault_id, &input.trash_token)?;
            if deletion.target.as_ref() != Some(&target) {
                return Err(rejected(StoreError::Conflict(
                    "Restore target changed".into(),
                )));
            }
            if !matches!(
                deletion.state,
                State::Completed {
                    outcome: DeleteSessionOutcome::Trashed { .. }
                }
            ) {
                store
                    .recover_session_delete(deletion, &token_path, None)
                    .await?;
            }
            let trash = store
                .lifecycle_tree(target.trash.clone(), target.session_id.clone(), None)
                .await?;
            let (snapshot, trash) = if let Some(trash) = trash {
                let (guard, tree) = store
                    .lifecycle_recheck(&target.session_id, target.trash.clone(), trash)
                    .await?;
                (guard, Some(tree))
            } else {
                (store.lock_snapshot().await, None)
            };
            if let Some(outcome) = Self::session_restore_replay(&root, &key, &input).await? {
                return Ok(outcome);
            }
            let occupied = exists(&store, store.vault_base.join(&target.source)).await?;
            let outcome = if occupied {
                Some(RestoreSessionOutcome::Conflict {
                    reason: RestoreSessionConflictReason::DestinationOccupied,
                    current_revision: None,
                })
            } else if let Some(trash) = &trash {
                if trash.revision != input.expected_revision {
                    Some(RestoreSessionOutcome::Conflict {
                        reason: RestoreSessionConflictReason::TrashChanged,
                        current_revision: Some(trash.revision.clone()),
                    })
                } else {
                    None
                }
            } else {
                Some(RestoreSessionOutcome::NotFound)
            };
            if let Some(outcome) = outcome {
                receipt.state = State::Completed {
                    outcome: outcome.clone(),
                };
                receipts::persist(&store, &path, &receipt)
                    .await
                    .map_err(rejected)?;
                return Ok(outcome);
            }
            let trash = trash.unwrap();
            receipt.prepared_revision = Some(trash.revision.clone());
            receipts::persist(&store, &path, &receipt)
                .await
                .map_err(LifecycleError::Uncertain)?;
            checkpoint(Stage::Intent)
                .await
                .map_err(LifecycleError::Uncertain)?;
            let vault = store.vault_base.clone();
            let moved_target = target.clone();
            let limits = store.content_limits;
            let snapshot = store
                .spawn_blocking(move || {
                    if !trash.unchanged(&vault, &moved_target.trash, limits)? {
                        return Err(StoreError::Conflict(
                            "Trash changed after restore intent".into(),
                        ));
                    }
                    create_source_parent(&vault, &moved_target.source)?;
                    drop(trash);
                    hypr_storage::fs::rename_no_replace(
                        &vault.join(&moved_target.trash),
                        &vault.join(&moved_target.source),
                    )
                    .map_err(io_error)?;
                    Ok(snapshot)
                })
                .await
                .map_err(|_| uncertain("Restore move stopped"))?
                .map_err(LifecycleError::Uncertain)?;
            store
                .deleted_sessions
                .lock()
                .unwrap()
                .remove(&target.session_id);
            checkpoint(Stage::Renamed)
                .await
                .map_err(LifecycleError::Uncertain)?;
            sync_parents(
                &store,
                store.vault_base.join(&target.source),
                store.vault_base.join(&target.trash),
            )
            .await
            .map_err(LifecycleError::Uncertain)?;
            checkpoint(Stage::Synced)
                .await
                .map_err(LifecycleError::Uncertain)?;
            drop(snapshot);
            store
                .refresh_session(&target.session_id)
                .await
                .map_err(LifecycleError::Uncertain)?;
            let outcome = RestoreSessionOutcome::Restored {
                session_id: target.session_id,
                revision: input.expected_revision,
            };
            receipt.state = State::Completed {
                outcome: outcome.clone(),
            };
            receipts::persist(&store, &path, &receipt)
                .await
                .map_err(LifecycleError::Uncertain)?;
            checkpoint(Stage::Completed)
                .await
                .map_err(LifecycleError::Uncertain)?;
            Ok(outcome)
        })
        .await
        .map_err(|_| uncertain("Restore worker stopped"))?
        .map_err(|cause| if has_intent { admitted(cause) } else { cause })
    }

    async fn recover_session_restore(
        &self,
        mut receipt: RestoreReceipt,
        path: &Path,
    ) -> Result<RestoreSessionOutcome, LifecycleError> {
        if let Some(target) = &receipt.target {
            receipts::validate_target(target, &receipt.input.trash_token)?;
        }
        match &receipt.state {
            State::Completed { outcome } => return Ok(outcome.clone()),
            State::Uncertain => return Err(uncertain("Restore requires review")),
            State::Prepared => {}
        }
        let target = receipt
            .target
            .clone()
            .ok_or_else(|| uncertain("Incomplete restore intent"))?;
        let trash_exists = exists(self, self.vault_base.join(&target.trash)).await?;
        let tree = if !trash_exists {
            self.lifecycle_tree(target.source.clone(), target.session_id.clone(), None)
                .await
                .map_err(admitted)?
        } else {
            None
        };
        if let Some(tree) =
            tree.filter(|tree| Some(&tree.revision) == receipt.prepared_revision.as_ref())
        {
            let (snapshot, _tree) = self
                .lifecycle_recheck(&target.session_id, target.source.clone(), tree)
                .await
                .map_err(admitted)?;
            if exists(self, self.vault_base.join(&target.trash)).await? {
                receipt.state = State::Uncertain;
                receipts::persist(self, path, &receipt)
                    .await
                    .map_err(LifecycleError::Uncertain)?;
                return Err(uncertain(
                    "Trash destination changed during restore recovery",
                ));
            }
            self.deleted_sessions
                .lock()
                .unwrap()
                .remove(&target.session_id);
            sync_parents(
                self,
                self.vault_base.join(&target.source),
                self.vault_base.join(&target.trash),
            )
            .await
            .map_err(LifecycleError::Uncertain)?;
            drop(snapshot);
            self.refresh_session(&target.session_id)
                .await
                .map_err(LifecycleError::Uncertain)?;
            let outcome = RestoreSessionOutcome::Restored {
                session_id: target.session_id,
                revision: tree_revision(&receipt)?,
            };
            receipt.state = State::Completed {
                outcome: outcome.clone(),
            };
            receipts::persist(self, path, &receipt)
                .await
                .map_err(LifecycleError::Uncertain)?;
            return Ok(outcome);
        }
        receipt.state = State::Uncertain;
        receipts::persist(self, path, &receipt)
            .await
            .map_err(LifecycleError::Uncertain)?;
        Err(uncertain(
            "Interrupted restore is ambiguous; original files were preserved",
        ))
    }
}

fn create_source_parent(vault: &Path, source: &Path) -> Result<(), StoreError> {
    let parent = source
        .parent()
        .ok_or_else(|| StoreError::Io("Missing restore parent".into()))?;
    let mut path = vault.to_path_buf();
    for component in parent.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(StoreError::Io("Invalid restore parent".into()));
        }
        path.push(component);
        match std::fs::create_dir(&path) {
            Ok(()) => operation_io::sync_parent(&path)?,
            Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {
                let meta = path.symlink_metadata().map_err(io_error)?;
                if !meta.is_dir() || meta.file_type().is_symlink() {
                    return Err(StoreError::Io(
                        "Restore parent is not a regular directory".into(),
                    ));
                }
            }
            Err(cause) => return Err(io_error(cause)),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
