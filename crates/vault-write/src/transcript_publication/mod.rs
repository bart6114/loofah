//! Checked publication of an already-generated transcript. Ordinary server work
//! requires absence; explicit Local replacement binds the raw transcript revision
//! and retains previous bytes before atomic replacement. The host keeps its
//! immutable inference input and result, reauthorizes after `recheck`, and retains
//! Context/admission ownership through actual commit. No operation lock is held
//! by a source snapshot during inference. Provisional captions are not canonical.
//!
//! A pending receipt owns one hidden stage. Atomic exclusive rename consumes it;
//! recovery never recreates a missing pending stage from the retained result.
//! Completed receipts are historical decisions, not proof of current content.
//! Confirmation errors and busy recovery retain the original uncertainty: hosts
//! must keep source/results and reconcile the identical identity, never infer a
//! failed prior commit from a failed status read.
//! The server absent-only primitive retains its WAV source and output budgets;
//! trusted Local replacement does not inherit those format/request limits.
//! Directory durability uses existing Unix-only synchronization helpers.

use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

use hypr_fs_format::{TranscriptJson, TranscriptWithData};
use serde::{Deserialize, Serialize};

use crate::{
    SessionStore, StoreError, file_snapshot::FileSnapshot,
    transcript_operation::TranscriptOperation,
};

mod io;
mod manual_source;
mod source_guard;
pub use manual_source::{
    CompletedTranscriptPublication, TranscriptOnlyBinding, TranscriptOnlyGuard,
};
mod subtitle;
pub use source_guard::{ProcessingSourceBinding, ProcessingSourceGuard};
pub use subtitle::{SubtitleSourcePreparation, SubtitleSourceSnapshot};
#[cfg(test)]
mod tests;

use io::{Binding, Receipt, State};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TranscriptPublicationIdentity {
    pub vault_id: String,
    pub namespace: String,
    pub request_id: uuid::Uuid,
}

