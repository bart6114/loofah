use super::*;
use crate::transcription_jobs::ledger::{
    check_file, ensure_directory, read, sync_parent, write_new,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
    sync::Mutex,
};

const RECORD_BYTES: u64 = 64 * 1024;
const MAX_RECORDS: usize = 256;
const MAX_ENTRIES: usize = 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    version: u32,
    pub identity: CaptureIdentity,
    pub format: CaptureFormat,
    pub model_sha256: String,
    pub target_session: String,
    pub snapshot: CaptureProcessingSnapshot,
    pub summary_revision: Option<String>,
    pub repair_request: Option<TranscriptionJobRequest>,
    pub summary_request: Option<SummaryJobRequest>,
    pub summary_source: Option<hypr_vault_write::transcript_publication::ProcessingSourceBinding>,
    pub transcript_ready: bool,
    pub empty: bool,
    pub cancel_requested: bool,
    pub cancel_reconciled: bool,
    pub epoch: u64,
}

impl Record {
    pub fn new(
        source: hypr_capture_store::capture_spool::CaptureSnapshot,
        model: CaptureProcessingModel,
        request: CaptureProcessingRequest,
        session: String,
        summary_revision: Option<String>,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            version: 1,
            snapshot: CaptureProcessingSnapshot {
                capture_id: source.identity.capture_id.clone(),
                capture_fingerprint: source.capture_fingerprint,
                intent_id: source.identity.request_id.clone(),
                session_id: source.session_id,
                model: model.alias,
                request,
                state: CaptureProcessingState::Waiting,
                live_generation_id: None,
                repair_job_id: None,
                summary_job_id: None,
                transcript_publication: None,
                failure: None,
                created_at: now.clone(),
                updated_at: now,
            },
            identity: source.identity,
            format: source.format,
            model_sha256: model.sha256,
            target_session: session,
            summary_revision,
            repair_request: None,
            summary_request: None,
            summary_source: None,
            transcript_ready: false,
            empty: false,
            cancel_requested: false,
            cancel_reconciled: false,
            epoch: 0,
        }
    }
    pub fn member(&self) -> &str {
        &self.identity.scope.member_id
    }
    pub fn blocks(&self) -> bool {
        !self.snapshot.state.is_terminal()
            || self.snapshot.state == CaptureProcessingState::Uncertain
            || (self.cancel_requested && !self.cancel_reconciled)
    }
}

#[derive(Clone)]
pub(super) struct Journal(Arc<Inner>);
struct Inner {
    root: PathBuf,
    vault: String,
    records: Mutex<BTreeMap<String, Record>>,
    io: Arc<tokio::sync::Semaphore>,
    events: crate::events::EventHub,
    failed: std::sync::atomic::AtomicBool,
    _lease: File,
    _vault: Arc<File>,
}

