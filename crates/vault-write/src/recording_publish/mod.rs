//! Absent-only recording publication. Core verifies the capture seal and creates WAV bytes;
//! this module verifies their bounded length/checksum and owns the canonical filesystem commit.
//! A failed or abandoned commit must be resolved with the same request and operation directory.
//! Receipt-bound staging files are retained for recovery; this operation never removes
//! the caller's capture source. It does not append, encode, transcribe, or authenticate.
//!
//! Core must verify the WAV's format/layout against the sealed source and supply an actual
//! first-audio producer wall-clock observation, not a staging or server-arrival time.
//! Artifacts are capped at the classic WAV byte limit, metadata at 8 MiB, and streaming
//! buffers at 64 KiB. The host owns request admission and exclusive vault ownership.
//! After restart the host must recover/gate pending publication before exposing that
//! session's recording or starting transcription. Recovery never replaces divergent data.
//! Pre-intent caller abandonment drops only its own temporary encoding. A process crash can
//! leave an unclaimed hidden stage, which is never guessed, reused or removed on retry.
//! Directory durability uses the existing platform helpers; Windows power-loss guarantees
//! are not stronger than those helpers, whose directory flush is currently Unix-only.

mod io;
#[cfg(test)]
mod tests;
mod types;

pub use types::{RecordingExpectation, RecordingPublication, RecordingPublicationRequest};

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use crate::{
    SessionStore, StoreError, file_snapshot::FileSnapshot,
    transcript_operation::TranscriptOperation,
};
use io::{Receipt, State};

pub struct RecordingPublicationPlan {
    store: SessionStore,
    request: RecordingPublicationRequest,
    receipt_path: PathBuf,
    directory: PathBuf,
    receipt: Option<Receipt>,
    artifact: Option<FileSnapshot>,
    stage: Option<io::Stage>,
    target: bool,
    checked: bool,
    finished: Option<RecordingPublication>,
    _audio: tokio::sync::OwnedMutexGuard<()>,
    _transcript: TranscriptOperation,
}

impl SessionStore {
    /// Preparation owns the reader and audio lease even if the requesting task is dropped.
    /// `source_binding` must canonically bind Core's authenticated capture identity and seal.
    pub async fn prepare_recording_publication(
        &self,
        operations: &Path,
        request: RecordingPublicationRequest,
        source: impl Read + Send + 'static,
    ) -> Result<RecordingPublicationPlan, StoreError> {
        prepare(
            self.clone(),
            operations.into(),
            request,
            Some(Box::new(source)),
        )
        .await?
        .ok_or_else(|| StoreError::Io("Missing recording publication plan".into()))
    }

    /// Loads a retained decision without reading or consuming another capture source.
    pub async fn prepare_recording_publication_status(
        &self,
        operations: &Path,
        request: RecordingPublicationRequest,
    ) -> Result<Option<RecordingPublicationPlan>, StoreError> {
        prepare(self.clone(), operations.into(), request, None).await
    }
}

async fn prepare(
    store: SessionStore,
    operations: PathBuf,
    request: RecordingPublicationRequest,
    source: Option<Box<dyn Read + Send>>,
) -> Result<Option<RecordingPublicationPlan>, StoreError> {
    request.validate()?;
    if !operations.is_absolute() {
        return Err(StoreError::Serialize(
            "Recording operations path must be absolute".into(),
        ));
    }
    tokio::spawn(async move {
        store.ensure_ready()?;
        let audio = store.lock_session_audio(&request.session_id).await?;
        let transcript = store.transcript_operations.lock(&request.session_id).await;
        let snapshot = store.lock_snapshot().await;
        store.ensure_ready()?;
        let directory = store
            .vault_base
            .join(store.session_dir(&request.session_id).await?);
        let deleted = store
            .deleted_sessions
            .lock()
            .unwrap()
            .contains(&request.session_id);
        let metadata = if deleted {
            None
        } else {
            read_current(&store, &directory, &request.session_id).await?
        };
        let busy = metadata
            .as_ref()
            .is_some_and(|raw| io::busy(&store, &request.session_id, raw));
        drop(snapshot);
        tokio::task::spawn_blocking(move || {
            let path = io::receipt_path(&operations, &request);
            let receipt = io::read(&path)?;
            if let Some(existing) = &receipt {
                existing.validate(&request)?;
            } else if source.is_none() {
                return Ok(None);
            }
            let mut plan = RecordingPublicationPlan {
                store,
                request,
                receipt_path: path,
                directory,
                receipt,
                artifact: None,
                stage: None,
                target: false,
                checked: false,
                finished: None,
                _audio: audio,
                _transcript: transcript,
            };
            if metadata.is_none() {
                plan.finished = Some(if plan.receipt.is_some() {
                    RecordingPublication::Uncertain
                } else {
                    RecordingPublication::Changed
                });
                return Ok(Some(plan));
            }
            if busy {
                plan.finished = Some(RecordingPublication::Busy);
                return Ok(Some(plan));
            }
            io::regular_directory(&plan.directory)?;
            if let Some(receipt) = plan.receipt.as_ref() {
                let stage = io::Stage::from_receipt(&plan.directory, receipt)?;
                let staged = io::open_snapshot(stage.path())?;
                let target = io::open_snapshot(&plan.directory.join("audio.wav"))?;
                let pending = receipt.state == State::Pending;
                plan.stage = Some(stage);
                match (staged, target) {
                    (Some(file), None) if pending => plan.artifact = Some(file),
                    (None, Some(file)) => {
                        plan.artifact = Some(file);
                        plan.target = true;
                    }
                    _ => plan.finished = Some(RecordingPublication::Uncertain),
                }
            } else {
                if !io::absent(&plan.directory, None)? {
                    plan.finished = Some(RecordingPublication::Changed);
                    return Ok(Some(plan));
                }
                let stage = io::write_stage(&plan.directory, source.unwrap(), &plan.request)?;
                plan.artifact = io::open_snapshot(stage.path())?;
                plan.stage = Some(stage);
            }
            Ok(Some(plan))
        })
        .await
        .map_err(io::worker_error)?
    })
    .await
    .map_err(io::worker_error)?
}

