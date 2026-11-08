use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use hypr_loofah_protocol::{
    ApplicationError, Change, DocumentTarget, ErrorCode, MutationOutcome, ProcessingJobFailure,
    ProcessingJobSnapshot, ProcessingJobState, ProcessingPhase, SummaryJobRequest, TitlePolicy,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{events::EventHub, vault::error};

const MAX_RECORD_BYTES: u64 = 16 * 1024;
const MAX_JOBS: usize = 10_000;
const MAX_PENDING: usize = 32;

mod access;
#[cfg(test)]
mod capture_origin_tests;
pub mod configured;
pub mod generation;
pub(crate) mod lifecycle;
mod prepared;
pub mod providers;
mod result;
pub mod source;
pub mod title_generation;
mod title_stage;
mod title_worker;
mod worker;
pub use prepared::{JobGenerator, PreparedGeneration, TitleRunner};
pub use result::GeneratedDocument;
pub use worker::{JobAuthorizer, JobFuture, JobWork, JobWorker};

pub(crate) struct ProcessingService {
    pub(crate) journal: JobJournal,
    pub(crate) lifetime: tokio_util::sync::CancellationToken,
    pub(crate) finished: tokio_util::sync::CancellationToken,
}

#[derive(Clone)]
pub struct JobJournal(Arc<Journal>);

struct Journal {
    state: Mutex<State>,
    events: EventHub,
    _lease: File,
    worker: Arc<tokio::sync::Semaphore>,
}
struct State {
    root: PathBuf,
    vault_id: String,
    records: HashMap<String, Record>,
    ids: BTreeMap<String, String>,
    failed: bool,
    #[cfg(test)]
    fail_after_persist: bool,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    schema_version: u32,
    vault_id: String,
    namespace: String,
    request_id: String,
    request: SummaryJobRequest,
    snapshot: ProcessingJobSnapshot,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    body_applied: Option<title_stage::BodyApplied>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capture_processing: Option<crate::capture_processing::CaptureChildOrigin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    local_capture: Option<crate::local_capture_intents::LocalCaptureChildOrigin>,
}

#[derive(Clone, Copy)]
pub enum JobCompletion {
    Succeeded,
    Failed(ProcessingJobFailure),
    Conflict,
    Cancelled,
    Interrupted,
}

impl JobJournal {
    pub(crate) async fn local_capture_origin(
        &self,
        id: String,
    ) -> Result<Option<crate::local_capture_intents::LocalCaptureChildOrigin>, ApplicationError>
    {
        self.run(move |state, _| Ok(state.record(&id)?.local_capture.clone()))
            .await
    }
    pub(crate) async fn capture_request_id(
        &self,
        id: String,
    ) -> Result<
        (
            String,
            Option<crate::capture_processing::CaptureChildOrigin>,
        ),
        ApplicationError,
    > {
        self.run(move |state, _| {
            let record = state.record(&id)?;
            Ok((record.request_id.clone(), record.capture_processing.clone()))
        })
        .await
    }
    pub(crate) async fn cancel_capture_summary_request(
        &self,
        namespace: String,
        request_id: String,
        request: SummaryJobRequest,
    ) -> Result<bool, ApplicationError> {
        if let Some(snapshot) = self
            .replay_submission(namespace.clone(), request_id, request)
            .await?
        {
            return Ok(self
                .cancel_owned(namespace, snapshot.id)
                .await?
                .state
                .is_terminal());
        }
        Ok(true)
    }
    pub async fn open(root: PathBuf, events: EventHub) -> Result<Self, ApplicationError> {
        tokio::task::spawn_blocking(move || {
            let root = std::path::absolute(root).map_err(|_| storage())?;
            ensure_directory(&root)?;
            let root = root.canonicalize().map_err(|_| storage())?;
            let lock = root.join(".lock");
            if let Ok(metadata) = lock.symlink_metadata()
                && (!metadata.is_file() || metadata.file_type().is_symlink())
            {
                return Err(storage());
            }
            let lease = File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock)
                .map_err(|_| storage())?;
            lease.try_lock().map_err(|_| {
                rejected(ErrorCode::Conflict, "Processing jobs already have an owner")
            })?;
            let mut state = State {
                root,
                vault_id: events.vault_id(),
                records: HashMap::new(),
                ids: BTreeMap::new(),
                failed: false,
                #[cfg(test)]
                fail_after_persist: false,
            };
            let entries = std::fs::read_dir(&state.root).map_err(|_| storage())?;
            for (scanned, entry) in entries.enumerate() {
                if scanned >= 20_000 {
                    return Err(capacity());
                }
                let entry = entry.map_err(|_| storage())?;
                let name = entry.file_name();
                let Some(key) = name
                    .to_str()
                    .and_then(|name| name.strip_suffix(".json"))
                    .filter(|key| is_key(key))
                else {
                    continue;
                };
                if state.records.len() >= MAX_JOBS {
                    return Err(capacity());
                }
                let record = read_record(&entry.path())?;
                validate_record(&state.vault_id, key, &record)?;
                if state
                    .ids
                    .insert(record.snapshot.id.clone(), key.to_owned())
                    .is_some()
                {
                    return Err(storage());
                }
                state.records.insert(key.into(), record);
            }
            state.interrupt_active(&events)?;
            Ok(Self(Arc::new(Journal {
                state: Mutex::new(state),
                events,
                _lease: lease,
                worker: Arc::new(tokio::sync::Semaphore::new(1)),
            })))
        })
        .await
        .map_err(|_| storage())?
    }

    pub async fn submit(
        &self,
        namespace: String,
        request_id: String,
        request: SummaryJobRequest,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        self.submit_with_origin(namespace, request_id, request, None)
            .await
    }
    pub(crate) async fn submit_with_origin(
        &self,
        namespace: String,
        request_id: String,
        request: SummaryJobRequest,
        origin: Option<crate::capture_processing::CaptureChildOrigin>,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        self.submit_with_origins(namespace, request_id, request, origin, None)
            .await
    }
    pub(crate) async fn submit_with_origins(
        &self,
        namespace: String,
        request_id: String,
        request: SummaryJobRequest,
        origin: Option<crate::capture_processing::CaptureChildOrigin>,
        local: Option<crate::local_capture_intents::LocalCaptureChildOrigin>,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        if let Some(local) = &local {
            local.validate_request(&request)?;
            if origin.is_some() || namespace != crate::local_capture_intents::SUMMARY_OWNER {
                return Err(storage());
            }
        }
        validate_namespace(&namespace)?;
        validate_request(&request)?;
        let request_id = parse_uuid(&request_id)?;
        if let Some(origin) = &origin {
            origin.validate_request(&request)?;
        }
        self.run(move |state, events| {
            let key = key(&namespace, &request_id);
            if let Some(record) = state.records.get(&key) {
                return if record.request == request
                    && local
                        .as_ref()
                        .is_none_or(|local| record.local_capture.as_ref() == Some(local))
                    && origin
                        .as_ref()
                        .is_none_or(|origin| record.capture_processing.as_ref() == Some(origin))
                {
                    Ok(record.snapshot.clone())
                } else {
                    Err(rejected(
                        ErrorCode::Conflict,
                        "The job request ID was already used for different input",
                    ))
                };
            }
            if state.records.len() >= MAX_JOBS
                || state
                    .records
                    .values()
                    .filter(|record| !record.snapshot.state.is_terminal())
                    .count()
                    >= MAX_PENDING
            {
                return Err(capacity());
            }
            let timestamp = now();
            let record = Record {
                schema_version: if local.is_some() {
                    4
                } else if request.only_if_empty {
                    3
                } else if request.title_policy == TitlePolicy::Preserve {
                    1
                } else {
                    2
                },
                vault_id: state.vault_id.clone(),
                namespace,
                request_id,
                snapshot: ProcessingJobSnapshot {
                    id: uuid::Uuid::new_v4().to_string(),
                    session_id: request.session_id.clone(),
                    target: request.target.clone(),
                    state: ProcessingJobState::Queued,
                    phase: None,
                    failure: None,
                    created_at: timestamp.clone(),
                    updated_at: timestamp,
                },
                request,
                body_applied: None,
                capture_processing: origin,
                local_capture: local,
            };
            state.persist(&record)?;
            state.ids.insert(record.snapshot.id.clone(), key.clone());
            let snapshot = record.snapshot.clone();
            let audience = record.namespace.clone();
            state.records.insert(key, record);
            events.publish_for(
                &audience,
                Change::JobChanged {
                    job_id: snapshot.id.clone(),
                },
            );
            Ok(snapshot)
        })
        .await
    }

    pub async fn get(&self, id: String) -> Result<Option<ProcessingJobSnapshot>, ApplicationError> {
        let id = parse_uuid(&id)?;
        self.run(move |state, _| {
            Ok(state
                .ids
                .get(&id)
                .and_then(|key| state.records.get(key))
                .map(|record| record.snapshot.clone()))
        })
        .await
    }

    pub async fn page(
        &self,
        limit: usize,
        after: Option<String>,
    ) -> Result<Vec<ProcessingJobSnapshot>, ApplicationError> {
        if !(1..=200).contains(&limit) {
            return Err(rejected(
                ErrorCode::Validation,
                "Invalid processing job page size",
            ));
        }
        let after = after.map(|value| parse_uuid(&value)).transpose()?;
        self.run(move |state, _| {
            Ok(state
                .ids
                .iter()
                .filter(|(id, _)| after.as_ref().is_none_or(|after| *id > after))
                .take(limit)
                .map(|(_, key)| state.records[key].snapshot.clone())
                .collect())
        })
        .await
    }

    pub async fn request(&self, id: String) -> Result<SummaryJobRequest, ApplicationError> {
        let id = parse_uuid(&id)?;
        self.run(move |state, _| Ok(state.record(&id)?.request.clone()))
            .await
    }

    pub async fn claim(
        &self,
        id: String,
    ) -> Result<Option<ProcessingJobSnapshot>, ApplicationError> {
        let id = parse_uuid(&id)?;
        self.run(move |state, events| {
            let mut record = state.record(&id)?.clone();
            if record.snapshot.state != ProcessingJobState::Queued {
                return Ok(None);
            }
            record.snapshot.state = ProcessingJobState::Running;
            record.snapshot.phase = Some(ProcessingPhase::Preparing);
            state.update(record, events).map(Some)
        })
        .await
    }

    pub async fn progress(
        &self,
        id: String,
        phase: ProcessingPhase,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        let id = parse_uuid(&id)?;
        self.run(move |state, events| {
            let mut record = state.record(&id)?.clone();
            if !matches!(
                record.snapshot.state,
                ProcessingJobState::Running | ProcessingJobState::Cancelling
            ) || (record.body_applied.is_some() && phase != ProcessingPhase::Applying)
            {
                return Err(transition());
            }
            if record.snapshot.phase == Some(phase) {
                return Ok(record.snapshot);
            }
            record.snapshot.phase = Some(phase);
            state.update(record, events)
        })
        .await
    }

    pub async fn cancel(&self, id: String) -> Result<ProcessingJobSnapshot, ApplicationError> {
        let id = parse_uuid(&id)?;
        self.run(move |state, events| {
            let record = state.record(&id)?.clone();
            state.cancel_record(record, events)
        })
        .await
    }

    pub async fn complete(
        &self,
        id: String,
        result: JobCompletion,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        let id = parse_uuid(&id)?;
        self.run(move |state, events| {
            let mut record = state.record(&id)?.clone();
            let result = if record.body_applied.is_some() {
                JobCompletion::Succeeded
            } else {
                result
            };
            let (next, failure) = match result {
                JobCompletion::Succeeded => (ProcessingJobState::Succeeded, None),
                JobCompletion::Conflict => (ProcessingJobState::Conflict, None),
                JobCompletion::Cancelled => (ProcessingJobState::Cancelled, None),
                JobCompletion::Interrupted => (
                    ProcessingJobState::Interrupted,
                    Some(ProcessingJobFailure::Interrupted),
                ),
                JobCompletion::Failed(ProcessingJobFailure::Interrupted) => {
                    return Err(transition());
                }
                JobCompletion::Failed(failure) => (ProcessingJobState::Failed, Some(failure)),
            };
            if record.snapshot.state.is_terminal() {
                return if record.snapshot.state == next && record.snapshot.failure == failure {
                    Ok(record.snapshot)
                } else {
                    Err(transition())
                };
            }
            if !matches!(
                record.snapshot.state,
                ProcessingJobState::Running | ProcessingJobState::Cancelling
            ) || (next == ProcessingJobState::Cancelled
                && record.snapshot.state != ProcessingJobState::Cancelling)
            {
                return Err(transition());
            }
            // A cancellation request cannot undo a result whose commit already finished.
            record.snapshot.state = next;
            record.snapshot.phase = None;
            record.snapshot.failure = failure;
            state.update(record, events)
        })
        .await
    }

    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut State, &EventHub) -> Result<T, ApplicationError> + Send + 'static,
    ) -> Result<T, ApplicationError> {
        let journal = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let mut state = journal.state.lock().map_err(|_| storage())?;
            if state.failed {
                let mut failure = storage();
                failure.code = ErrorCode::Unavailable;
                failure.message = "Processing job storage requires recovery".into();
                return Err(failure);
            }
            operation(&mut state, &journal.events)
        })
        .await
        .map_err(|_| storage())?
    }
}

