//! Owned append publication: immutable admission sources, retained combined output, and a
//! recoverable audio/transcript commit. Core owns decoding, offsets, authorization and models.
//! Pending compound writes fence audio/transcript access until their exact receipt is resolved.
mod io;
mod recovery;
mod source;
#[cfg(test)]
mod tests;
mod types;
pub use recovery::RecordingAppendRecovery;
pub use types::{
    AppendIdentity, AppendSource, PendingRecordingAppend, RecordingAppendArtifact,
    RecordingAppendOutcome,
};

use crate::{
    SessionStore, StoreError, file_snapshot::FileSnapshot,
    transcript_operation::TranscriptOperation,
};
use std::{
    collections::HashMap,
    fs::File,
    io::Seek,
    path::{Path, PathBuf},
    sync::Arc,
};
use types::*;

pub struct RecordingAppendPlan {
    store: SessionStore,
    root: PathBuf,
    directory: PathBuf,
    result: ResultReceipt,
    intent: Option<Intent>,
    checked_files: HashMap<PathBuf, (FileSnapshot, String)>,
    checked: bool,
    finished: Option<RecordingAppendOutcome>,
    _audio: Option<tokio::sync::OwnedMutexGuard<()>>,
    _transcript: Option<TranscriptOperation>,
}

impl SessionStore {
    pub(crate) fn ensure_append_ready(&self, id: &str) -> Result<(), StoreError> {
        if self
            .append_pending
            .lock()
            .map_err(|_| io::invalid("Append recovery inventory is unavailable"))?
            .contains_key(id)
        {
            return Err(io::invalid(
                "Recording append requires recovery before audio or transcript access",
            ));
        }
        Ok(())
    }
    pub(crate) async fn check_append_metadata_write(
        &self,
        id: &str,
        relative: &Path,
        bytes: &[u8],
    ) -> Result<(), StoreError> {
        if !self
            .append_pending
            .lock()
            .map_err(|_| io::invalid("Append recovery inventory is unavailable"))?
            .contains_key(id)
        {
            return Ok(());
        }
        let path = self.vault_base.join(relative);
        let bytes = bytes.to_vec();
        self.spawn_blocking(move || {
            let current = io::read(&path, io::METADATA_LIMIT)?
                .ok_or_else(|| io::invalid("Append metadata disappeared"))?;
            let current = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&current)
                .map_err(io::error)?
                .0;
            let proposed = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&bytes)
                .map_err(io::error)?
                .0;
            if [
                "id",
                "audio",
                "audio_import_pending",
                "started_at",
                "ended_at",
            ]
            .into_iter()
            .any(|key| current.get(key) != proposed.get(key))
            {
                return Err(io::invalid(
                    "Recording metadata cannot change during append recovery",
                ));
            }
            Ok(())
        })
        .await
        .map_err(io::error)?
    }
    fn set_append_pending(
        &self,
        identity: &AppendIdentity,
        id: &str,
        pending: bool,
    ) -> Result<(), StoreError> {
        let mut entries = self
            .append_pending
            .lock()
            .map_err(|_| io::invalid("Append recovery inventory is unavailable"))?;
        if pending {
            if entries
                .get(id)
                .is_some_and(|owner| *owner != identity.admission_id)
            {
                return Err(io::invalid("Another append requires recovery"));
            }
            entries.insert(id.into(), identity.admission_id);
        } else if entries.get(id) == Some(&identity.admission_id) {
            entries.remove(id);
        }
        Ok(())
    }
    pub async fn prepare_recording_append(
        &self,
        operations: &Path,
        identity: AppendIdentity,
        source: AppendSource,
        combined: File,
        artifact: RecordingAppendArtifact,
        merged: Option<Vec<u8>>,
    ) -> Result<RecordingAppendPlan, StoreError> {
        let root = io::root(operations, &identity)?;
        if !Arc::ptr_eq(&self.write_lock, &source.0.store.write_lock)
            || source.0.directory != root
            || source.0.receipt.identity != identity
        {
            return Err(io::invalid("Append source belongs to another owner"));
        }
        if merged
            .as_ref()
            .is_some_and(|bytes| bytes.len() as u64 > io::TRANSCRIPT_LIMIT)
        {
            return Err(io::invalid("Append transcript exceeds its byte budget"));
        }
        if merged.as_deref() != source.raw_transcript()
            && let Some(bytes) = merged.as_deref()
        {
            // Invalid retained originals may be preserved byte-for-byte, never manufactured.
            let value = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(bytes)
                .map_err(io::error)?
                .0;
            let parsed: hypr_fs_format::TranscriptJson =
                serde_json::from_value(value).map_err(io::error)?;
            if parsed
                .transcripts
                .iter()
                .any(|transcript| transcript.session_id != source.0.receipt.session_id)
            {
                return Err(io::invalid("Append transcript belongs to another session"));
            }
        }
        let result = ResultReceipt {
            schema_version: 1,
            source: source.0.receipt.clone(),
            artifact,
            transcript_present: merged.is_some(),
            transcript_revision: io::revision(merged.as_deref()),
        };
        io::validate_result(&result)?;
        if result.artifact.layout != result.source.audio.layout {
            return Err(io::invalid("Append must preserve original audio layout"));
        }
        prepare(
            self.clone(),
            root,
            identity,
            Some((result, combined, merged)),
        )
        .await
    }
    pub async fn prepare_recording_append_status(
        &self,
        operations: &Path,
        identity: AppendIdentity,
    ) -> Result<Option<RecordingAppendPlan>, StoreError> {
        let root = io::root(operations, &identity)?;
        let candidate = root.clone();
        if !self
            .spawn_blocking(move || io::json::<ResultReceipt>(&candidate.join("result.json")))
            .await
            .map_err(io::error)??
            .is_some()
        {
            return Ok(None);
        }
        prepare(self.clone(), root, identity, None).await.map(Some)
    }
}

