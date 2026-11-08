use super::*;
use hypr_vault_read::TranscriptJson;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};
use tokio::sync::{Notify, watch};

const RECORD_LIMIT: u64 = 64 * 1024;
const ENTRY_LIMIT: usize = 4096;
#[cfg(test)]
#[path = "origin_tests.rs"]
mod origin_tests;

#[derive(Clone)]
pub(super) struct Journal(Arc<Inner>);
struct Inner {
    state: Mutex<State>,
    pub(super) notify: Notify,
    sources: Mutex<Vec<(String, watch::Receiver<bool>)>>,
    _lease: File,
    #[cfg(test)]
    fault: Mutex<Option<Arc<dyn Fn(&str) + Send + Sync>>>,
}
struct State {
    root: PathBuf,
    vault: String,
    records: BTreeMap<String, Record>,
    limits: TranscriptionLimits,
    audio_bytes: u64,
    failed: bool,
    generation: u64,
    events: crate::events::EventHub,
    active: BTreeMap<String, CancellationToken>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum Action {
    Generate,
    Publish,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResultRef {
    pub sha256: String,
    pub bytes: u64,
    pub transcript_id: String,
    pub started_at_ms: f64,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    schema_version: u32,
    vault_id: String,
    pub member: String,
    request_id: String,
    pub request: TranscriptionJobRequest,
    pub snapshot: TranscriptionJobSnapshot,
    pub action: Action,
    pub uncertain: bool,
    pub source: Option<source::Binding>,
    pub result: Option<ResultRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_processing: Option<crate::capture_processing::CaptureChildOrigin>,
}
impl Record {
    pub fn work(&self) -> TranscriptionWork {
        TranscriptionWork {
            job_id: self.snapshot.id.clone(),
            principal: self.member.clone(),
            request: self.request.clone(),
        }
    }
    pub fn identity(
        &self,
    ) -> hypr_vault_write::transcript_publication::TranscriptPublicationIdentity {
        hypr_vault_write::transcript_publication::TranscriptPublicationIdentity {
            vault_id: self.vault_id.clone(),
            namespace: self.member.clone(),
            request_id: uuid::Uuid::parse_str(&self.request_id).expect("validated request UUID"),
        }
    }
}

impl Journal {
    pub(super) async fn retry_unstarted(
        &self,
        member: String,
        id: String,
    ) -> Result<(), ApplicationError> {
        self.run(move |state| {
            let mut record = state
                .records
                .get(&id)
                .filter(|r| r.member == member)
                .cloned()
                .ok_or_else(unavailable)?;
            if state.active.contains_key(&id) {
                return Err(unavailable());
            }
            if record.snapshot.state != TranscriptionJobState::Failed
                || record.action != Action::Generate
                || record.source.is_some()
                || record.result.is_some()
                || record.uncertain
                || record.snapshot.result_available
                || !matches!(
                    record.snapshot.failure,
                    Some(
                        TranscriptionFailure::EngineUnavailable
                            | TranscriptionFailure::ResourceLimit
                    )
                )
            {
                return Err(error(
                    ErrorCode::Conflict,
                    "Inference attempt is not safely retryable",
                ));
            }
            record.snapshot.state = TranscriptionJobState::Queued;
            record.snapshot.failure = None;
            record.snapshot.phase = None;
            state.update(record)
        })
        .await
    }
    pub async fn open(
        root: PathBuf,
        events: crate::events::EventHub,
        limits: TranscriptionLimits,
        context: Option<Arc<VaultContext>>,
    ) -> Result<Self, ApplicationError> {
        if !(1..=256).contains(&limits.max_jobs)
            || !(1..=16).contains(&limits.max_queued)
            || limits.private_audio_bytes == 0
            || limits.private_audio_bytes > 8 * 1024 * 1024 * 1024
        {
            return Err(capacity());
        }
        tokio::task::spawn_blocking(move || {
            let _context = context;
            ensure_directory(&root)?;
            let lease_path = root.join(".lock");
            check_file(&lease_path)?;
            let lease = File::options()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(lease_path)
                .map_err(storage)?;
            lease.try_lock().map_err(|_| {
                error(
                    ErrorCode::Conflict,
                    "Transcription jobs already have an owner",
                )
            })?;
            let mut state = State {
                root,
                vault: events.vault_id(),
                records: BTreeMap::new(),
                limits,
                audio_bytes: 0,
                failed: false,
                generation: 0,
                events,
                active: BTreeMap::new(),
            };
            for (index, entry) in std::fs::read_dir(&state.root).map_err(storage)?.enumerate() {
                if index >= ENTRY_LIMIT {
                    return Err(capacity());
                }
                let entry = entry.map_err(storage)?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                let meta = std::fs::symlink_metadata(entry.path()).map_err(storage)?;
                if meta.is_file() && (name.contains(".source.") || name.ends_with(".source.tmp")) {
                    state.audio_bytes = state
                        .audio_bytes
                        .checked_add(meta.len())
                        .ok_or_else(capacity)?;
                }
                let Some(stem) = name.strip_suffix(".json").filter(|stem| uuid(stem).is_ok())
                else {
                    continue;
                };
                if state.records.len() >= limits.max_jobs {
                    return Err(capacity());
                }
                let record: Record =
                    serde_json::from_slice(&read(&entry.path(), RECORD_LIMIT)?).map_err(storage)?;
                validate(&record, &state.vault, stem)?;
                if state.records.values().any(|other| {
                    other.member == record.member && other.request_id == record.request_id
                }) {
                    return Err(storage("duplicate transcription request"));
                }
                state.records.insert(stem.to_owned(), record);
            }
            if state.audio_bytes > limits.private_audio_bytes {
                return Err(capacity());
            }
            // A retained matching result may be reconciled, but no inference is replayed.
            let before: Vec<_> = state.records.values().cloned().collect();
            for mut record in before {
                if record.result.is_some() {
                    let available = read_result(&state.root, &record)?.is_some();
                    if record.snapshot.result_available && !available {
                        return Err(storage("confirmed transcription result is missing"));
                    }
                    record.snapshot.result_available = available;
                }
                if matches!(
                    record.snapshot.state,
                    TranscriptionJobState::Running | TranscriptionJobState::Cancelling
                ) {
                    record.snapshot.state = if record.uncertain {
                        TranscriptionJobState::Uncertain
                    } else {
                        TranscriptionJobState::Interrupted
                    };
                    record.snapshot.phase = None;
                    record.snapshot.failure = Some(TranscriptionFailure::Interrupted);
                }
                if state.records[&record.snapshot.id] != record {
                    state.update(record)?;
                }
            }
            Ok(Self(Arc::new(Inner {
                state: Mutex::new(state),
                notify: Notify::new(),
                sources: Mutex::new(Vec::new()),
                _lease: lease,
                #[cfg(test)]
                fault: Mutex::new(None),
            })))
        })
        .await
        .map_err(storage)?
    }
    pub fn notify(&self) -> &Notify {
        &self.0.notify
    }
    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut State) -> Result<T, ApplicationError> + Send + 'static,
    ) -> Result<T, ApplicationError> {
        let inner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut state = inner.state.lock().map_err(storage)?;
            if state.failed {
                return Err(unavailable());
            }
            let before = state.generation;
            let result = operation(&mut state);
            if state.generation != before {
                inner.notify.notify_one();
            }
            result
        })
        .await
        .map_err(storage)?
    }
    #[cfg(test)]
    pub async fn submit(
        &self,
        member: String,
        request_id: String,
        request: TranscriptionJobRequest,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        self.submit_with_origin(member, request_id, request, None)
            .await
    }
    pub(super) async fn submit_with_origin(
        &self,
        member: String,
        request_id: String,
        request: TranscriptionJobRequest,
        origin: Option<crate::capture_processing::CaptureChildOrigin>,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        let member = uuid(&member)?;
        let request_id = uuid(&request_id)?;
        if let Some(origin) = &origin {
            origin.validate_request(&request)?;
        }
        self.run(move |s| {
            if let Some(existing) = s
                .records
                .values()
                .find(|r| r.member == member && r.request_id == request_id)
            {
                return if existing.request == request
                    && origin
                        .as_ref()
                        .is_none_or(|origin| existing.capture_processing.as_ref() == Some(origin))
                {
                    Ok(existing.snapshot.clone())
                } else {
                    Err(error(ErrorCode::Conflict, "Transcription request changed"))
                };
            }
            if s.records.len() >= s.limits.max_jobs
                || s.records
                    .values()
                    .filter(|r| !r.snapshot.state.is_terminal())
                    .count()
                    >= s.limits.max_queued
            {
                return Err(capacity());
            }
            let now = chrono::Utc::now().to_rfc3339();
            let id = uuid::Uuid::new_v4().to_string();
            let snapshot = TranscriptionJobSnapshot {
                id: id.clone(),
                session_id: request.session_id.clone(),
                state: TranscriptionJobState::Queued,
                phase: None,
                created_at: now.clone(),
                updated_at: now,
                result_available: false,
                failure: None,
                publication: None,
            };
            s.update(Record {
                schema_version: 1,
                vault_id: s.vault.clone(),
                member,
                request_id,
                request,
                snapshot: snapshot.clone(),
                action: Action::Generate,
                uncertain: false,
                source: None,
                result: None,
                capture_processing: origin,
            })?;
            Ok(snapshot)
        })
        .await
    }
    pub async fn record(&self, member: String, job: String) -> Result<Record, ApplicationError> {
        let member = uuid(&member)?;
        let job = uuid(&job)?;
        self.run(move |s| {
            s.records
                .get(&job)
                .filter(|r| r.member == member)
                .cloned()
                .ok_or_else(|| error(ErrorCode::NotFound, "Transcription job not found"))
        })
        .await
    }
    pub async fn owned(
        &self,
        member: String,
        job: String,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        Ok(self.record(member, job).await?.snapshot)
    }
    pub async fn list(
        &self,
        member: String,
        input: ListTranscriptionJobsInput,
    ) -> Result<TranscriptionJobPage, ApplicationError> {
        let member = uuid(&member)?;
        let limit = input.limit.unwrap_or(50) as usize;
        if !(1..=200).contains(&limit) {
            return Err(error(
                ErrorCode::Validation,
                "Invalid transcription page size",
            ));
        }
        let after = input.after.map(|s| uuid(&s)).transpose()?;
        self.run(move |s| {
            let mut jobs: Vec<_> = s
                .records
                .values()
                .filter(|r| {
                    r.member == member && after.as_ref().is_none_or(|id| &r.snapshot.id > id)
                })
                .map(|r| r.snapshot.clone())
                .collect();
            let next_cursor = (jobs.len() > limit).then(|| jobs[limit - 1].id.clone());
            jobs.truncate(limit);
            Ok(TranscriptionJobPage { jobs, next_cursor })
        })
        .await
    }
    pub async fn cancel(
        &self,
        member: String,
        job: String,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        let record = self.record(member, job).await?;
        self.run(move |s| {
            let mut r = s.records[&record.snapshot.id].clone();
            r.snapshot.state = match r.snapshot.state {
                TranscriptionJobState::Queued => TranscriptionJobState::Cancelled,
                TranscriptionJobState::Running => TranscriptionJobState::Cancelling,
                _ => return Ok(r.snapshot),
            };
            r.snapshot.phase = None;
            let snapshot = r.snapshot.clone();
            s.update(r)?;
            if let Some(cancel) = s.active.get(&record.snapshot.id) {
                cancel.cancel();
            }
            Ok(snapshot)
        })
        .await
    }
    pub async fn resume(
        &self,
        member: String,
        job: String,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        let record = self.record(member, job).await?;
        self.run(move |s| {
            let mut r = s.records[&record.snapshot.id].clone();
            if matches!(
                r.snapshot.state,
                TranscriptionJobState::Succeeded | TranscriptionJobState::Empty
            ) {
                return Ok(r.snapshot);
            }
            if !r.snapshot.state.is_terminal() || !r.snapshot.result_available {
                return Err(error(
                    ErrorCode::Conflict,
                    "No retained result can be resumed",
                ));
            }
            if s.records
                .values()
                .filter(|r| !r.snapshot.state.is_terminal())
                .count()
                >= s.limits.max_queued
            {
                return Err(capacity());
            }
            r.action = Action::Publish;
            r.snapshot.state = TranscriptionJobState::Queued;
            r.snapshot.failure = None;
            r.snapshot.phase = None;
            let snapshot = r.snapshot.clone();
            s.update(r)?;
            Ok(snapshot)
        })
        .await
    }
    pub async fn claim(
        &self,
        cancel: CancellationToken,
    ) -> Result<Option<Record>, ApplicationError> {
        self.run(move |s| {
            let Some(mut r) = s
                .records
                .values()
                .filter(|r| r.snapshot.state == TranscriptionJobState::Queued)
                .min_by(|a, b| {
                    (&a.snapshot.created_at, &a.snapshot.id)
                        .cmp(&(&b.snapshot.created_at, &b.snapshot.id))
                })
                .cloned()
            else {
                return Ok(None);
            };
            r.snapshot.state = TranscriptionJobState::Running;
            r.snapshot.phase = Some(TranscriptionPhase::Preparing);
            s.update(r.clone())?;
            s.active.insert(r.snapshot.id.clone(), cancel);
            Ok(Some(r))
        })
        .await
    }
    pub async fn change(
        &self,
        id: String,
        change: impl FnOnce(&mut Record) + Send + 'static,
    ) -> Result<Record, ApplicationError> {
        self.run(move |s| {
            let mut r = s.records.get(&id).cloned().ok_or_else(unavailable)?;
            change(&mut r);
            s.update(r.clone())?;
            Ok(r)
        })
        .await
    }
    pub async fn finish(
        &self,
        id: String,
        state: TranscriptionJobState,
        failure: Option<TranscriptionFailure>,
        publication: Option<TranscriptionPublication>,
    ) -> Result<(), ApplicationError> {
        self.run(move |s| {
            let mut r = s.records.get(&id).cloned().ok_or_else(unavailable)?;
            if state == TranscriptionJobState::Uncertain {
                r.uncertain = true;
            }
            let state = if r.uncertain
                && matches!(
                    state,
                    TranscriptionJobState::Failed
                        | TranscriptionJobState::Cancelled
                        | TranscriptionJobState::Interrupted
                ) {
                TranscriptionJobState::Uncertain
            } else {
                state
            };
            if matches!(
                state,
                TranscriptionJobState::Succeeded | TranscriptionJobState::Empty
            ) {
                r.uncertain = false;
            }
            r.snapshot.state = state;
            r.snapshot.phase = None;
            r.snapshot.failure = failure;
            r.snapshot.publication = publication;
            s.update(r)?;
            s.active.remove(&id);
            Ok(())
        })
        .await
    }
    pub async fn interrupt(&self) -> Result<(), ApplicationError> {
        self.run(|s| {
            let active: Vec<_> = s
                .records
                .values()
                .filter(|r| {
                    matches!(
                        r.snapshot.state,
                        TranscriptionJobState::Running | TranscriptionJobState::Cancelling
                    )
                })
                .cloned()
                .collect();
            for mut r in active {
                r.snapshot.state = if r.uncertain {
                    TranscriptionJobState::Uncertain
                } else {
                    TranscriptionJobState::Interrupted
                };
                r.snapshot.phase = None;
                r.snapshot.failure = Some(TranscriptionFailure::Interrupted);
                s.update(r)?;
            }
            Ok(())
        })
        .await
    }
    pub async fn root(&self) -> Result<PathBuf, ApplicationError> {
        self.run(|s| Ok(s.root.clone())).await
    }
    pub async fn reserve_audio(&self, bytes: u64) -> Result<(), ApplicationError> {
        self.run(move |s| {
            let next = s.audio_bytes.checked_add(bytes).ok_or_else(capacity)?;
            if next > s.limits.private_audio_bytes {
                return Err(capacity());
            }
            s.audio_bytes = next;
            Ok(())
        })
        .await
    }
    pub async fn result(
        &self,
        record: &Record,
    ) -> Result<Option<TranscriptWithData>, ApplicationError> {
        let record = record.clone();
        let root = self.root().await?;
        let owner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let _owner = owner;
            read_result(&root, &record)
        })
        .await
        .map_err(storage)?
    }
    pub async fn retain_result(
        &self,
        id: String,
        transcript: TranscriptWithData,
    ) -> Result<Record, ApplicationError> {
        let (bytes, reference) = tokio::task::spawn_blocking(move || {
            let bytes = encode_result(&transcript)?;
            let reference = ResultRef {
                sha256: hash(&bytes),
                bytes: bytes.len() as u64,
                transcript_id: transcript.id,
                started_at_ms: transcript.started_at,
            };
            Ok::<_, ApplicationError>((bytes, reference))
        })
        .await
        .map_err(storage)??;
        let record = self
            .change(id.clone(), move |r| r.result = Some(reference))
            .await?;
        self.fault("result_intent");
        let root = self.root().await?;
        tokio::task::spawn_blocking(move || {
            write_new(&root.join(format!("{id}.result.json")), &bytes)
        })
        .await
        .map_err(storage)??;
        self.fault("result_file");
        self.change(record.snapshot.id, |r| r.snapshot.result_available = true)
            .await
    }
    pub fn fault(&self, point: &str) {
        #[cfg(test)]
        if let Some(hook) = self.0.fault.lock().unwrap().clone() {
            hook(point);
        }
        #[cfg(not(test))]
        let _ = point;
    }
    #[cfg(test)]
    pub fn set_fault(&self, hook: Arc<dyn Fn(&str) + Send + Sync>) {
        *self.0.fault.lock().unwrap() = Some(hook);
    }
    pub fn track_source(&self, session: String, receipt: watch::Receiver<bool>) {
        let mut sources = self.0.sources.lock().unwrap();
        sources.retain(|(_, source)| !*source.borrow());
        sources.push((session, receipt));
    }
    pub(super) fn ensure_sources_released(&self, session: &str) -> Result<(), ApplicationError> {
        let sources = self.0.sources.lock().map_err(storage)?;
        if sources
            .iter()
            .any(|(id, released)| id == session && !*released.borrow())
        {
            return Err(crate::session_lifecycle_gate::session_lifecycle_busy());
        }
        Ok(())
    }
    pub async fn drain_sources(&self) {
        let sources = std::mem::take(&mut *self.0.sources.lock().unwrap());
        for (_, mut source) in sources {
            loop {
                if *source.borrow_and_update() {
                    break;
                }
                if source.changed().await.is_err() {
                    tracing::error!("native source channel closed without release evidence");
                    std::future::pending::<()>().await;
                }
            }
        }
    }
}