impl State {
    fn cancel_record(
        &mut self,
        mut record: Record,
        events: &EventHub,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        record.snapshot.state = match record.snapshot.state {
            ProcessingJobState::Queued => ProcessingJobState::Cancelled,
            ProcessingJobState::Running => ProcessingJobState::Cancelling,
            _ => return Ok(record.snapshot),
        };
        self.update(record, events)
    }
    fn interrupt_active(&mut self, events: &EventHub) -> Result<(), ApplicationError> {
        let active: Vec<_> = self
            .records
            .values()
            .filter(|record| {
                matches!(
                    record.snapshot.state,
                    ProcessingJobState::Running | ProcessingJobState::Cancelling
                )
            })
            .cloned()
            .collect();
        for mut record in active {
            let confirmed = record.body_applied.is_some();
            if confirmed {
                title_stage::interrupt(self, &record);
            }
            record.snapshot.state = if confirmed {
                ProcessingJobState::Succeeded
            } else {
                ProcessingJobState::Interrupted
            };
            record.snapshot.phase = None;
            record.snapshot.failure = (!confirmed).then_some(ProcessingJobFailure::Interrupted);
            self.update(record, events)?;
        }
        Ok(())
    }

    fn persist(&mut self, record: &Record) -> Result<(), ApplicationError> {
        let result = persist(&self.root, record);
        #[cfg(test)]
        let result = if result.is_ok() && self.fail_after_persist {
            self.fail_after_persist = false;
            Err(storage())
        } else {
            result
        };
        if result.is_err() {
            // A rename may have succeeded before its sync failed; never overwrite from stale state.
            self.failed = true;
        }
        result
    }
    fn record(&self, id: &str) -> Result<&Record, ApplicationError> {
        self.ids
            .get(id)
            .and_then(|key| self.records.get(key))
            .ok_or_else(|| rejected(ErrorCode::NotFound, "Processing job not found"))
    }
    fn update(
        &mut self,
        mut record: Record,
        events: &EventHub,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        record.snapshot.updated_at = now();
        self.persist(&record)?;
        let snapshot = record.snapshot.clone();
        let audience = record.namespace.clone();
        self.records
            .insert(key(&record.namespace, &record.request_id), record);
        events.publish_for(
            &audience,
            Change::JobChanged {
                job_id: snapshot.id.clone(),
            },
        );
        Ok(snapshot)
    }
}