async fn prepare(
    store: SessionStore,
    root: PathBuf,
    identity: AppendIdentity,
    new: Option<(ResultReceipt, File, Option<Vec<u8>>)>,
) -> Result<RecordingAppendPlan, StoreError> {
    tokio::spawn(async move {
        let initial_root = root.clone();
        let initial_identity = identity.clone();
        let existing = store
            .spawn_blocking(move || -> Result<_, StoreError> {
                let result = io::json::<ResultReceipt>(&initial_root.join("result.json"))?;
                if let Some(result) = &result {
                    io::validate_result(result)?;
                    if result.source.identity != initial_identity {
                        return Err(io::invalid("Append result owner changed"));
                    }
                }
                let intent = io::json::<Intent>(&initial_root.join("intent.json"))?;
                if let Some(intent) = &intent {
                    validate_intent(intent)?;
                    if result.as_ref() != Some(&intent.result) {
                        return Err(io::invalid("Append result no longer matches its intent"));
                    }
                }
                Ok((result, intent))
            })
            .await
            .map_err(io::error)??;
        let result = match (&existing.0, &new) {
            (Some(original), Some((request, _, _))) if original != request => {
                return Err(io::invalid("Append retry cannot replace retained output"));
            }
            (Some(original), _) => original.clone(),
            (None, Some((request, _, _))) => request.clone(),
            _ => return Err(io::invalid("Append result is unavailable")),
        };
        let id = result.source.session_id.clone();
        if existing
            .1
            .as_ref()
            .is_some_and(|intent| intent.state == IntentState::Committed)
        {
            store.set_append_pending(&identity, &id, false)?;
            return Ok(RecordingAppendPlan {
                store,
                root,
                directory: PathBuf::new(),
                result: result.clone(),
                intent: existing.1,
                checked_files: HashMap::new(),
                checked: true,
                finished: Some(outcome(&result, true)),
                _audio: None,
                _transcript: None,
            });
        }
        let audio = store.lock_session_audio_for_append(&id).await?;
        let transcript = store.transcript_operations.lock(&id).await;
        let snapshot = store.lock_snapshot().await;
        store.ensure_ready()?;
        let directory = store
            .vault_base
            .join(store.session_dir_locked(&snapshot._guard, &id).await?);
        if store.is_recording(&id) || store.live.lock().await.contains_key(&id) {
            return Err(io::invalid("Append session is busy"));
        }
        drop(snapshot);
        let worker = store.clone();
        store
            .spawn_blocking(move || {
                let source =
                    source::load(worker.clone(), root.clone(), &identity, Some(id.as_str()))?;
                if source.0.receipt != result.source {
                    return Err(io::invalid("Append original source changed"));
                }
                if let Some((_, mut combined, merged)) = new {
                    io::wave(combined.try_clone().map_err(io::error)?, &result.artifact)?;
                    combined.rewind().map_err(io::error)?;
                    io::copy(
                        &root.join("combined.wav"),
                        combined,
                        &result.artifact.sha256,
                        result.artifact.size_bytes,
                    )?;
                    if let Some(bytes) = merged {
                        io::retain_bytes(&root.join("transcript.after.json"), &bytes)?;
                    }
                    if existing.0.is_none() {
                        io::persist(&root.join("result.json"), &result)?;
                    }
                }
                let intent = existing.1;
                if intent.is_some() {
                    worker.set_append_pending(&identity, &id, true)?;
                }
                Ok(RecordingAppendPlan {
                    store: worker,
                    root,
                    directory,
                    result,
                    intent,
                    checked_files: HashMap::new(),
                    checked: false,
                    finished: None,
                    _audio: Some(audio),
                    _transcript: Some(transcript),
                })
            })
            .await
            .map_err(io::error)?
    })
    .await
    .map_err(io::error)?
}

