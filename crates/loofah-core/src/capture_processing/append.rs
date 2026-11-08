use super::*;
use crate::transcription_jobs::ledger::{check_file, read, write_new};
use hypr_capture_store::capture_spool::CaptureSnapshot;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Mutex};

#[derive(Default)]
pub(super) struct Workers(Mutex<BTreeMap<String, Task>>);
struct Task {
    stop: CancellationToken,
    join: tokio::task::JoinHandle<Result<(), ApplicationError>>,
}
impl Workers {
    pub async fn shutdown(&self) -> Result<(), ApplicationError> {
        let tasks = std::mem::take(&mut *self.0.lock().map_err(storage)?);
        for task in tasks.values() {
            task.stop.cancel();
        }
        let mut failure = None;
        for (_, task) in tasks {
            if let Err(cause) = task.join.await.map_err(storage).and_then(|result| result) {
                failure.get_or_insert(cause);
            }
        }
        failure.map_or(Ok(()), Err)
    }
    async fn completed(&self, capture: &str) -> Result<bool, ApplicationError> {
        let task = {
            let mut tasks = self.0.lock().map_err(storage)?;
            if !tasks
                .get(capture)
                .is_some_and(|task| task.join.is_finished())
            {
                return Ok(false);
            }
            tasks.remove(capture).unwrap()
        };
        task.join.await.map_err(unknown)??;
        Ok(true)
    }
    pub async fn cancel(&self, capture: &str) -> Result<bool, ApplicationError> {
        {
            if let Some(task) = self.0.lock().map_err(storage)?.get(capture) {
                task.stop.cancel();
            } else {
                return Ok(true);
            }
        }
        self.completed(capture).await
    }
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Dispatch {
    version: u32,
    identity: CaptureIdentity,
    format: CaptureFormat,
    request: CaptureProcessingRequest,
    model: String,
    model_sha256: String,
    seal: CaptureSeal,
    suffix_sha256: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ResultFile {
    dispatch: Dispatch,
    transcript: hypr_loofah_protocol::TranscriptWithData,
}
pub(crate) struct Merged {
    pub bytes: Option<Vec<u8>>,
    pub publication: Option<TranscriptionPublication>,
    pub empty: bool,
}

pub(super) async fn step(
    context: &Arc<VaultContext>,
    service: &Arc<CaptureProcessingService>,
    record: &mut ledger::Record,
    source: &CaptureSnapshot,
    ready: bool,
) -> Result<bool, ApplicationError> {
    context.ensure_recording_publication_ready(&record.target_session)?;
    let Some(seal) = &source.seal else {
        record.snapshot.state = CaptureProcessingState::Waiting;
        return Ok(false);
    };
    service
        .append
        .completed(&record.identity.capture_id)
        .await?;
    if service
        .append
        .0
        .lock()
        .map_err(storage)?
        .contains_key(&record.identity.capture_id)
    {
        record.snapshot.state = CaptureProcessingState::Repairing;
        return Ok(false);
    }
    if let Some(merged) = result(context, record, seal).await? {
        if source.publication.is_none() {
            record.snapshot.state = CaptureProcessingState::WaitingForPublication;
            return Ok(false);
        }
        let canonical = source.publication.as_ref().unwrap();
        let guard = context
            .store
            .guard_processing_source(&record.target_session, None)
            .await
            .map_err(|cause| crate::vault::storage_error(cause, false))?
            .ok_or_else(|| conflict("Append canonical recording is missing"))?;
        if guard.binding().recording_sha256 != canonical.canonical_audio_sha256
            || guard.binding().transcript_present != merged.bytes.is_some()
            || guard.binding().transcript_revision != revision(merged.bytes.as_deref())
        {
            return Err(conflict(
                "Append compound result changed before processing completion",
            ));
        }
        record.transcript_ready = true;
        record.empty = merged.empty;
        record.snapshot.transcript_publication = merged.publication;
        return Ok(true);
    }
    let original = context
        .capture_append_source(&record.identity, true)
        .await?;
    let path = original.directory().join("core-inference-dispatch.json");
    let pin = context.clone();
    let dispatched = tokio::task::spawn_blocking(move || {
        let _pin = pin;
        check_file(&path)
    })
    .await
    .map_err(storage)??;
    if dispatched {
        return Err(unknown(
            "Append inference was admitted previously; retained output is required before recovery",
        ));
    }
    if !ready {
        return Ok(false);
    }
    if service.append.0.lock().map_err(storage)?.len() >= 2 {
        return Err(unavailable("Append inference capacity is busy"));
    }
    let source = context.prepare_capture_suffix(&record.identity).await?;
    let dispatch = Dispatch {
        version: 1,
        identity: record.identity.clone(),
        format: record.format.clone(),
        request: record.snapshot.request.clone(),
        model: record.snapshot.model.clone(),
        model_sha256: record.model_sha256.clone(),
        seal: source.seal.clone(),
        suffix_sha256: source.sha256.clone(),
    };
    let stop = service.stop.child_token();
    let stopping = stop.clone();
    let output = source.original.directory().join("core-suffix-result.json");
    let engine_input = crate::transcription_jobs::CaptureSuffixInput {
        identity: record.identity.clone(),
        options: record.snapshot.request.clone(),
        model: record.snapshot.model.clone(),
        source_sha256: source.sha256,
        path: source.path,
        original: source.original.clone(),
    };
    let prepared = context
        .prepare_capture_suffix_engine(&engine_input, stopping.clone())
        .await?;
    let path = source
        .original
        .directory()
        .join("core-inference-dispatch.json");
    let bytes = serde_json::to_vec(&dispatch).map_err(storage)?;
    let pin = context.clone();
    tokio::task::spawn_blocking(move || {
        let _pin = pin;
        write_new(&path, &bytes)
    })
    .await
    .map_err(storage)??;
    let owner = context.clone();
    let limit = result_limit(context);
    let join = tokio::spawn(async move {
        let _session = owner
            .admit_session_work(&dispatch.request_session())
            .await?;
        let transcript = owner
            .transcribe_capture_suffix(engine_input, prepared, stopping)
            .await
            .map_err(unknown)?;
        let result = ResultFile {
            dispatch,
            transcript,
        };
        let bytes = serde_json::to_vec(&result).map_err(storage)?;
        if bytes.len() as u64 > limit {
            return Err(unknown("Append result exceeds its retained output budget"));
        }
        tokio::task::spawn_blocking(move || {
            let _owner = owner;
            write_new(&output, &bytes)
        })
        .await
        .map_err(unknown)?
    });
    service
        .append
        .0
        .lock()
        .map_err(storage)?
        .insert(record.identity.capture_id.clone(), Task { stop, join });
    record.snapshot.state = CaptureProcessingState::Repairing;
    Ok(false)
}
impl Dispatch {
    fn request_session(&self) -> String {
        match &self.identity.target {
            CaptureTarget::AppendSession { session_id, .. } => session_id.clone(),
            _ => String::new(),
        }
    }
    fn validate(
        &self,
        record: &ledger::Record,
        seal: &CaptureSeal,
    ) -> Result<(), ApplicationError> {
        if self.version != 1
            || self.identity != record.identity
            || self.format != record.format
            || self.request != record.snapshot.request
            || self.model != record.snapshot.model
            || self.model_sha256 != record.model_sha256
            || &self.seal != seal
            || !is_transcription_sha256(&self.suffix_sha256)
        {
            return Err(conflict("Retained append inference binding changed"));
        }
        Ok(())
    }
}
async fn result(
    context: &Arc<VaultContext>,
    record: &ledger::Record,
    seal: &CaptureSeal,
) -> Result<Option<Merged>, ApplicationError> {
    let original = context
        .capture_append_source(&record.identity, false)
        .await?;
    let path = original.directory().join("core-suffix-result.json");
    let declared = original.directory().join("core-inference-dispatch.json");
    let expected = record.clone();
    let seal = seal.clone();
    let limit = result_limit(context);
    let pin = context.clone();
    tokio::task::spawn_blocking(move || {
        let _pin = pin;
        if !check_file(&path)? {
            return Ok(None);
        }
        let retained = read(&path, limit)?;
        let output: ResultFile = serde_json::from_slice(&retained).map_err(storage)?;
        if serde_json::to_vec(&output).map_err(storage)? != retained {
            return Err(conflict("Unsupported retained append result fields"));
        }
        output.dispatch.validate(&expected, &seal)?;
        let dispatch: Dispatch =
            serde_json::from_slice(&read(&declared, 65536)?).map_err(storage)?;
        if dispatch != output.dispatch {
            return Err(conflict("Append inference receipt changed"));
        }
        let CaptureTarget::AppendSession {
            admission,
            session_id,
        } = &expected.identity.target
        else {
            return Err(conflict("Append target changed"));
        };
        let bytes = crate::capture_append::merge_transcript(
            original.raw_transcript(),
            output.transcript,
            session_id,
            admission.baseline.frames,
            seal.frames,
        )?;
        let transcript: hypr_vault_read::TranscriptJson = match &bytes {
            Some(bytes) => serde_json::from_slice(bytes).map_err(storage)?,
            None => hypr_vault_read::TranscriptJson {
                transcripts: vec![],
            },
        };
        let selected = transcript
            .transcripts
            .iter()
            .rev()
            .find(|transcript| !transcript.words.is_empty());
        let publication = selected.map(|transcript| TranscriptionPublication {
            transcript_id: transcript.id.clone(),
            revision: revision(bytes.as_deref()),
        });
        Ok(Some(Merged {
            empty: publication.is_none(),
            publication,
            bytes,
        }))
    })
    .await
    .map_err(storage)?
}
impl VaultContext {
    pub(crate) async fn capture_append_merged(
        self: &Arc<Self>,
        identity: &CaptureIdentity,
        seal: &CaptureSeal,
    ) -> Result<Merged, ApplicationError> {
        let service = self.capture_processing_service()?;
        let record = service
            .journal
            .get(&identity.scope.member_id, &identity.capture_id)?;
        if record.identity != *identity
            || record.cancel_requested
            || !service.model_matches(&record)
        {
            return Err(conflict("Append processing is cancelled or changed"));
        }
        result(self, &record, seal).await?.ok_or_else(|| {
            unavailable("Append suffix processing must finish before compound publication")
        })
    }
}
fn result_limit(context: &VaultContext) -> u64 {
    (context.store.content_limits().recovery_bytes as u64).min(u64::MAX - 1)
}
fn revision(bytes: Option<&[u8]>) -> String {
    crate::transcription_jobs::ledger::hash(
        bytes.unwrap_or(b"loofah/transcript-revision/v1/absent"),
    )
}
fn unknown(cause: impl std::fmt::Display) -> ApplicationError {
    let mut cause = error(
        ErrorCode::Conflict,
        &format!("Append inference outcome requires review: {cause}"),
    );
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}

pub(super) async fn cancel(
    context: &Arc<VaultContext>,
    record: &ledger::Record,
) -> Result<bool, ApplicationError> {
    let service = context.capture_processing_service()?;
    if !service.append.cancel(&record.identity.capture_id).await? {
        return Ok(false);
    }
    let original = context
        .capture_append_source(&record.identity, false)
        .await?;
    let dispatch = original.directory().join("core-inference-dispatch.json");
    let result = original.directory().join("core-suffix-result.json");
    let pin = context.clone();
    tokio::task::spawn_blocking(move || {
        let _pin = pin;
        Ok(!check_file(&dispatch)? || check_file(&result)?)
    })
    .await
    .map_err(storage)?
}