fn validate_request(request: &SummaryJobRequest) -> Result<(), ApplicationError> {
    hypr_vault_read::paths::validate_session_id(&request.session_id)
        .map_err(|_| rejected(ErrorCode::Validation, "Invalid job session"))?;
    if let DocumentTarget::TemplateOutput { document_id } = &request.target {
        hypr_vault_read::paths::validate_session_id(document_id)
            .map_err(|_| rejected(ErrorCode::Validation, "Invalid job target"))?;
    }
    if request.session_id.len() > 512
        || request.expected_revision.is_empty()
        || request.expected_revision.len() > 128
        || request.expected_revision.chars().any(char::is_control)
        || matches!(&request.target, DocumentTarget::TemplateOutput {document_id} if document_id.len()>512)
    {
        return Err(rejected(
            ErrorCode::Validation,
            "Invalid processing job input",
        ));
    }
    Ok(())
}
fn validate_namespace(namespace: &str) -> Result<(), ApplicationError> {
    if namespace.trim().is_empty()
        || namespace.len() > 256
        || namespace.chars().any(char::is_control)
    {
        return Err(rejected(
            ErrorCode::Validation,
            "Invalid processing job owner",
        ));
    }
    Ok(())
}
fn conditional_target_filled(
    request: &SummaryJobRequest,
    snapshot: &hypr_loofah_protocol::DocumentSnapshot,
) -> bool {
    request.only_if_empty
        && snapshot
            .document
            .as_ref()
            .is_some_and(|document| !document.markdown.trim().is_empty())
}