impl RecordingPublicationPlan {
    /// Hashing and Windows file-lock acquisition happen outside the store write guard.
    /// Core can recheck authorization after this call and hold its grant through `commit`.
    pub async fn recheck(self) -> Result<Self, StoreError> {
        tokio::task::spawn_blocking(move || self.recheck_sync())
            .await
            .map_err(io::worker_error)?
    }

    fn recheck_sync(mut self) -> Result<Self, StoreError> {
        if self.finished.is_some() {
            return Ok(self);
        }
        let Some(file) = &self.artifact else {
            self.finished = Some(RecordingPublication::Uncertain);
            return Ok(self);
        };
        if !file.lock_for_recheck()? {
            self.finished = Some(RecordingPublication::Busy);
            return Ok(self);
        }
        if !io::matches(file, &self.request)?
            || !io::absent(&self.directory, self.target.then_some("audio.wav"))?
        {
            self.finished = Some(if self.receipt.is_some() {
                RecordingPublication::Uncertain
            } else {
                RecordingPublication::Changed
            });
        }
        self.checked = true;
        Ok(self)
    }

    /// Errors after an intent is persisted may have committed canonical bytes. Retry/status
    /// with the identical request; never interpret an error as permission to delete the source.
    pub async fn commit(self) -> Result<RecordingPublication, StoreError> {
        self.commit_with(|_| Ok(())).await
    }

    async fn commit_with(
        self,
        fault: impl Fn(u8) -> Result<(), StoreError> + Send + 'static,
    ) -> Result<RecordingPublication, StoreError> {
        tokio::spawn(async move {
            let mut plan = if self.checked || self.finished.is_some() {
                self
            } else {
                self.recheck().await?
            };
            if let Some(finished) = plan.finished.take() {
                return Ok(finished);
            }
            let snapshot = plan.store.lock_snapshot().await;
            plan.store.ensure_ready()?;
            let current =
                read_current(&plan.store, &plan.directory, &plan.request.session_id).await?;
            let Some(raw) = current else {
                return Ok(if plan.receipt.is_some() {
                    RecordingPublication::Uncertain
                } else {
                    RecordingPublication::Changed
                });
            };
            if io::busy(&plan.store, &plan.request.session_id, &raw) {
                return Ok(RecordingPublication::Busy);
            }
            let limit = plan
                .store
                .content_limits
                .metadata_bytes
                .min(8 * 1024 * 1024);
            let result = tokio::task::spawn_blocking(move || {
                let _snapshot = snapshot;
                plan.commit_sync(raw, limit, &fault)
                    .map(|step| (step, fault))
            })
            .await
            .map_err(io::worker_error)??;
            let (step, fault) = result;
            match step {
                CommitStep::Finished(outcome) => Ok(outcome),
                CommitStep::Confirm(plan) => {
                    let fault = tokio::task::spawn_blocking(move || {
                        fault(6)?;
                        Ok::<_, StoreError>(fault)
                    })
                    .await
                    .map_err(io::worker_error)??;
                    // A canonical checksum can take seconds. Keep the audio lease, not the
                    // store-wide write guard, while hashing the published artifact.
                    let mut plan = (*plan).recheck().await?;
                    if let Some(outcome) = plan.finished.take() {
                        return Ok(outcome);
                    }
                    let snapshot = plan.store.lock_snapshot().await;
                    plan.store.ensure_ready()?;
                    let result = tokio::task::spawn_blocking(move || {
                        let _snapshot = snapshot;
                        plan.confirm_sync(fault)
                    })
                    .await
                    .map_err(io::worker_error)??;
                    Ok(result)
                }
            }
        })
        .await
        .map_err(io::worker_error)?
    }