impl State {
    fn update(&mut self, mut record: Record) -> Result<(), ApplicationError> {
        record.snapshot.updated_at = chrono::Utc::now().to_rfc3339();
        if record.snapshot.state == TranscriptionJobState::Uncertain {
            record.uncertain = true;
        }
        validate(&record, &self.vault, &record.snapshot.id)?;
        if let Err(cause) = persist(&self.root, &record, self.records.get(&record.snapshot.id)) {
            self.failed = true;
            return Err(cause);
        }
        self.generation = self.generation.wrapping_add(1);
        self.events.publish_for(
            &record.member,
            Change::JobChanged {
                job_id: record.snapshot.id.clone(),
            },
        );
        self.records.insert(record.snapshot.id.clone(), record);
        Ok(())
    }
}

fn validate(r: &Record, vault: &str, id: &str) -> Result<(), ApplicationError> {
    if let Some(origin) = &r.capture_processing {
        origin.validate_request(&r.request)?;
    }
    if r.schema_version != 1
        || r.vault_id != vault
        || r.snapshot.id != id
        || !uuid(id).is_ok_and(|canonical| canonical == id)
        || !uuid(&r.member).is_ok_and(|canonical| canonical == r.member)
        || !uuid(&r.request_id).is_ok_and(|canonical| canonical == r.request_id)
        || r.snapshot.session_id != r.request.session_id
        || validate_transcription_job_request(&r.request).is_err()
        || chrono::DateTime::parse_from_rfc3339(&r.snapshot.created_at).is_err()
        || chrono::DateTime::parse_from_rfc3339(&r.snapshot.updated_at).is_err()
    {
        return Err(storage("invalid transcription record"));
    }
    if let Some(source) = &r.source {
        source.validate(&r.request)?;
    }
    if let Some(result) = &r.result
        && (r.source.is_none()
            || !is_transcription_sha256(&result.sha256)
            || result.bytes > TRANSCRIPTION_RESULT_LIMIT as u64
            || uuid(&result.transcript_id).is_err()
            || !result.started_at_ms.is_finite()
            || result.started_at_ms < 0.0)
    {
        return Err(storage("invalid transcription result reference"));
    }
    if (r.action == Action::Generate
        && r.snapshot.state == TranscriptionJobState::Queued
        && (r.source.is_some() || r.result.is_some() || r.uncertain || r.snapshot.result_available))
        || (r.action == Action::Publish
            && (r.source.is_none() || r.result.is_none() || !r.snapshot.result_available))
        || (r.snapshot.state == TranscriptionJobState::Uncertain && !r.uncertain)
        || (r.snapshot.state.is_terminal() && r.snapshot.phase.is_some())
        || (r.snapshot.state == TranscriptionJobState::Succeeded
            && r.snapshot.publication.is_none())
        || (matches!(
            r.snapshot.state,
            TranscriptionJobState::Succeeded | TranscriptionJobState::Empty
        ) && (r.result.is_none() || r.uncertain))
    {
        return Err(storage("inconsistent transcription journal transition"));
    }
    if let Some(publication) = &r.snapshot.publication
        && (r.snapshot.state != TranscriptionJobState::Succeeded
            || !is_transcription_sha256(&publication.revision)
            || r.result
                .as_ref()
                .is_none_or(|result| result.transcript_id != publication.transcript_id))
    {
        return Err(storage("invalid transcription publication confirmation"));
    }
    if r.snapshot.result_available && r.result.is_none() {
        return Err(storage("missing transcription result reference"));
    }
    Ok(())
}
fn persist(root: &Path, r: &Record, previous: Option<&Record>) -> Result<(), ApplicationError> {
    let path = root.join(format!("{}.json", r.snapshot.id));
    let bytes = serde_json::to_vec(r).map_err(storage)?;
    if bytes.len() > RECORD_LIMIT as usize {
        return Err(capacity());
    }
    if let Some(previous) = previous {
        let current: Record =
            serde_json::from_slice(&read(&path, RECORD_LIMIT)?).map_err(storage)?;
        if &current != previous {
            return Err(storage("transcription record changed outside its owner"));
        }
        hypr_storage::fs::atomic_write(&path, std::str::from_utf8(&bytes).map_err(storage)?)
            .map_err(storage)?;
        sync_parent(&path)
    } else {
        write_new(&path, &bytes)
    }
}
fn read_result(root: &Path, r: &Record) -> Result<Option<TranscriptWithData>, ApplicationError> {
    let Some(reference) = &r.result else {
        return Ok(None);
    };
    let path = root.join(format!("{}.result.json", r.snapshot.id));
    if !check_file(&path)? {
        return Ok(None);
    }
    let bytes = read(&path, TRANSCRIPTION_RESULT_LIMIT as u64)?;
    if bytes.len() as u64 != reference.bytes || hash(&bytes) != reference.sha256 {
        return Err(storage("retained transcription result changed"));
    }
    let mut data: TranscriptJson = serde_json::from_slice(&bytes).map_err(storage)?;
    if data.transcripts.len() != 1 {
        return Err(storage("invalid retained transcription result"));
    }
    let transcript = data.transcripts.pop().unwrap();
    // The retained representation is immutable. A future field that this reader
    // cannot reproduce must never disappear during an older server's promotion.
    if encode_result(&transcript)? != bytes {
        return Err(storage("unsupported retained transcription result fields"));
    }
    if transcript.id != reference.transcript_id
        || transcript.session_id != r.request.session_id
        || transcript.started_at != reference.started_at_ms
        || transcript.user_id != r.member
        || transcript.created_at != r.snapshot.created_at
        || transcript.ended_at.is_some()
        || r.source
            .as_ref()
            .is_none_or(|s| hash(transcript.memo_md.as_bytes()) != s.memo_sha256)
    {
        return Err(storage("retained transcription metadata changed"));
    }
    Ok(Some(transcript))
}
pub(crate) fn validate_result(transcript: &TranscriptWithData) -> Result<(), ApplicationError> {
    if transcript.words.len() > 250_000
        || transcript.speaker_hints.len() > 500_000
        || !transcript.started_at.is_finite()
        || transcript.words.iter().any(|w| {
            ![w.start_ms, w.end_ms, w.channel]
                .into_iter()
                .all(f64::is_finite)
        })
    {
        return Err(error(ErrorCode::Validation, "Invalid transcription result"));
    }
    Ok(())
}
pub(crate) fn encode_bounded_result(
    value: &impl Serialize,
    limit: usize,
) -> Result<Vec<u8>, ApplicationError> {
    struct Buffer {
        bytes: Vec<u8>,
        limit: usize,
    }

    impl Write for Buffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.bytes.len().saturating_add(bytes.len()) > self.limit {
                return Err(std::io::Error::other("transcription result size limit"));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Buffer {
        bytes: Vec::new(),
        limit: limit.min(TRANSCRIPTION_RESULT_LIMIT),
    };
    serde_json::to_writer(&mut buffer, value).map_err(|_| capacity())?;
    Ok(buffer.bytes)
}
pub(crate) fn encode_result(transcript: &TranscriptWithData) -> Result<Vec<u8>, ApplicationError> {
    validate_result(transcript)?;
    #[derive(Serialize)]
    struct Envelope<'a> {
        transcripts: [&'a TranscriptWithData; 1],
    }
    encode_bounded_result(
        &Envelope {
            transcripts: [transcript],
        },
        TRANSCRIPTION_RESULT_LIMIT,
    )
}
pub(crate) fn hash(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
pub(crate) fn check_file(path: &Path) -> Result<bool, ApplicationError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(cause) => Err(storage(cause)),
        _ => Err(storage("private transcription path is not a regular file")),
    }
}
pub(crate) fn read(path: &Path, limit: u64) -> Result<Vec<u8>, ApplicationError> {
    if !check_file(path)? {
        return Err(storage("missing transcription file"));
    }
    let file = File::open(path).map_err(storage)?;
    if file.metadata().map_err(storage)?.len() > limit {
        return Err(capacity());
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(storage)?;
    if bytes.len() as u64 > limit {
        return Err(capacity());
    }
    Ok(bytes)
}
pub(crate) fn existing_directory(path: &Path) -> Result<bool, ApplicationError> {
    if !path.is_absolute() {
        return Err(storage("operation path is not absolute"));
    }
    let mut present = true;
    for parent in path.ancestors() {
        match std::fs::symlink_metadata(parent) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => present = false,
            Err(cause) => return Err(storage(cause)),
            _ => return Err(storage("unsafe operation directory")),
        }
    }
    Ok(present)
}
pub(crate) fn ensure_directory(path: &Path) -> Result<(), ApplicationError> {
    if !path.is_absolute() {
        return Err(storage("transcription operation path is not absolute"));
    }
    for parent in path.ancestors() {
        match std::fs::symlink_metadata(parent) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(storage(cause)),
            _ => return Err(storage("unsafe transcription operation directory")),
        }
    }
    let mut missing = Vec::new();
    for parent in path.ancestors() {
        if std::fs::symlink_metadata(parent)
            .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            missing.push(parent.to_owned());
        }
    }
    for directory in missing.into_iter().rev() {
        match std::fs::create_dir(&directory) {
            Ok(()) => sync_parent(&directory)?,
            Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(cause) => return Err(storage(cause)),
        }
        let meta = std::fs::symlink_metadata(&directory).map_err(storage)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(storage("unsafe transcription operation directory"));
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(storage)?;
    }
    Ok(())
}
pub(crate) fn write_new(path: &Path, bytes: &[u8]) -> Result<(), ApplicationError> {
    let parent = path.parent().ok_or_else(unavailable)?;
    ensure_directory(parent)?;
    let temporary = parent.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
    let mut file = File::options()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .map_err(storage)?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(storage)?;
    drop(file);
    hypr_storage::fs::rename_no_replace(&temporary, path).map_err(storage)?;
    sync_parent(path)
}
pub(crate) fn sync_parent(path: &Path) -> Result<(), ApplicationError> {
    #[cfg(unix)]
    {
        File::open(path.parent().ok_or_else(unavailable)?)
            .and_then(|p| p.sync_all())
            .map_err(storage)?;
    }
    Ok(())
}

