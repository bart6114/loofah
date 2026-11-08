use super::{RecordingAppendPlan, io, prepare, types::*, validate_intent};
use crate::{SessionStore, StoreError};
use std::{path::Path, sync::Arc};

pub struct RecordingAppendRecovery(SessionStore);
impl RecordingAppendRecovery {
    pub async fn discover(
        &self,
        operations: &Path,
        vault: &str,
    ) -> Result<Vec<PendingRecordingAppend>, StoreError> {
        self.0.discover_recording_appends(operations, vault).await
    }
    pub async fn prepare(
        &self,
        operations: &Path,
        pending: PendingRecordingAppend,
    ) -> Result<RecordingAppendPlan, StoreError> {
        self.0
            .prepare_recording_append_recovery(operations, pending)
            .await
    }
}
impl SessionStore {
    pub fn recording_append_recovery(&self) -> Result<RecordingAppendRecovery, StoreError> {
        self.ensure_ready()?;
        Ok(RecordingAppendRecovery(self.clone()))
    }
    pub fn recording_append_startup_recovery(
        &self,
        owner: &Arc<std::fs::File>,
    ) -> Result<RecordingAppendRecovery, StoreError> {
        if !self
            .vault_lease
            .get()
            .is_some_and(|original| Arc::ptr_eq(original, owner))
        {
            return Err(io::invalid(
                "Append recovery requires the original vault owner",
            ));
        }
        let mut recovery = self.clone();
        recovery.startup_pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
        Ok(RecordingAppendRecovery(recovery))
    }
    pub async fn discover_recording_appends(
        &self,
        operations: &Path,
        vault: &str,
    ) -> Result<Vec<PendingRecordingAppend>, StoreError> {
        if !operations.is_absolute() || vault.is_empty() {
            return Err(io::invalid("Invalid append discovery scope"));
        }
        let operations = operations.to_owned();
        let vault = vault.to_owned();
        let store = self.clone();
        tokio::spawn(async move {
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            let worker = store.clone();
            store
                .spawn_blocking(move || {
                    let _snapshot = snapshot;
                    let root = operations.join("recording-append");
                    io::check(&root.join("probe"))?;
                    let entries = match std::fs::read_dir(&root) {
                        Ok(entries) => entries,
                        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
                            return Ok(vec![]);
                        }
                        Err(cause) => return Err(io::error(cause)),
                    };
                    let mut pending = vec![];
                    let mut count = 0;
                    let mut total = 0u64;
                    for entry in entries {
                        count += 1;
                        if count > 4096 {
                            return Err(io::invalid(
                                "Append discovery exceeds its directory budget",
                            ));
                        }
                        let entry = entry.map_err(io::error)?;
                        let directory = entry.path();
                        let kind = entry.file_type().map_err(io::error)?;
                        if !kind.is_dir()
                            || kind.is_symlink()
                            || !entry.file_name().to_str().is_some_and(io::valid_hash)
                        {
                            return Err(io::invalid("Append admission directory requires review"));
                        }
                        for name in ["source.json", "result.json", "intent.json"] {
                            if let Some(file) = io::open(&directory.join(name))? {
                                total = total
                                    .checked_add(file.metadata().map_err(io::error)?.len())
                                    .ok_or_else(|| {
                                        io::invalid("Append discovery byte budget overflow")
                                    })?;
                                if total > 64 * 1024 * 1024 {
                                    return Err(io::invalid(
                                        "Append discovery exceeds its receipt budget",
                                    ));
                                }
                            }
                        }
                        if let Some(source) =
                            io::json::<SourceReceipt>(&directory.join("source.json"))?
                        {
                            io::validate_source(&source, &source.identity, None)?;
                            if source.identity.vault_id != vault
                                || io::root(&operations, &source.identity)? != directory
                            {
                                return Err(io::invalid("Append discovery owner mismatch"));
                            }
                        }
                        let Some(intent) = io::json::<Intent>(&directory.join("intent.json"))?
                        else {
                            continue;
                        };
                        validate_intent(&intent)?;
                        if intent.result.source.identity.vault_id != vault
                            || io::root(&operations, &intent.result.source.identity)? != directory
                            || io::json::<SourceReceipt>(&directory.join("source.json"))?.as_ref()
                                != Some(&intent.result.source)
                            || io::json::<ResultReceipt>(&directory.join("result.json"))?.as_ref()
                                != Some(&intent.result)
                        {
                            return Err(io::invalid("Append recovery receipts diverged"));
                        }
                        let source = &intent.result.source;
                        worker.set_append_pending(
                            &source.identity,
                            &source.session_id,
                            intent.state != IntentState::Committed,
                        )?;
                        if intent.state != IntentState::Committed {
                            pending.push(PendingRecordingAppend {
                                store: worker.clone(),
                                directory,
                                intent,
                            });
                        }
                    }
                    Ok(pending)
                })
                .await
                .map_err(io::error)?
        })
        .await
        .map_err(io::error)?
    }
    pub async fn prepare_recording_append_recovery(
        &self,
        operations: &Path,
        pending: PendingRecordingAppend,
    ) -> Result<RecordingAppendPlan, StoreError> {
        if !Arc::ptr_eq(&self.write_lock, &pending.store.write_lock)
            || io::root(operations, pending.identity())? != pending.directory
        {
            return Err(io::invalid(
                "Append recovery belongs to another vault owner",
            ));
        }
        let expected = pending.intent.clone();
        let root = pending.directory.clone();
        self.spawn_blocking(move || {
            let current = io::json::<Intent>(&root.join("intent.json"))?
                .ok_or_else(|| io::invalid("Append recovery intent disappeared"))?;
            validate_intent(&current)?;
            if current.result != expected.result {
                return Err(io::invalid("Append recovery identity changed"));
            }
            Ok(())
        })
        .await
        .map_err(io::error)??;
        prepare(
            self.clone(),
            pending.directory,
            pending.intent.result.source.identity,
            None,
        )
        .await
    }
}