    fn commit_sync(
        mut self,
        raw: serde_json::Value,
        limit: usize,
        fault: &impl Fn(u8) -> Result<(), StoreError>,
    ) -> Result<CommitStep, StoreError> {
        io::regular_directory(&self.directory)?;
        if io::read_metadata(&self.directory, limit)?.as_ref() != Some(&raw) {
            return Ok(CommitStep::Finished(if self.receipt.is_some() {
                RecordingPublication::Uncertain
            } else {
                RecordingPublication::Changed
            }));
        }
        let current_receipt = io::read(&self.receipt_path)?;
        if current_receipt != self.receipt {
            return Ok(CommitStep::Finished(RecordingPublication::Uncertain));
        }
        let stage = self
            .stage
            .as_ref()
            .ok_or_else(|| StoreError::Io("Missing recording stage ownership".into()))?
            .path()
            .to_path_buf();
        let canonical = self.directory.join("audio.wav");
        let current = io::open_snapshot(if self.target { &canonical } else { &stage })?;
        if !matches!((&self.artifact,&current),(Some(before),Some(after)) if before.unchanged(after)?)
            || !io::absent(&self.directory, self.target.then_some("audio.wav"))?
        {
            return Ok(CommitStep::Finished(if self.receipt.is_some() {
                RecordingPublication::Uncertain
            } else {
                RecordingPublication::Changed
            }));
        }
        let before = io::json_hash(&raw)?;
        let planned = io::patch_metadata(raw.clone(), &self.request)?;
        let planned_bytes = serde_json::to_vec_pretty(&planned).map_err(io::serialize_error)?;
        if planned_bytes.len() > limit {
            return Err(StoreError::ResourceLimit(
                "Session metadata exceeds its size limit".into(),
            ));
        }
        let after = io::json_hash(&planned)?;
        if let Some(receipt) = &self.receipt {
            if receipt.state == State::Uncertain {
                return Ok(CommitStep::Finished(RecordingPublication::Uncertain));
            }
            if receipt.state == State::Committed {
                if !self.target || !io::owned_metadata_matches(&raw, receipt)? {
                    return Ok(CommitStep::Finished(RecordingPublication::Changed));
                }
                // A previous receipt flush may have succeeded before its caller saw an
                // error. Replay must also repair that process's unconfirmed projection.
                self.publish_index(&raw)?;
                return Ok(CommitStep::Finished(self.request.published()));
            }
            if receipt.state == State::MetadataWritten {
                if !self.target || !io::owned_metadata_matches(&raw, receipt)? {
                    return Ok(CommitStep::Finished(RecordingPublication::Uncertain));
                }
                self.artifact = current;
                self.checked = false;
                return Ok(CommitStep::Confirm(Box::new(self)));
            }
            if before != receipt.before_metadata_sha256 && before != receipt.after_metadata_sha256 {
                return Ok(CommitStep::Finished(RecordingPublication::Uncertain));
            }
            if before == receipt.before_metadata_sha256 && after != receipt.after_metadata_sha256 {
                return Ok(CommitStep::Finished(RecordingPublication::Uncertain));
            }
        } else {
            let receipt = Receipt {
                schema_version: 1,
                request: self.request.clone(),
                stage_name: self.stage.as_ref().unwrap().name()?.into(),
                before_metadata_sha256: before.clone(),
                after_metadata_sha256: after,
                published_started_at: planned["started_at"]
                    .as_str()
                    .ok_or_else(|| {
                        StoreError::Serialize("Recording start is not a timestamp".into())
                    })?
                    .into(),
                published_ended_at: planned
                    .get("ended_at")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                state: State::Pending,
            };
            receipt.validate(&self.request)?;
            // Retain before persisting: an fsync error can leave a visible valid intent.
            self.stage.as_mut().unwrap().retain();
            io::persist(&self.receipt_path, &receipt)?;
            self.receipt = Some(receipt);
        }
        fault(1)?;
        if !self.target {
            // Windows must close its shared verification handle before moving this file.
            drop(current);
            self.artifact.take();
            hypr_storage::fs::rename_no_replace(&stage, &canonical).map_err(io::io_error)?;
            io::sync_parent(&canonical)?;
        }
        fault(2)?;
        io::invalidate_peaks(&self.directory)?;
        fault(3)?;
        let receipt = self.receipt.as_mut().unwrap();
        let latest = io::read_metadata(&self.directory, limit)?
            .ok_or_else(|| StoreError::Io("Recording metadata disappeared".into()))?;
        let latest_hash = io::json_hash(&latest)?;
        if latest_hash != receipt.before_metadata_sha256
            && latest_hash != receipt.after_metadata_sha256
        {
            return Ok(CommitStep::Finished(RecordingPublication::Uncertain));
        }
        if latest_hash != receipt.after_metadata_sha256 {
            hypr_storage::fs::atomic_write(
                &self.directory.join("_meta.json"),
                std::str::from_utf8(&planned_bytes)
                    .map_err(|cause| StoreError::Serialize(cause.to_string()))?,
            )
            .map_err(io::io_error)?;
            io::sync_parent(&self.directory.join("_meta.json"))?;
            self.store.journal.record(
                &format!("sessions/{}/_meta.json", self.request.session_id),
                &crate::sha256(&planned_bytes),
            );
        }
        receipt.state = State::MetadataWritten;
        io::persist(&self.receipt_path, receipt)?;
        fault(4)?;
        self.artifact = io::open_snapshot(&canonical)?;
        self.target = true;
        self.checked = false;
        Ok(CommitStep::Confirm(Box::new(self)))
    }