impl Journal {
    pub(super) async fn replay_submission(
        &self,
        member: String,
        request_id: String,
        request: TranscriptionJobRequest,
    ) -> Result<Option<TranscriptionJobSnapshot>, ApplicationError> {
        self.replay_with_origin(member, request_id, request, None)
            .await
    }
    pub(super) async fn replay_with_origin(
        &self,
        member: String,
        request_id: String,
        request: TranscriptionJobRequest,
        origin: Option<crate::capture_processing::CaptureChildOrigin>,
    ) -> Result<Option<TranscriptionJobSnapshot>, ApplicationError> {
        let member = uuid(&member)?;
        let request_id = uuid(&request_id)?;
        self.run(move |state| {
            let Some(record) = state
                .records
                .values()
                .find(|record| record.member == member && record.request_id == request_id)
            else {
                return Ok(None);
            };
            if record.request != request
                || origin
                    .as_ref()
                    .is_some_and(|origin| record.capture_processing.as_ref() != Some(origin))
            {
                return Err(error(ErrorCode::Conflict, "Transcription request changed"));
            }
            Ok(Some(record.snapshot.clone()))
        })
        .await
    }
    pub(super) async fn ensure_session_idle(&self, id: String) -> Result<(), ApplicationError> {
        self.run(move |state| {
            if state.records.values().any(|record| {
                record.request.session_id == id
                    && (!record.snapshot.state.is_terminal()
                        || record.uncertain
                        || state.active.contains_key(&record.snapshot.id))
            }) {
                return Err(crate::session_lifecycle_gate::session_lifecycle_busy());
            }
            Ok(())
        })
        .await
    }
}