fn validate_record(
    vault: &str,
    filename_key: &str,
    record: &Record,
) -> Result<(), ApplicationError> {
    if let Some(origin) = &record.capture_processing {
        origin.validate_request(&record.request)?;
    }
    if let Some(local) = &record.local_capture {
        local.validate_request(&record.request)?;
        if record.capture_processing.is_some()
            || record.namespace != crate::local_capture_intents::SUMMARY_OWNER
        {
            return Err(storage());
        }
    }
    let expected_version = if record.local_capture.is_some() {
        4
    } else if record.request.only_if_empty {
        3
    } else if record.request.title_policy == TitlePolicy::Preserve {
        1
    } else {
        2
    };
    if record.schema_version != expected_version {
        return Err(rejected(
            ErrorCode::IncompatibleVersion,
            "Unsupported processing job storage version",
        ));
    }
    validate_namespace(&record.namespace).map_err(|_| storage())?;
    validate_request(&record.request).map_err(|_| storage())?;
    if parse_uuid(&record.request_id).map_err(|_| storage())? != record.request_id {
        return Err(storage());
    }
    if record.vault_id != vault
        || key(
            &record.namespace,
            &parse_uuid(&record.request_id).map_err(|_| storage())?,
        ) != filename_key
        || parse_uuid(&record.snapshot.id).map_err(|_| storage())? != record.snapshot.id
        || record.request.session_id != record.snapshot.session_id
        || record.request.target != record.snapshot.target
        || chrono::DateTime::parse_from_rfc3339(&record.snapshot.created_at).is_err()
        || chrono::DateTime::parse_from_rfc3339(&record.snapshot.updated_at).is_err()
    {
        return Err(storage());
    }
    let snapshot = &record.snapshot;
    if let Some(body) = &record.body_applied
        && (record.request.title_policy != TitlePolicy::GenerateIfEmpty
            || !title_stage::valid_revision(&body.revision)
            || !matches!(
                snapshot.state,
                ProcessingJobState::Running
                    | ProcessingJobState::Cancelling
                    | ProcessingJobState::Succeeded
            )
            || (snapshot.state != ProcessingJobState::Succeeded
                && snapshot.phase != Some(ProcessingPhase::Applying)))
    {
        return Err(storage());
    }
    let active = matches!(
        snapshot.state,
        ProcessingJobState::Running | ProcessingJobState::Cancelling
    );
    if snapshot.phase.is_some() != active
        || match snapshot.state {
            ProcessingJobState::Failed => {
                snapshot.failure.is_none()
                    || snapshot.failure == Some(ProcessingJobFailure::Interrupted)
            }
            ProcessingJobState::Interrupted => {
                snapshot.failure != Some(ProcessingJobFailure::Interrupted)
            }
            _ => snapshot.failure.is_some(),
        }
    {
        return Err(storage());
    }
    Ok(())
}
fn read_record(path: &Path) -> Result<Record, ApplicationError> {
    let metadata = path.symlink_metadata().map_err(|_| storage())?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > MAX_RECORD_BYTES
    {
        return Err(storage());
    }
    let file = File::open(path).map_err(|_| storage())?;
    if !file.metadata().map_err(|_| storage())?.is_file() {
        return Err(storage());
    }
    let mut bytes = Vec::new();
    file.take(MAX_RECORD_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| storage())?;
    if bytes.len() as u64 > MAX_RECORD_BYTES {
        return Err(storage());
    }
    serde_json::from_slice(&bytes).map_err(|_| storage())
}
fn persist(root: &Path, record: &Record) -> Result<(), ApplicationError> {
    let value = serde_json::to_string(record).map_err(|_| storage())?;
    if value.len() as u64 > MAX_RECORD_BYTES {
        return Err(capacity());
    }
    let path = root.join(format!(
        "{}.json",
        key(&record.namespace, &record.request_id)
    ));
    hypr_storage::fs::atomic_write(&path, &value).map_err(|_| storage())?;
    #[cfg(unix)]
    File::open(root)
        .and_then(|file| file.sync_all())
        .map_err(|_| storage())?;
    Ok(())
}
fn ensure_directory(root: &Path) -> Result<(), ApplicationError> {
    create_directory(root)?;
    let metadata = root.symlink_metadata().map_err(|_| storage())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(storage());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| storage())?;
    }
    Ok(())
}
fn create_directory(path: &Path) -> Result<(), ApplicationError> {
    if path.is_dir() {
        return Ok(());
    }
    let parent = path.parent().ok_or_else(storage)?;
    create_directory(parent)?;
    match std::fs::create_dir(path) {
        Ok(()) => {
            #[cfg(unix)]
            File::open(parent)
                .and_then(|file| file.sync_all())
                .map_err(|_| storage())?;
            Ok(())
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(_) => Err(storage()),
    }
}
fn key(namespace: &str, request_id: &str) -> String {
    Sha256::digest(serde_json::to_vec(&(namespace, request_id)).unwrap())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn is_key(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn parse_uuid(value: &str) -> Result<String, ApplicationError> {
    uuid::Uuid::parse_str(value)
        .map(|value| value.to_string())
        .map_err(|_| {
            rejected(
                ErrorCode::Validation,
                "A UUID processing job identifier is required",
            )
        })
}
fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
fn capacity() -> ApplicationError {
    rejected(ErrorCode::ResourceLimit, "Processing job capacity reached")
}
fn transition() -> ApplicationError {
    rejected(
        ErrorCode::Conflict,
        "The processing job state no longer permits this operation",
    )
}
fn rejected(code: ErrorCode, message: &str) -> ApplicationError {
    let mut failure = error(code, message);
    failure.mutation_outcome = Some(MutationOutcome::NotCommitted);
    failure
}
fn storage() -> ApplicationError {
    let mut failure = error(ErrorCode::Internal, "Processing job storage is unavailable");
    failure.mutation_outcome = Some(MutationOutcome::Unknown);
    failure
}

#[cfg(test)]
mod tests;