impl Journal {
    pub async fn open(root: PathBuf, context: Arc<VaultContext>) -> Result<Self, ApplicationError> {
        tokio::task::spawn_blocking(move || {
            ensure_directory(&root)?;
            let lock = root.join(".lock");
            check_file(&lock)?;
            let lease = File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock)
                .map_err(storage)?;
            lease
                .try_lock()
                .map_err(|_| unavailable("Capture processing already has an owner"))?;
            let vault = context.events.vault_id();
            let records = read_records(&root, &vault)?;
            Ok(Self(Arc::new(Inner {
                root,
                vault,
                records: Mutex::new(records),
                io: Arc::new(tokio::sync::Semaphore::new(2)),
                events: context.events.clone(),
                failed: false.into(),
                _lease: lease,
                _vault: context
                    .lease_pin()
                    .ok_or_else(|| unavailable("Vault owner required"))?,
            })))
        })
        .await
        .map_err(storage)?
    }
    async fn run<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&Inner, &mut BTreeMap<String, Record>) -> Result<T, ApplicationError>
        + Send
        + 'static,
    ) -> Result<T, ApplicationError> {
        let permit = self
            .0
            .io
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable("Capture processing storage busy"))?;
        let owner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if owner.failed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(storage("Capture processing storage requires review"));
            }
            let mut records = owner.records.lock().map_err(storage)?;
            operation(&owner, &mut records)
        })
        .await
        .map_err(storage)?
    }
    pub fn records(&self) -> Result<Vec<Record>, ApplicationError> {
        if self.0.failed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(storage("Capture processing storage requires review"));
        }
        Ok(self
            .0
            .records
            .lock()
            .map_err(storage)?
            .values()
            .cloned()
            .collect())
    }
    pub fn get(&self, member: &str, capture: &str) -> Result<Record, ApplicationError> {
        if self.0.failed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(storage("Capture processing storage requires review"));
        }
        self.0
            .records
            .lock()
            .map_err(storage)?
            .get(capture)
            .filter(|r| r.member() == member)
            .cloned()
            .ok_or_else(not_found)
    }
    pub fn pending(&self) -> Result<Vec<Record>, ApplicationError> {
        if self.0.failed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(storage("Capture processing storage requires review"));
        }
        Ok(self
            .0
            .records
            .lock()
            .map_err(storage)?
            .values()
            .filter(|r| {
                !r.snapshot.state.is_terminal() || (r.cancel_requested && !r.cancel_reconciled)
            })
            .cloned()
            .collect())
    }
    pub async fn admit(&self, record: Record) -> Result<Record, ApplicationError> {
        self.run(move |owner, records| {
            if let Some(old) = records.get(&record.identity.capture_id) {
                return if old.identity == record.identity
                    && old.format == record.format
                    && old.snapshot.request == record.snapshot.request
                {
                    Ok(old.clone())
                } else {
                    Err(conflict("Capture processing replay changed"))
                };
            }
            if records.len() >= MAX_RECORDS
                || records
                    .values()
                    .filter(|r| !r.snapshot.state.is_terminal())
                    .count()
                    >= 32
            {
                return Err(capacity());
            }
            owner.persist(None, &record)?;
            records.insert(record.identity.capture_id.clone(), record.clone());
            Ok(record)
        })
        .await
    }
    pub async fn update(
        &self,
        previous: Record,
        mut next: Record,
    ) -> Result<Record, ApplicationError> {
        self.run(move |owner, records| {
            let current = records
                .get(&previous.identity.capture_id)
                .ok_or_else(not_found)?;
            if current != &previous {
                return Ok(current.clone());
            }
            if next == previous {
                return Ok(next);
            }
            next.epoch = previous.epoch.checked_add(1).ok_or_else(capacity)?;
            next.snapshot.updated_at = chrono::Utc::now().to_rfc3339();
            owner.persist(Some(&previous), &next)?;
            records.insert(next.identity.capture_id.clone(), next.clone());
            Ok(next)
        })
        .await
    }
}
impl Inner {
    fn persist(&self, previous: Option<&Record>, record: &Record) -> Result<(), ApplicationError> {
        validate(record, &self.vault, &record.identity.capture_id)?;
        let bytes = serde_json::to_vec(record).map_err(storage)?;
        if bytes.len() as u64 > RECORD_BYTES {
            return Err(capacity());
        }
        let path = self
            .root
            .join(format!("{}.json", record.identity.capture_id));
        let result = (|| {
            if let Some(previous) = previous {
                let actual: Record =
                    serde_json::from_slice(&read(&path, RECORD_BYTES)?).map_err(storage)?;
                if &actual != previous {
                    return Err(storage(
                        "Capture processing intent changed outside its owner",
                    ));
                }
                hypr_storage::fs::atomic_write(
                    &path,
                    std::str::from_utf8(&bytes).map_err(storage)?,
                )
                .map_err(storage)?;
                sync_parent(&path)
            } else {
                write_new(&path, &bytes)
            }
        })();
        if result.is_err() {
            self.failed
                .store(true, std::sync::atomic::Ordering::Release);
        }
        result?;
        self.events.publish_for(
            record.member(),
            Change::JobChanged {
                job_id: record.snapshot.intent_id.clone(),
            },
        );
        Ok(())
    }
}
fn validate(record: &Record, vault: &str, id: &str) -> Result<(), ApplicationError> {
    let s = &record.snapshot;
    if record.version != 1
        || record.identity.scope.vault_id != vault
        || record.identity.capture_id != id
        || !valid_uuid(id)
        || !valid_uuid(&record.identity.request_id)
        || !valid_uuid(record.member())
        || s.capture_id != id
        || s.intent_id != record.identity.request_id
        || s.capture_fingerprint != capture_fingerprint(&record.identity, &record.format)
        || s.request.capture_fingerprint != s.capture_fingerprint
        || validate_capture_format(&record.format).is_err()
        || matches!(
            record.identity.target,
            CaptureTarget::ExistingSession {
                expected_audio_sha256: Some(_),
                ..
            }
        )
        || s.request
            .model
            .as_ref()
            .is_some_and(|alias| alias != &s.model)
        || !is_transcription_sha256(&record.model_sha256)
        || s.model.is_empty()
        || s.model.len() > 512
        || validate_capture_processing_request(&s.request).is_err()
        || VaultContext::capture_target_session_id(&record.identity)? != record.target_session
        || s.session_id
            .as_ref()
            .is_some_and(|id| id != &record.target_session)
        || chrono::DateTime::parse_from_rfc3339(&s.created_at).is_err()
        || chrono::DateTime::parse_from_rfc3339(&s.updated_at).is_err()
        || s.live_generation_id
            .as_ref()
            .is_some_and(|id| id != &s.intent_id)
        || s.repair_job_id.as_ref().is_some_and(|id| !valid_uuid(id))
        || s.summary_job_id.as_ref().is_some_and(|id| !valid_uuid(id))
        || (s.transcript_publication.is_some() && !record.transcript_ready)
        || (record.empty && !record.transcript_ready)
        || (record.cancel_reconciled && (!record.cancel_requested || !s.state.is_terminal()))
        || (record.transcript_ready && !record.empty && s.transcript_publication.is_none())
        || (record.empty && s.transcript_publication.is_some())
        || (s.state == CaptureProcessingState::Empty && !record.empty)
        || (s.state == CaptureProcessingState::Empty
            && s.request.summary.is_some()
            && s.summary_job_id.is_none())
        || (s.state == CaptureProcessingState::Succeeded
            && (record.empty || (s.request.summary.is_some() && s.summary_job_id.is_none())))
        || (matches!(
            s.state,
            CaptureProcessingState::Succeeded | CaptureProcessingState::Empty
        ) && !record.transcript_ready)
        || record
            .summary_revision
            .as_ref()
            .is_some_and(|revision| !is_transcription_sha256(revision))
    {
        return Err(storage("Invalid capture processing intent"));
    }
    let endpoint = url::Url::parse(&record.identity.scope.server).map_err(storage)?;
    if validate_endpoint(&endpoint, true).is_err()
        || endpoint.as_str() != record.identity.scope.server
        || endpoint.path() != "/"
        || endpoint.query().is_some()
    {
        return Err(storage("Invalid capture processing server binding"));
    }
    if let Some(binding) = &record.summary_source
        && (!is_transcription_sha256(&binding.recording_sha256)
            || !is_transcription_sha256(&binding.transcript_revision)
            || s.transcript_publication
                .as_ref()
                .is_some_and(|publication| publication.revision != binding.transcript_revision)
            || binding.transcript_present == record.empty
            || record.repair_request.as_ref().is_some_and(|request| {
                request.expected_recording_sha256 != binding.recording_sha256
            }))
    {
        return Err(storage("Invalid summary source revision"));
    }
    if let Some(publication) = &s.transcript_publication
        && (!is_transcription_sha256(&publication.revision)
            || publication.transcript_id.is_empty()
            || publication.transcript_id.len() > 512
            || publication.transcript_id.chars().any(char::is_control))
    {
        return Err(storage("Invalid transcript publication proof"));
    }
    if let Some(request) = &record.repair_request
        && (validate_transcription_job_request(request).is_err()
            || request.session_id != record.target_session
            || request.model != s.model
            || request.languages != s.request.languages
            || request.keywords != s.request.keywords)
    {
        return Err(storage("Invalid repair binding"));
    }
    if let Some(request) = &record.summary_request
        && (s.request.summary.as_ref().is_none_or(|summary| {
            summary.target != request.target || summary.title_policy != request.title_policy
        }) || request.session_id != record.target_session
            || record.summary_revision.as_ref() != Some(&request.expected_revision)
            || !record.transcript_ready
            || record.summary_source.is_none())
    {
        return Err(storage("Invalid summary binding"));
    }
    Ok(())
}
fn read_records(root: &Path, vault: &str) -> Result<BTreeMap<String, Record>, ApplicationError> {
    let mut records = BTreeMap::new();
    for (scanned, entry) in std::fs::read_dir(root).map_err(storage)?.enumerate() {
        if scanned >= MAX_ENTRIES {
            return Err(capacity());
        }
        let entry = entry.map_err(storage)?;
        let name = entry.file_name();
        let Some(id) = name.to_str().and_then(|name| name.strip_suffix(".json")) else {
            continue;
        };
        // Only the private ledger's admitted UUID files are readable content.
        if !valid_uuid(id) {
            return Err(storage("Invalid capture processing filename"));
        }
        if records.len() >= MAX_RECORDS {
            return Err(capacity());
        }
        let record: Record =
            serde_json::from_slice(&read(&entry.path(), RECORD_BYTES)?).map_err(storage)?;
        validate(&record, vault, id)?;
        records.insert(id.to_owned(), record);
    }
    Ok(records)
}
pub(super) async fn ensure_stored_idle(
    root: PathBuf,
    vault: String,
    session: String,
    lease: Option<Arc<File>>,
    source: bool,
) -> Result<(), ApplicationError> {
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        match root.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            _ => return Err(storage("Unsafe capture processing inventory")),
        }
        for directory in root.ancestors() {
            let metadata = directory.symlink_metadata().map_err(storage)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(storage("Unsafe capture processing inventory ancestor"));
            }
        }
        if read_records(&root, &vault)?.values().any(|r| {
            r.target_session == session
                && if source {
                    !r.transcript_ready && r.snapshot.state != CaptureProcessingState::Cancelled
                } else {
                    r.blocks()
                }
        }) {
            return Err(unavailable("Capture processing must finish or be reviewed"));
        }
        Ok(())
    })
    .await
    .map_err(storage)?
}

pub(super) async fn reject_stored_child(
    root: PathBuf,
    vault: String,
    member: String,
    request_id: String,
    live_capture: Option<String>,
    lease: Option<Arc<File>>,
) -> Result<(), ApplicationError> {
    tokio::task::spawn_blocking(move || {
        let _lease = lease;
        match root.symlink_metadata() {
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            _ => return Err(storage("Unsafe capture processing inventory")),
        }
        for path in root.ancestors() {
            let meta = path.symlink_metadata().map_err(storage)?;
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(storage("Unsafe capture processing ancestor"));
            }
        }
        let records = read_records(&root, &vault)?;
        let owned = records.values().any(|record| {
            record.member() == member
                && match &live_capture {
                    Some(capture) => {
                        record.identity.capture_id == *capture
                            && record.identity.request_id == request_id
                    }
                    None => {
                        child_request_id(record, "repair") == request_id
                            || child_request_id(record, "summary") == request_id
                    }
                }
        });
        if owned {
            Err(unavailable(
                "Capture processing coordinator must reconcile its retained child",
            ))
        } else {
            Ok(())
        }
    })
    .await
    .map_err(storage)?
}