impl RecordingAppendPlan {
    fn stage(&self) -> PathBuf {
        self.directory.join(format!(
            ".recording-append-{}.wav.tmp",
            self.result.source.identity.admission_id
        ))
    }
    fn retired(&self) -> PathBuf {
        self.directory.join(format!(
            ".recording-append-{}.original",
            self.result.source.identity.admission_id
        ))
    }
    fn original(&self) -> PathBuf {
        self.directory.join(&self.result.source.selection)
    }
    fn canonical(&self) -> PathBuf {
        self.directory.join("audio.wav")
    }
    pub async fn recheck(self) -> Result<Self, StoreError> {
        self.store
            .clone()
            .spawn_blocking(move || self.recheck_sync())
            .await
            .map_err(io::error)?
    }
    fn recheck_sync(mut self) -> Result<Self, StoreError> {
        if self.finished.is_some() {
            return Ok(self);
        }
        if self
            .intent
            .as_ref()
            .is_some_and(|intent| intent.state == IntentState::Review)
        {
            self.finished = Some(RecordingAppendOutcome::RequiresReview);
            return Ok(self);
        }
        io::wave(
            io::open(&self.root.join("combined.wav"))?
                .ok_or_else(|| io::invalid("Combined append result is missing"))?,
            &self.result.artifact,
        )?;
        let transcript = io::read(
            &self.root.join("transcript.after.json"),
            io::TRANSCRIPT_LIMIT,
        )?;
        if transcript.is_some() != self.result.transcript_present
            || io::revision(transcript.as_deref()) != self.result.transcript_revision
        {
            return Err(io::invalid("Retained append transcript changed"));
        }
        if self.intent.is_none() {
            if !source::matches_current(&self.directory, &self.result.source)? {
                self.finished = Some(RecordingAppendOutcome::Changed);
                return Ok(self);
            }
            let file = io::open(&self.root.join("combined.wav"))?
                .ok_or_else(|| io::invalid("Combined append result is missing"))?;
            io::copy(
                &self.stage(),
                file,
                &self.result.artifact.sha256,
                self.result.artifact.size_bytes,
            )?;
        }
        self.checked_files.clear();
        let mut paths = io::RECORDINGS
            .into_iter()
            .map(|name| self.directory.join(name))
            .collect::<Vec<_>>();
        paths.extend([self.stage(), self.retired()]);
        for path in paths {
            if let Some(file) = io::open(&path)? {
                let before = FileSnapshot::new(file)?;
                if !before.lock_for_recheck()? {
                    self.finished = Some(RecordingAppendOutcome::Busy);
                    return Ok(self);
                }
                let (sha, _) = io::hash(before.file(), io::AUDIO_LIMIT)?;
                self.checked_files.insert(path, (before, sha));
            }
        }
        self.checked = true;
        Ok(self)
    }
    pub async fn commit(self) -> Result<RecordingAppendOutcome, StoreError> {
        self.commit_with(|_| Ok(())).await
    }
    async fn commit_with(
        self,
        fault: impl Fn(u8) -> Result<(), StoreError> + Send + 'static,
    ) -> Result<RecordingAppendOutcome, StoreError> {
        tokio::spawn(async move {
            let mut plan = if self.checked || self.finished.is_some() {
                self
            } else {
                self.recheck().await?
            };
            if let Some(outcome) = plan.finished.take() {
                return Ok(outcome);
            }
            let snapshot = plan.store.lock_snapshot().await;
            plan.store.ensure_ready()?;
            if plan.store.is_recording(&plan.result.source.session_id)
                || plan
                    .store
                    .live
                    .lock()
                    .await
                    .contains_key(&plan.result.source.session_id)
            {
                return Ok(RecordingAppendOutcome::Busy);
            }
            let (plan, finished, fault) = plan
                .store
                .clone()
                .spawn_blocking(move || {
                    let _snapshot = snapshot;
                    let finished = plan.commit_sync(&fault)?;
                    Ok::<_, StoreError>((plan, finished, fault))
                })
                .await
                .map_err(io::error)??;
            if let Some(outcome) = finished {
                return Ok(outcome);
            }
            let mut plan = plan.recheck().await?;
            if let Some(outcome) = plan.finished.take() {
                return Ok(outcome);
            }
            let snapshot = plan.store.lock_snapshot().await;
            plan.store
                .clone()
                .spawn_blocking(move || {
                    let _snapshot = snapshot;
                    let result = plan.confirm_sync()?;
                    fault(6)?;
                    Ok(result)
                })
                .await
                .map_err(io::error)?
        })
        .await
        .map_err(io::error)?
    }
    fn review(&mut self) -> Result<RecordingAppendOutcome, StoreError> {
        if let Some(intent) = self.intent.as_mut() {
            intent.state = IntentState::Review;
            io::persist(&self.root.join("intent.json"), intent)?;
            self.store.set_append_pending(
                &self.result.source.identity,
                &self.result.source.session_id,
                true,
            )?;
            Ok(RecordingAppendOutcome::RequiresReview)
        } else {
            Ok(RecordingAppendOutcome::Changed)
        }
    }
    fn commit_sync(
        &mut self,
        fault: &impl Fn(u8) -> Result<(), StoreError>,
    ) -> Result<Option<RecordingAppendOutcome>, StoreError> {
        let id = self.result.source.session_id.clone();
        if self
            .store
            .deleted_sessions
            .lock()
            .map_err(io::error)?
            .contains(&id)
        {
            return self.review().map(Some);
        }
        let Some(metadata) = io::read(&self.directory.join("_meta.json"), io::METADATA_LIMIT)?
        else {
            return self.review().map(Some);
        };
        let raw = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&metadata)
            .map_err(io::error)?
            .0;
        let meta: crate::SessionMeta = serde_json::from_value(raw.clone()).map_err(io::error)?;
        let audio: hypr_vault_read::SessionAudio = raw
            .get("audio")
            .map(|audio| serde_json::from_value(audio.clone()).map_err(io::error))
            .transpose()?
            .unwrap_or_default();
        if meta.id != id
            || audio != self.result.source.audio
            || raw
                .get("audio_import_pending")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        {
            return self.review().map(Some);
        }
        if io::json::<ResultReceipt>(&self.root.join("result.json"))?.as_ref() != Some(&self.result)
            || io::json::<Intent>(&self.root.join("intent.json"))? != self.intent
        {
            return self.review().map(Some);
        }
        let paths = io::RECORDINGS
            .into_iter()
            .map(|name| self.directory.join(name))
            .chain([self.stage(), self.retired()])
            .collect::<Vec<_>>();
        for name in paths {
            let current = io::open(&name)?.map(FileSnapshot::new).transpose()?;
            match (self.checked_files.get(&name), current) {
                (Some((before, _)), Some(after)) if before.unchanged(&after)? => {}
                (None, None) => {}
                _ => return self.review().map(Some),
            }
        }
        let hashes = self
            .checked_files
            .iter()
            .map(|(path, (_, hash))| (path.clone(), hash.clone()))
            .collect::<HashMap<_, _>>();
        let old = &self.result.source.recording_sha256;
        let new = &self.result.artifact.sha256;
        let original = self.original();
        let retired = self.retired();
        let stage = self.stage();
        let canonical = self.canonical();
        let already_new = hashes.get(&canonical) == Some(new);
        let before = hashes.get(&original) == Some(old)
            && !hashes.contains_key(&retired)
            && hashes.get(&stage) == Some(new);
        let moved = hashes.get(&retired) == Some(old)
            && (!hashes.contains_key(&original) || original == canonical && already_new);
        let allowed = if before {
            vec![original.clone(), stage.clone()]
        } else if moved && already_new {
            vec![retired.clone(), canonical.clone()]
        } else if moved && !hashes.contains_key(&canonical) && hashes.get(&stage) == Some(new) {
            vec![retired.clone(), stage.clone()]
        } else {
            return self.review().map(Some);
        };
        if hashes.keys().any(|path| !allowed.contains(path)) {
            return self.review().map(Some);
        }
        let transcript = io::read(
            &self.directory.join("transcript.json"),
            io::TRANSCRIPT_LIMIT,
        )?;
        let transcript_before = transcript.is_some() == self.result.source.transcript_present
            && io::revision(transcript.as_deref()) == self.result.source.transcript_revision;
        let transcript_after = transcript.is_some() == self.result.transcript_present
            && io::revision(transcript.as_deref()) == self.result.transcript_revision;
        if (!transcript_before && !transcript_after)
            || (self.intent.is_none() && (!before || !transcript_before))
        {
            return self.review().map(Some);
        }
        if self.intent.is_none() {
            let intent = Intent {
                schema_version: 1,
                result: self.result.clone(),
                state: IntentState::Pending,
            };
            self.store
                .set_append_pending(&self.result.source.identity, &id, true)?;
            io::persist(&self.root.join("intent.json"), &intent)?;
            self.intent = Some(intent);
        }
        fault(1)?;
        // Windows must release verification handles before same-volume canonical renames.
        self.checked_files.clear();
        if before {
            hypr_storage::fs::rename_no_replace(&original, &retired).map_err(io::error)?;
            crate::operation_io::sync_parent(&original)?;
            crate::operation_io::sync_parent(&retired)?;
        }
        fault(2)?;
        if !already_new {
            hypr_storage::fs::rename_no_replace(&stage, &canonical).map_err(io::error)?;
            crate::operation_io::sync_parent(&canonical)?;
        }
        fault(3)?;
        if !transcript_after {
            let bytes = io::read(
                &self.root.join("transcript.after.json"),
                io::TRANSCRIPT_LIMIT,
            )?
            .ok_or_else(|| io::invalid("Retained append transcript missing"))?;
            if crate::sha256(&bytes) != self.result.transcript_revision {
                return self.review().map(Some);
            }
            hypr_storage::fs::atomic_write(
                &self.directory.join("transcript.json"),
                std::str::from_utf8(&bytes).map_err(io::error)?,
            )
            .map_err(io::error)?;
            crate::operation_io::sync_parent(&self.directory.join("transcript.json"))?;
        }
        fault(4)?;
        let legacy_directory = self.directory.join("audio");
        for directory in [&self.directory, &legacy_directory] {
            for name in ["audio.peaks.json", "audio.peaks.json.tmp"] {
                let path = directory.join(name);
                if io::open(&path)?.is_some() {
                    std::fs::remove_file(&path).map_err(io::error)?;
                    crate::operation_io::sync_parent(&path)?;
                }
            }
        }
        fault(5)?;
        Ok(None)
    }

    fn confirm_sync(mut self) -> Result<RecordingAppendOutcome, StoreError> {
        let id = self.result.source.session_id.clone();
        let paths = io::RECORDINGS
            .into_iter()
            .map(|name| self.directory.join(name))
            .chain([self.stage(), self.retired()])
            .collect::<Vec<_>>();
        for name in paths {
            let current = io::open(&name)?.map(FileSnapshot::new).transpose()?;
            match (self.checked_files.get(&name), current) {
                (Some((before, _)), Some(after)) if before.unchanged(&after)? => {}
                (None, None) => {}
                _ => return self.review(),
            }
        }
        if self.checked_files.len() != 2
            || self
                .checked_files
                .get(&self.canonical())
                .map(|(_, hash)| hash)
                != Some(&self.result.artifact.sha256)
            || self
                .checked_files
                .get(&self.retired())
                .map(|(_, hash)| hash)
                != Some(&self.result.source.recording_sha256)
            || io::json::<Intent>(&self.root.join("intent.json"))? != self.intent
        {
            return self.review();
        }
        let raw = io::read(&self.directory.join("_meta.json"), io::METADATA_LIMIT)?
            .ok_or_else(|| io::invalid("Append metadata disappeared"))?;
        let metadata = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&raw)
            .map_err(io::error)?
            .0;
        let audio: hypr_vault_read::SessionAudio = metadata
            .get("audio")
            .map(|value| serde_json::from_value(value.clone()).map_err(io::error))
            .transpose()?
            .unwrap_or_default();
        if audio != self.result.source.audio
            || metadata.get("id").and_then(serde_json::Value::as_str) != Some(id.as_str())
        {
            return self.review();
        }
        let current = io::read(
            &self.directory.join("transcript.json"),
            io::TRANSCRIPT_LIMIT,
        )?;
        if current.is_some() != self.result.transcript_present
            || io::revision(current.as_deref()) != self.result.transcript_revision
        {
            return self.review();
        }
        io::sync_file(&self.canonical())?;
        if current.is_some() {
            io::sync_file(&self.directory.join("transcript.json"))?;
        }
        crate::operation_io::sync_parent(&self.canonical())?;
        let intent = self.intent.as_mut().unwrap();
        intent.state = IntentState::Committed;
        io::persist(&self.root.join("intent.json"), intent)?;
        self.store.journal.record(
            &format!("sessions/{id}/audio.wav"),
            &self.result.artifact.sha256,
        );
        if let Some(bytes) = current {
            self.store.journal.record(
                &format!("sessions/{id}/transcript.json"),
                &self.result.transcript_revision,
            );
            if let Ok(summary) = crate::transcript::summarize_transcript_bytes(&bytes) {
                self.store.index_set_transcript_summary(&id, summary);
            }
        }
        self.store.notify_artifacts_changed(&id);
        self.store
            .notify_index_changed(crate::IndexEntity::Transcripts, vec![id.clone()]);
        self.store
            .set_append_pending(&self.result.source.identity, &id, false)?;
        Ok(outcome(&self.result, false))
    }
}
fn validate_intent(intent: &Intent) -> Result<(), StoreError> {
    io::validate_result(&intent.result)?;
    if intent.schema_version != 1 {
        return Err(io::invalid("Unknown append intent schema"));
    }
    Ok(())
}
fn outcome(result: &ResultReceipt, committed: bool) -> RecordingAppendOutcome {
    let artifact = &result.artifact;
    if committed {
        RecordingAppendOutcome::AlreadyCommitted {
            sha256: artifact.sha256.clone(),
            size_bytes: artifact.size_bytes,
            frames: artifact.frames,
            transcript_revision: result.transcript_revision.clone(),
            transcript_present: result.transcript_present,
        }
    } else {
        RecordingAppendOutcome::Published {
            sha256: artifact.sha256.clone(),
            size_bytes: artifact.size_bytes,
            frames: artifact.frames,
            transcript_revision: result.transcript_revision.clone(),
            transcript_present: result.transcript_present,
        }
    }
}