    fn confirm_sync(
        mut self,
        fault: impl Fn(u8) -> Result<(), StoreError>,
    ) -> Result<RecordingPublication, StoreError> {
        let canonical = self.directory.join("audio.wav");
        let current = io::open_snapshot(&canonical)?;
        if !matches!((&self.artifact,&current),(Some(before),Some(after)) if before.unchanged(after)?)
            || !io::absent(&self.directory, Some("audio.wav"))?
        {
            return Ok(RecordingPublication::Uncertain);
        }
        let confirmed = io::read_metadata(
            &self.directory,
            self.store
                .content_limits
                .metadata_bytes
                .min(8 * 1024 * 1024),
        )?
        .ok_or_else(|| StoreError::Io("Recording metadata disappeared".into()))?;
        let receipt = self.receipt.as_mut().unwrap();
        if io::busy(&self.store, &self.request.session_id, &confirmed)
            || !io::owned_metadata_matches(&confirmed, receipt)?
            || io::read(&self.receipt_path)?.as_ref() != Some(receipt)
        {
            return Ok(RecordingPublication::Uncertain);
        }
        io::sync_file(&canonical)?;
        io::sync_file(&self.directory.join("_meta.json"))?;
        io::sync_parent(&canonical)?;
        receipt.state = State::Committed;
        io::persist(&self.receipt_path, receipt)?;
        self.publish_index(&confirmed)?;
        fault(5)?;
        Ok(self.request.published())
    }
    fn publish_index(&self, raw: &serde_json::Value) -> Result<(), StoreError> {
        let meta: crate::SessionMeta =
            serde_json::from_value(raw.clone()).map_err(io::serialize_error)?;
        self.store.index_upsert_meta(&meta);
        self.store
            .notify_index_changed(crate::IndexEntity::Sessions, vec![meta.id.clone()]);
        self.store.notify_artifacts_changed(&meta.id);
        Ok(())
    }
}

enum CommitStep {
    Finished(RecordingPublication),
    Confirm(Box<RecordingPublicationPlan>),
}

async fn read_current(
    store: &SessionStore,
    directory: &Path,
    id: &str,
) -> Result<Option<serde_json::Value>, StoreError> {
    let path = directory.to_path_buf();
    let limit = store.content_limits.metadata_bytes.min(8 * 1024 * 1024);
    let id = id.to_owned();
    tokio::task::spawn_blocking(move || {
        let Some(raw) = io::read_metadata(&path, limit)? else {
            return Ok(None);
        };
        let meta: crate::SessionMeta =
            serde_json::from_value(raw.clone()).map_err(io::serialize_error)?;
        if meta.id != id {
            return Err(StoreError::Serialize(
                "Session metadata identity does not match its directory".into(),
            ));
        }
        Ok(Some(raw))
    })
    .await
    .map_err(io::worker_error)?
}