pub(super) async fn ensure_stored_session_idle(
    root: PathBuf,
    vault: String,
    id: String,
) -> Result<(), ApplicationError> {
    tokio::task::spawn_blocking(move || {
        match root.symlink_metadata() {
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            _ => return Err(storage("Invalid transcription inventory")),
        }
        let mut requests = std::collections::HashSet::new();
        for (scanned, entry) in std::fs::read_dir(&root).map_err(storage)?.enumerate() {
            if scanned >= ENTRY_LIMIT {
                return Err(capacity());
            }
            let entry = entry.map_err(storage)?;
            let name = entry.file_name();
            let Some(stem) = name
                .to_str()
                .and_then(|name| name.strip_suffix(".json"))
                .filter(|stem| uuid(stem).is_ok())
            else {
                continue;
            };
            let record: Record =
                serde_json::from_slice(&read(&entry.path(), RECORD_LIMIT)?).map_err(storage)?;
            validate(&record, &vault, stem)?;
            if !requests.insert((record.member.clone(), record.request_id.clone())) {
                return Err(storage("Duplicate transcription request"));
            }
            if record.request.session_id == id
                && (!record.snapshot.state.is_terminal() || record.uncertain)
            {
                return Err(crate::session_lifecycle_gate::session_lifecycle_busy());
            }
        }
        Ok(())
    })
    .await
    .map_err(storage)?
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;

    #[tokio::test]
    async fn pending_or_uncertain_batch_work_is_not_hidden_when_the_engine_is_disabled() {
        let root = tempfile::tempdir().unwrap();
        let directory = root
            .path()
            .canonicalize()
            .unwrap()
            .join("transcription-jobs");
        let journal = Journal::open(
            directory.clone(),
            crate::events::EventHub::new("vault".into(), 16),
            TranscriptionLimits::default(),
            None,
        )
        .await
        .unwrap();
        let member = uuid::Uuid::new_v4().to_string();
        let job = journal
            .submit(
                member.clone(),
                uuid::Uuid::new_v4().to_string(),
                TranscriptionJobRequest {
                    session_id: "note".into(),
                    expected_recording_sha256: "a".repeat(64),
                    expected_transcript: AbsentTranscriptExpectation::Absent,
                    model: "whisper-base".into(),
                    languages: vec![],
                    keywords: vec![],
                },
            )
            .await
            .unwrap();
        assert_eq!(
            journal
                .ensure_session_idle("note".into())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert!(journal.ensure_session_idle("other".into()).await.is_ok());
        assert!(
            ensure_stored_session_idle(directory.clone(), "vault".into(), "note".into())
                .await
                .is_err()
        );
        journal.cancel(member, job.id.clone()).await.unwrap();
        assert!(journal.ensure_session_idle("note".into()).await.is_ok());
        journal
            .change(job.id, |record| record.uncertain = true)
            .await
            .unwrap();
        assert!(journal.ensure_session_idle("note".into()).await.is_err());
        drop(journal);
        assert!(
            ensure_stored_session_idle(directory, "vault".into(), "note".into())
                .await
                .is_err()
        );
    }
}

#[cfg(test)]
mod retirement_source_tests {
    use super::*;
    #[tokio::test]
    async fn closed_or_retained_native_source_cannot_be_mistaken_for_terminal_drain() {
        let root = tempfile::tempdir().unwrap();
        let journal = Journal::open(
            root.path().canonicalize().unwrap().join("jobs"),
            crate::events::EventHub::new("vault".into(), 16),
            TranscriptionLimits::default(),
            None,
        )
        .await
        .unwrap();
        let (release, receipt) = watch::channel(false);
        journal.track_source("note".into(), receipt);
        assert!(journal.ensure_sources_released("note").is_err());
        assert!(journal.ensure_sources_released("other").is_ok());
        drop(release);
        assert!(journal.ensure_sources_released("note").is_err());
        let (release, receipt) = watch::channel(false);
        journal.track_source("other".into(), receipt);
        release.send_replace(true);
        assert!(journal.ensure_sources_released("other").is_ok());
        assert!(journal.ensure_sources_released("note").is_err());
    }
}