impl TranscriptPublicationIdentity {
    fn validate(&self) -> Result<(), StoreError> {
        if !valid_text(&self.vault_id, 512)
            || !valid_text(&self.namespace, 512)
            || self.request_id.is_nil()
        {
            return Err(StoreError::Serialize(
                "Invalid transcript publication identity".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct TranscriptPublicationBaseline(Arc<Baseline>);

struct Baseline {
    owner: Arc<tokio::sync::Mutex<()>>,
    binding: Binding,
    recording: Option<FileSnapshot>,
}

pub struct TranscriptSourceSnapshot {
    baseline: TranscriptPublicationBaseline,
}

impl TranscriptSourceSnapshot {
    pub fn baseline(&self) -> TranscriptPublicationBaseline {
        self.baseline.clone()
    }
    pub fn recording_file(&self) -> &File {
        self.baseline
            .0
            .recording
            .as_ref()
            .expect("audio-bound source")
            .file()
    }
    pub fn recording_sha256(&self) -> &str {
        &self.baseline.0.binding.recording_sha256
    }
    pub fn transcript_revision(&self) -> &str {
        &self.baseline.0.binding.transcript_revision
    }
    pub fn transcript_present(&self) -> bool {
        self.baseline.0.binding.transcript_present
    }
    pub fn recording_selection(&self) -> &str {
        &self.baseline.0.binding.recording_selection
    }
    pub fn audio(&self) -> hypr_vault_read::SessionAudio {
        self.baseline.0.binding.audio
    }
}

pub enum TranscriptSourcePreparation {
    Ready(Box<TranscriptSourceSnapshot>),
    NoRecording,
    Conflict,
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptPublicationOutcome {
    Published {
        revision: String,
        transcript_id: String,
    },
    AlreadyCommitted {
        revision: String,
        transcript_id: String,
    },
    Empty,
    Conflict,
    Busy,
    RequiresReview,
}

pub struct TranscriptPublicationPlan {
    store: SessionStore,
    root: PathBuf,
    directory: PathBuf,
    receipt: Receipt,
    result: Vec<u8>,
    empty: bool,
    summary: Option<crate::index::TranscriptSummary>,
    source: Option<FileSnapshot>,
    baseline: Option<TranscriptPublicationBaseline>,
    checked: bool,
    finished: Option<TranscriptPublicationOutcome>,
    _audio: Option<tokio::sync::OwnedMutexGuard<()>>,
    _transcript: Option<TranscriptOperation>,
}

impl SessionStore {
    pub async fn prepare_transcript_source(
        &self,
        id: &str,
    ) -> Result<Option<TranscriptSourcePreparation>, StoreError> {
        self.prepare_transcript_source_mode(id, false).await
    }

    pub async fn prepare_transcript_replacement_source(
        &self,
        id: &str,
    ) -> Result<Option<TranscriptSourcePreparation>, StoreError> {
        self.prepare_transcript_source_mode(id, true).await
    }

    async fn prepare_transcript_source_mode(
        &self,
        id: &str,
        replacement: bool,
    ) -> Result<Option<TranscriptSourcePreparation>, StoreError> {
        crate::validate_session_id(id)?;
        let store = self.clone();
        let id = id.to_owned();
        tokio::spawn(async move {
            let audio = match store.lock_session_audio(&id).await {
                Ok(audio) => audio,
                Err(StoreError::Conflict(_)) => return Ok(Some(TranscriptSourcePreparation::Busy)),
                Err(cause) => return Err(cause),
            };
            let transcript = store.transcript_operations.lock(&id).await;
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            let Some(meta) = store
                .metadata_snapshot_locked(&snapshot._guard, &id)
                .await?
            else {
                return Ok(None);
            };
            if store.deleted_sessions.lock().unwrap().contains(&id) {
                return Ok(None);
            }
            if busy(&store, &id, &meta.meta).await {
                return Ok(Some(TranscriptSourcePreparation::Busy));
            }
            let directory = store.vault_base.join(store.session_dir(&id).await?);
            let audio_metadata = match meta.meta.extra.get("audio") {
                Some(value) => serde_json::from_value(value.clone()).map_err(io::serialization)?,
                None => hypr_vault_read::SessionAudio::default(),
            };
            let initial = store
                .spawn_blocking(move || -> Result<_, StoreError> {
                    let _snapshot = snapshot;
                    let transcript_path = directory.join("transcript.json");
                    let (present, revision) = if replacement {
                        io::transcript_binding(&transcript_path, result_limit(true))?
                    } else {
                        if !io::absent(&transcript_path)? {
                            return Ok(Err(TranscriptSourcePreparation::Conflict));
                        }
                        (false, io::absent_revision())
                    };
                    let source = io::select_with_limit(&directory, source_limit(replacement))?;
                    Ok(source
                        .map(|(selection, recording)| (selection, recording, present, revision))
                        .ok_or(TranscriptSourcePreparation::NoRecording))
                })
                .await
                .map_err(io::error)??;
            let (selection, recording, transcript_present, transcript_revision) = match initial {
                Ok(source) => source,
                Err(outcome) => return Ok(Some(outcome)),
            };
            let owner = store.write_lock.clone();
            let baseline = store
                .spawn_blocking(move || -> Result<_, StoreError> {
                    let _audio = audio;
                    let _transcript = transcript;
                    if !recording.lock_for_recheck()? {
                        return Ok(None);
                    }
                    let sha256 = io::hash_bounded(recording.file(), source_limit(replacement))?;
                    let mut reader = recording.file();
                    std::io::Seek::rewind(&mut reader).map_err(io::error)?;
                    let current = FileSnapshot::new(io::clone_file(recording.file())?)?;
                    if !recording.unchanged(&current)? {
                        return Ok(None);
                    }
                    Ok(Some(TranscriptPublicationBaseline(Arc::new(Baseline {
                        owner,
                        binding: Binding {
                            session_id: id,
                            recording_selection: selection,
                            recording_sha256: sha256,
                            transcript_revision,
                            replacement,
                            transcript_present,
                            audio: audio_metadata,
                            subtitle: None,
                        },
                        recording: Some(recording),
                    }))))
                })
                .await
                .map_err(io::error)??;
            Ok(Some(match baseline {
                Some(baseline) => {
                    TranscriptSourcePreparation::Ready(Box::new(TranscriptSourceSnapshot {
                        baseline,
                    }))
                }
                None => TranscriptSourcePreparation::Busy,
            }))
        })
        .await
        .map_err(io::error)?
    }

    pub async fn prepare_transcript_publication(
        &self,
        operations: &Path,
        identity: TranscriptPublicationIdentity,
        baseline: TranscriptPublicationBaseline,
        generated: TranscriptWithData,
    ) -> Result<TranscriptPublicationPlan, StoreError> {
        identity.validate()?;
        if baseline.0.binding.subtitle.is_some()
            || !Arc::ptr_eq(&self.write_lock, &baseline.0.owner)
            || generated.session_id != baseline.0.binding.session_id
        {
            return Err(StoreError::Conflict(
                "Transcript baseline belongs to another store or session".into(),
            ));
        }
        if !operations.is_absolute() {
            return Err(StoreError::Serialize(
                "Transcript operation path must be absolute".into(),
            ));
        }
        validate_generated(&generated, !baseline.0.binding.replacement)?;
        let transcript_id = generated.id.clone();
        let store = self.clone();
        let root = operations.to_path_buf();
        let limit = result_limit(baseline.0.binding.replacement);
        tokio::spawn(async move {
            let result = store
                .spawn_blocking(move || {
                    let bytes = io::encode_result(
                        &TranscriptJson {
                            transcripts: vec![generated],
                        },
                        limit,
                    )?;
                    if bytes.len() > limit {
                        return Err(StoreError::ResourceLimit(
                            "Generated transcript exceeds its size limit".into(),
                        ));
                    }
                    Ok(bytes)
                })
                .await
                .map_err(io::error)??;
            let (receipt_path, result_path) = io::paths(&root, &identity);
            let wanted = Receipt {
                schema_version: 1,
                identity: identity.clone(),
                binding: baseline.0.binding.clone(),
                result_sha256: crate::sha256(&result),
                transcript_id,
                stage_name: io::stage_name(),
                state: State::Prepared,
            };
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            let receipt = store
                .spawn_blocking(move || {
                    let _snapshot = snapshot;
                    if let Some(existing) = io::read_receipt(&receipt_path, &identity)
                        .map_err(StoreError::after_commit)?
                    {
                        if existing.binding != wanted.binding
                            || existing.result_sha256 != wanted.result_sha256
                            || existing.transcript_id != wanted.transcript_id
                        {
                            return Err(StoreError::Conflict(
                                "Transcript publication request changed".into(),
                            ));
                        }
                        return Ok(existing);
                    }
                    io::save_result(&result_path, &result, limit)?;
                    io::persist(&receipt_path, &wanted)?;
                    Ok(wanted)
                })
                .await
                .map_err(io::error)??;
            prepare_plan(store, root, receipt, Some(baseline)).await
        })
        .await
        .map_err(io::error)?
    }

    /// Reloads only this immutable recorded result. It never calls an inference provider.
    pub async fn prepare_transcript_publication_status(
        &self,
        operations: &Path,
        identity: TranscriptPublicationIdentity,
    ) -> Result<Option<TranscriptPublicationPlan>, StoreError> {
        identity.validate()?;
        if !operations.is_absolute() {
            return Err(StoreError::Serialize(
                "Transcript operation path must be absolute".into(),
            ));
        }
        let store = self.clone();
        let root = operations.to_owned();
        let outcome = tokio::spawn(async move {
            let receipt_path = io::paths(&root, &identity).0;
            let receipt = store
                .spawn_blocking(move || io::read_receipt(&receipt_path, &identity))
                .await
                .map_err(io::error)??;
            let Some(receipt) = receipt else {
                return Ok(None);
            };
            prepare_plan(store, root, receipt, None).await.map(Some)
        })
        .await
        .map_err(io::error)?;
        outcome.map_err(StoreError::after_commit)
    }
}

async fn prepare_plan(
    store: SessionStore,
    root: PathBuf,
    receipt: Receipt,
    baseline: Option<TranscriptPublicationBaseline>,
) -> Result<TranscriptPublicationPlan, StoreError> {
    let directory = store.vault_base.join(crate::paths::validated_session_dir(
        &receipt.binding.session_id,
    )?);
    let finished = match receipt.state {
        State::Committed => Some(TranscriptPublicationOutcome::AlreadyCommitted {
            revision: receipt.result_sha256.clone(),
            transcript_id: receipt.transcript_id.clone(),
        }),
        State::Conflict => Some(TranscriptPublicationOutcome::Conflict),
        State::Empty => Some(TranscriptPublicationOutcome::Empty),
        State::Review => Some(TranscriptPublicationOutcome::RequiresReview),
        _ => None,
    };
    let mut plan = TranscriptPublicationPlan {
        store: store.clone(),
        root,
        directory,
        receipt,
        result: Vec::new(),
        empty: false,
        summary: None,
        source: None,
        baseline,
        checked: false,
        finished,
        _audio: None,
        _transcript: None,
    };
    if plan.finished.is_some() {
        return Ok(plan);
    }
    plan._audio = match store
        .lock_session_audio(&plan.receipt.binding.session_id)
        .await
    {
        Ok(audio) => Some(audio),
        Err(StoreError::Conflict(_)) => {
            plan.finished = Some(TranscriptPublicationOutcome::Busy);
            return Ok(plan);
        }
        Err(cause) => return Err(cause),
    };
    plan._transcript = Some(
        store
            .transcript_operations
            .lock(&plan.receipt.binding.session_id)
            .await,
    );
    let path = io::paths(&plan.root, &plan.receipt.identity).1;
    let result_sha256 = plan.receipt.result_sha256.clone();
    let transcript_id = plan.receipt.transcript_id.clone();
    let session_id = plan.receipt.binding.session_id.clone();
    let limit = result_limit(plan.receipt.binding.replacement);
    let subtitle = plan.receipt.binding.subtitle.is_some();
    let (result, empty, summary) = store
        .spawn_blocking(move || {
            let bytes = io::read(&path, limit)?.ok_or_else(|| {
                StoreError::Conflict("Retained transcript result disappeared".into())
            })?;
            if crate::sha256(&bytes) != result_sha256 {
                return Err(StoreError::Conflict(
                    "Retained transcript result changed".into(),
                ));
            }
            let raw = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&bytes)
                .map_err(io::serialization)?
                .0;
            let file: TranscriptJson = serde_json::from_value(raw).map_err(io::serialization)?;
            if (!subtitle && file.transcripts.len() != 1)
                || file
                    .transcripts
                    .iter()
                    .any(|transcript| transcript.session_id != session_id)
            {
                return Err(StoreError::Serialize(
                    "Invalid retained transcript result".into(),
                ));
            }
            let mut matching = file
                .transcripts
                .iter()
                .filter(|transcript| transcript.id == transcript_id);
            let appended = matching.next().ok_or_else(|| {
                StoreError::Conflict("Retained transcript identity changed".into())
            })?;
            if matching.next().is_some() {
                return Err(StoreError::Conflict(
                    "Duplicate retained transcript identity".into(),
                ));
            }
            validate_generated(appended, limit != usize::MAX)?;
            let empty = appended.words.is_empty();
            let summary = crate::transcript::summarize_transcript_bytes(&bytes)?;
            Ok((bytes, empty, summary))
        })
        .await
        .map_err(io::error)?
        .map_err(StoreError::after_commit)?;
    plan.result = result;
    plan.empty = empty;
    plan.summary = Some(summary);
    Ok(plan)
}

impl TranscriptPublicationPlan {
    pub fn result_identity(&self) -> (&str, &str) {
        (
            &self.receipt.binding.session_id,
            &self.receipt.transcript_id,
        )
    }

    pub async fn recheck(self) -> Result<Self, StoreError> {
        let admitted = self.receipt.state == State::Pending;
        tokio::spawn(async move { self.recheck_owned().await })
            .await
            .map_err(io::error)?
            .map_err(|cause| {
                if admitted {
                    cause.after_commit()
                } else {
                    cause
                }
            })
    }

    async fn recheck_owned(self) -> Result<Self, StoreError> {
        if self.finished.is_some() {
            return Ok(self);
        }
        let store = self.store.clone();
        let snapshot = store.lock_snapshot().await;
        store.ensure_ready()?;
        let Some(meta) = store
            .metadata_snapshot_locked(&snapshot._guard, &self.receipt.binding.session_id)
            .await?
        else {
            return Ok(self.finish(TranscriptPublicationOutcome::Conflict));
        };
        if !metadata_matches(&meta.meta, &self.receipt.binding)? {
            return Ok(self.finish(TranscriptPublicationOutcome::Conflict));
        }
        if busy(&store, &self.receipt.binding.session_id, &meta.meta).await {
            return Ok(self.finish(TranscriptPublicationOutcome::Busy));
        }
        if self.receipt.binding.subtitle.is_some() {
            drop(snapshot);
            let mut plan = self;
            plan.checked = true;
            return Ok(plan);
        }
        store
            .spawn_blocking(move || {
                let snapshot = snapshot;
                let mut plan = self;
                let current = io::select_with_limit(
                    &plan.directory,
                    source_limit(plan.receipt.binding.replacement),
                )?;
                drop(snapshot);
                let Some((selection, recording)) = current else {
                    return Ok(plan.finish(TranscriptPublicationOutcome::Conflict));
                };
                if selection != plan.receipt.binding.recording_selection
                    || !recording.lock_for_recheck()?
                {
                    return Ok(plan.finish(TranscriptPublicationOutcome::Conflict));
                }
                if let Some(baseline) = &plan.baseline {
                    let Some(original) = baseline.0.recording.as_ref() else {
                        return Ok(plan.finish(TranscriptPublicationOutcome::Conflict));
                    };
                    if !original.unchanged(&recording)? {
                        return Ok(plan.finish(TranscriptPublicationOutcome::Conflict));
                    }
                }
                let sha256 = io::hash_bounded(
                    recording.file(),
                    source_limit(plan.receipt.binding.replacement),
                )?;
                let after = FileSnapshot::new(io::clone_file(recording.file())?)?;
                if sha256 != plan.receipt.binding.recording_sha256
                    || !recording.unchanged(&after)?
                {
                    return Ok(plan.finish(TranscriptPublicationOutcome::Conflict));
                }
                plan.source = Some(recording);
                plan.checked = true;
                Ok(plan)
            })
            .await
            .map_err(io::error)?
    }

    pub async fn commit(self) -> Result<TranscriptPublicationOutcome, StoreError> {
        self.commit_with(|_| Ok(()))
            .await
            .map_err(StoreError::after_commit)
    }

    async fn commit_with(
        self,
        fault: impl Fn(u8) -> Result<(), StoreError> + Send + 'static,
    ) -> Result<TranscriptPublicationOutcome, StoreError> {
        tokio::spawn(async move {
            let mut plan = if self.checked || self.finished.is_some() {
                self
            } else {
                self.recheck().await?
            };
            if let Some(finished) = plan.finished.take() {
                return plan.record_refusal(finished).await;
            }
            let store = plan.store.clone();
            let snapshot = store.lock_snapshot().await;
            store.ensure_ready()?;
            let Some(meta) = store
                .metadata_snapshot_locked(&snapshot._guard, &plan.receipt.binding.session_id)
                .await?
            else {
                drop(snapshot);
                return plan
                    .record_refusal(TranscriptPublicationOutcome::Conflict)
                    .await;
            };
            if !metadata_matches(&meta.meta, &plan.receipt.binding)? {
                drop(snapshot);
                return plan
                    .record_refusal(TranscriptPublicationOutcome::Conflict)
                    .await;
            }
            if busy(&store, &plan.receipt.binding.session_id, &meta.meta).await {
                return Ok(TranscriptPublicationOutcome::Busy);
            }
            store
                .spawn_blocking(move || {
                    let _snapshot = snapshot;
                    plan.commit_sync(fault)
                })
                .await
                .map_err(io::error)?
        })
        .await
        .map_err(io::error)?
    }

    async fn record_refusal(
        mut self,
        outcome: TranscriptPublicationOutcome,
    ) -> Result<TranscriptPublicationOutcome, StoreError> {
        if outcome != TranscriptPublicationOutcome::Conflict
            || matches!(
                self.receipt.state,
                State::Committed | State::Conflict | State::Empty | State::Review
            )
        {
            return Ok(outcome);
        }
        let store = self.store.clone();
        let guard = store.lock_snapshot().await;
        store
            .spawn_blocking(move || {
                let _guard = guard;
                let path = io::paths(&self.root, &self.receipt.identity).0;
                if io::read_receipt(&path, &self.receipt.identity)?.as_ref() != Some(&self.receipt)
                {
                    return Ok(TranscriptPublicationOutcome::RequiresReview);
                }
                let outcome = if self.receipt.state == State::Pending {
                    self.receipt.state = State::Review;
                    TranscriptPublicationOutcome::RequiresReview
                } else {
                    self.receipt.state = State::Conflict;
                    outcome
                };
                io::persist(&path, &self.receipt)?;
                Ok(outcome)
            })
            .await
            .map_err(io::error)?
    }

    fn finish(mut self, outcome: TranscriptPublicationOutcome) -> Self {
        self.finished = Some(outcome);
        self
    }

    fn refuse_sync(
        &mut self,
        receipt_path: &Path,
    ) -> Result<TranscriptPublicationOutcome, StoreError> {
        let outcome = if self.receipt.state == State::Pending {
            self.receipt.state = State::Review;
            TranscriptPublicationOutcome::RequiresReview
        } else {
            self.receipt.state = State::Conflict;
            TranscriptPublicationOutcome::Conflict
        };
        io::persist(receipt_path, &self.receipt)?;
        Ok(outcome)
    }

    fn review_sync(
        &mut self,
        receipt_path: &Path,
    ) -> Result<TranscriptPublicationOutcome, StoreError> {
        self.receipt.state = State::Review;
        io::persist(receipt_path, &self.receipt)?;
        Ok(TranscriptPublicationOutcome::RequiresReview)
    }

    fn commit_sync(
        mut self,
        fault: impl Fn(u8) -> Result<(), StoreError>,
    ) -> Result<TranscriptPublicationOutcome, StoreError> {
        let limit = result_limit(self.receipt.binding.replacement);
        let receipt_path = io::paths(&self.root, &self.receipt.identity).0;
        if io::read_receipt(&receipt_path, &self.receipt.identity)?.as_ref() != Some(&self.receipt)
        {
            return Ok(TranscriptPublicationOutcome::RequiresReview);
        }
        if self
            .store
            .deleted_sessions
            .lock()
            .unwrap()
            .contains(&self.receipt.binding.session_id)
        {
            return self.refuse_sync(&receipt_path);
        }
        if self.receipt.binding.subtitle.is_none() {
            let current = io::select_with_limit(
                &self.directory,
                source_limit(self.receipt.binding.replacement),
            )?;
            if !matches!((&self.source,&current),(Some(before),Some((selection,after))) if selection == &self.receipt.binding.recording_selection && before.unchanged(after)?)
            {
                return self.refuse_sync(&receipt_path);
            }
        }
        let canonical = self.directory.join("transcript.json");
        let stage = self.directory.join(&self.receipt.stage_name);
        if self.receipt.state == State::Prepared {
            if io::transcript_binding(&canonical, limit)?
                != (
                    self.receipt.binding.transcript_present,
                    self.receipt.binding.transcript_revision.clone(),
                )
            {
                self.receipt.state = State::Conflict;
                io::persist(&receipt_path, &self.receipt)?;
                return Ok(TranscriptPublicationOutcome::Conflict);
            }
            if self.empty {
                self.receipt.state = State::Empty;
                io::persist(&receipt_path, &self.receipt)?;
                return Ok(TranscriptPublicationOutcome::Empty);
            }
            if io::absent(&stage)? {
                io::write_stage(&stage, &self.result)?;
            }
            if io::read(&stage, limit)?.as_deref() != Some(self.result.as_slice()) {
                return self.review_sync(&receipt_path);
            }
            io::sync_stage(&stage, limit)?;
            if self.receipt.binding.replacement && self.receipt.binding.transcript_present {
                let original = io::read(&canonical, limit)?.ok_or_else(|| {
                    StoreError::Conflict("Original transcript disappeared".into())
                })?;
                if crate::sha256(&original) != self.receipt.binding.transcript_revision {
                    return self.refuse_sync(&receipt_path);
                }
                io::save_result(
                    &io::replacement_backup(&self.root, &self.receipt.identity),
                    &original,
                    limit,
                )?;
            }
            fault(0)?;
            self.receipt.state = State::Pending;
            io::persist(&receipt_path, &self.receipt)?;
        }
        fault(1)?;
        let staged = io::snapshot_with_limit(&stage, limit)?;
        let target = io::snapshot_with_limit(&canonical, limit)?;
        match (staged, target) {
            (Some(staged), Some(target))
                if self.receipt.binding.replacement && self.receipt.binding.transcript_present =>
            {
                if io::hash_bounded(staged.file(), limit as u64)? != self.receipt.result_sha256
                    || io::hash_bounded(target.file(), limit as u64)?
                        != self.receipt.binding.transcript_revision
                    || io::read(
                        &io::replacement_backup(&self.root, &self.receipt.identity),
                        limit,
                    )?
                    .is_none_or(|bytes| {
                        crate::sha256(&bytes) != self.receipt.binding.transcript_revision
                    })
                {
                    return self.review_sync(&receipt_path);
                }
                // Original bytes are durable before replacing the checked canonical file.
                drop(staged);
                drop(target);
                hypr_storage::fs::rename_with_retry(&stage, &canonical).map_err(io::error)?;
            }
            (Some(staged), None) => {
                if self.receipt.binding.transcript_present {
                    return self.review_sync(&receipt_path);
                }
                if io::hash_bounded(staged.file(), limit as u64)? != self.receipt.result_sha256 {
                    return self.review_sync(&receipt_path);
                }
                drop(staged);
                hypr_storage::fs::rename_no_replace(&stage, &canonical).map_err(io::error)?;
            }
            (None, Some(target)) => {
                if io::hash_bounded(target.file(), limit as u64)? != self.receipt.result_sha256 {
                    return self.review_sync(&receipt_path);
                }
            }
            _ => return self.review_sync(&receipt_path),
        }
        crate::operation_io::sync_parent(&canonical)?;
        fault(2)?;
        let Some(confirmed) = io::read(&canonical, limit)? else {
            return self.review_sync(&receipt_path);
        };
        if crate::sha256(&confirmed) != self.receipt.result_sha256 {
            return self.review_sync(&receipt_path);
        }
        let summary = self
            .summary
            .take()
            .ok_or_else(|| StoreError::Io("Missing transcript projection".into()))?;
        self.receipt.state = State::Committed;
        io::persist(&receipt_path, &self.receipt)?;
        self.store.journal.record(
            &format!(
                "sessions/{}/transcript.json",
                self.receipt.binding.session_id
            ),
            &self.receipt.result_sha256,
        );
        self.store
            .index_set_transcript_summary(&self.receipt.binding.session_id, summary);
        self.store.notify_index_changed(
            crate::IndexEntity::Transcripts,
            vec![self.receipt.binding.session_id.clone()],
        );
        fault(3)?;
        Ok(TranscriptPublicationOutcome::Published {
            revision: self.receipt.result_sha256,
            transcript_id: self.receipt.transcript_id,
        })
    }
}

async fn busy(store: &SessionStore, id: &str, meta: &crate::SessionMeta) -> bool {
    store.is_recording(id)
        || meta
            .extra
            .get("audio_import_pending")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
        || store.live.lock().await.contains_key(id)
}

fn metadata_matches(meta: &crate::SessionMeta, binding: &Binding) -> Result<bool, StoreError> {
    match &binding.subtitle {
        Some(source) => Ok(meta.created_at == source.session_created_at),
        None => Ok(audio_metadata(meta)? == binding.audio),
    }
}

fn audio_metadata(meta: &crate::SessionMeta) -> Result<hypr_vault_read::SessionAudio, StoreError> {
    meta.extra
        .get("audio")
        .map(|value| serde_json::from_value(value.clone()).map_err(io::serialization))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn valid_text(value: &str, limit: usize) -> bool {
    !value.is_empty() && value.len() <= limit && !value.chars().any(char::is_control)
}

fn validate_generated(
    transcript: &TranscriptWithData,
    remote_limits: bool,
) -> Result<(), StoreError> {
    crate::validate_session_id(&transcript.session_id)?;
    if !valid_text(&transcript.id, 512)
        || (remote_limits && transcript.words.len() > 250_000)
        || (remote_limits && transcript.speaker_hints.len() > 500_000)
        || !transcript.started_at.is_finite()
        || transcript.started_at < 0.0
        || transcript
            .ended_at
            .is_some_and(|time| !time.is_finite() || time < transcript.started_at)
        || transcript.user_id.len() > 512
        || transcript.created_at.len() > 128
    {
        return Err(StoreError::Serialize("Invalid generated transcript".into()));
    }
    let mut words = std::collections::HashSet::new();
    for word in &transcript.words {
        let Some(id) = &word.id else {
            return Err(StoreError::Serialize(
                "Generated word is missing its identity".into(),
            ));
        };
        if !valid_text(id, 512)
            || !words.insert(id)
            || (remote_limits && word.text.len() > 64 * 1024)
            || !word.start_ms.is_finite()
            || word.start_ms < 0.0
            || !word.end_ms.is_finite()
            || word.end_ms < word.start_ms
            || !matches!(word.channel, 0.0 | 1.0 | 2.0)
        {
            return Err(StoreError::Serialize(
                "Invalid generated transcript word".into(),
            ));
        }
    }
    let mut hints = std::collections::HashSet::new();
    for hint in &transcript.speaker_hints {
        let Some(id) = &hint.id else {
            return Err(StoreError::Serialize(
                "Generated hint is missing its identity".into(),
            ));
        };
        if !valid_text(id, 512)
            || !hints.insert(id)
            || !words.contains(&hint.word_id)
            || !valid_text(&hint.hint_type, 256)
        {
            return Err(StoreError::Serialize(
                "Invalid generated transcript hint".into(),
            ));
        }
    }
    Ok(())
}

fn source_limit(replacement: bool) -> u64 {
    if replacement {
        u64::MAX
    } else {
        u32::MAX as u64
    }
}

fn result_limit(replacement: bool) -> usize {
    if replacement {
        usize::MAX
    } else {
        io::RESULT_LIMIT
    }
}
