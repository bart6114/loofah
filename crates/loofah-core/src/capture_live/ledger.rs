use super::*;
use crate::transcription_jobs::ledger::{
    check_file, encode_result, ensure_directory, hash, read, sync_parent, write_new,
};
use hypr_vault_read::{TranscriptJson, TranscriptWithData};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, fs::File, path::PathBuf, sync::Mutex};
use tokio::sync::Semaphore;

const MANIFEST_BYTES: u64 = 64 * 1024;
const MAX_GENERATIONS: usize = 64;
const MAX_CAPTIONS: u64 = 65_536;
const MAX_CAPTION_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MEMO_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Record {
    version: u32,
    pub identity: CaptureIdentity,
    pub format: CaptureFormat,
    pub request: CaptureLiveRequest,
    pub snapshot: CaptureLiveSnapshot,
    pub observation_ms: u64,
    pub created_at: String,
    pub memo_sha256: String,
    pub caption_digest: String,
    pub caption_bytes: u64,
    pub result: Option<String>,
    pub processed_seal: Option<CaptureSeal>,
    // Publication ambiguity survives a crash without advertising a terminal
    // failure while the actual commit is still running.
    pub commit_pending: bool,
}

impl Record {
    pub fn new(
        identity: CaptureIdentity,
        format: CaptureFormat,
        request: CaptureLiveRequest,
        generation: String,
        session: String,
        observation_ms: u64,
        memo: &str,
    ) -> Self {
        Self {
            version: 1,
            snapshot: CaptureLiveSnapshot {
                capture_id: identity.capture_id.clone(),
                capture_fingerprint: request.capture_fingerprint.clone(),
                generation_id: generation,
                session_id: session,
                state: CaptureLiveState::Starting,
                failure: None,
                admitted_end_frame: 0,
                input_consumed_end_frame: 0,
                completed_through_frame: None,
                next_caption_sequence: 0,
                result_available: false,
                publication: None,
            },
            identity,
            format,
            request,
            observation_ms,
            created_at: chrono::Utc::now().to_rfc3339(),
            memo_sha256: hash(memo.as_bytes()),
            caption_digest: "0".repeat(64),
            caption_bytes: 0,
            result: None,
            processed_seal: None,
            commit_pending: false,
        }
    }

    pub fn work(&self) -> CaptureLiveWork {
        CaptureLiveWork {
            principal: self.identity.scope.member_id.clone(),
            session_id: self.snapshot.session_id.clone(),
            capture_id: self.identity.capture_id.clone(),
            generation_id: self.snapshot.generation_id.clone(),
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptionRecord {
    previous: String,
    caption: CaptureLiveCaption,
}

#[derive(Clone)]
pub(super) struct Journal(Arc<Inner>);

struct Inner {
    root: PathBuf,
    vault: String,
    records: Mutex<BTreeMap<String, Record>>,
    io: Arc<Semaphore>,
    _lease: File,
    _vault_lease: File,
    failed: std::sync::atomic::AtomicBool,
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
                .map_err(|_| unavailable("Live capture ledger already has an owner"))?;
            let vault_lease = context
                .lease
                .as_ref()
                .ok_or_else(|| unavailable("Vault lease required"))?
                .try_clone()
                .map_err(storage)?;
            let vault = context.events.vault_id();
            let mut records = read_records(&root, &vault)?;
            for record in records.values_mut() {
                if !record.snapshot.state.is_terminal() {
                    record.snapshot.state = if record.commit_pending {
                        CaptureLiveState::Uncertain
                    } else {
                        CaptureLiveState::Interrupted
                    };
                    record.snapshot.failure = Some(if record.commit_pending {
                        CaptureLiveFailure::Storage
                    } else {
                        CaptureLiveFailure::Interrupted
                    });
                    record.snapshot.completed_through_frame = None;
                    persist(&root.join(&record.snapshot.generation_id), record)?;
                }
            }
            Ok(Self(Arc::new(Inner {
                root,
                vault,
                records: Mutex::new(records),
                io: Arc::new(Semaphore::new(2)),
                _lease: lease,
                _vault_lease: vault_lease,
                failed: false.into(),
            })))
        })
        .await
        .map_err(storage)?
    }

    async fn run<T: Send + 'static>(
        &self,
        action: impl FnOnce(&Inner, &mut BTreeMap<String, Record>) -> Result<T, ApplicationError>
        + Send
        + 'static,
    ) -> Result<T, ApplicationError> {
        let permit = self
            .0
            .io
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable("Live storage busy"))?;
        let owner = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            if owner.failed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(storage("Live ledger needs review"));
            }
            let mut records = owner.records.lock().map_err(storage)?;
            action(&owner, &mut records)
        })
        .await
        .map_err(storage)?
    }

    pub async fn get(&self, member: String, id: String) -> Result<Record, ApplicationError> {
        self.run(move |_, records| {
            records
                .get(&id)
                .filter(|r| r.identity.scope.member_id == member)
                .cloned()
                .ok_or_else(not_found)
        })
        .await
    }

    pub async fn admit(
        &self,
        record: Record,
        memo: String,
    ) -> Result<(Record, bool), ApplicationError> {
        self.run(move |inner, records| {
            let id = &record.snapshot.generation_id;
            if let Some(old) = records.get(id) {
                if old.identity != record.identity
                    || old.format != record.format
                    || old.request != record.request
                {
                    return Err(conflict("Live request identity changed"));
                }
                return Ok((old.clone(), false));
            }
            if records.len() >= MAX_GENERATIONS {
                return Err(capacity());
            }
            if records.values().any(|r| {
                r.identity.capture_id == record.identity.capture_id
                    && !r.snapshot.state.is_terminal()
            }) {
                return Err(conflict("Capture already has an active live generation"));
            }
            validate(&record, &inner.vault, id)?;
            if memo.len() as u64 > MAX_MEMO_BYTES || hash(memo.as_bytes()) != record.memo_sha256 {
                return Err(capacity());
            }
            let bytes = serde_json::to_vec(&record).map_err(storage)?;
            if bytes.len() as u64 > MANIFEST_BYTES {
                return Err(capacity());
            }
            let directory = inner.root.join(id);
            if directory.try_exists().map_err(storage)? {
                return Err(storage("Unclaimed live generation path"));
            }
            ensure_directory(&directory)?;
            write_new(&directory.join("memo.json"), memo.as_bytes())?;
            if let Err(cause) = write_new(&directory.join("record.json"), &bytes) {
                inner
                    .failed
                    .store(true, std::sync::atomic::Ordering::Release);
                return Err(cause);
            }
            records.insert(id.clone(), record.clone());
            Ok((record, true))
        })
        .await
    }

    pub async fn change(
        &self,
        id: String,
        change: impl FnOnce(&mut Record) + Send + 'static,
    ) -> Result<Record, ApplicationError> {
        self.run(move |inner, records| {
            let old = records.get(&id).ok_or_else(not_found)?.clone();
            let mut record = old.clone();
            change(&mut record);
            validate(&record, &inner.vault, &id)?;
            checked_persist(inner, &old, &record)?;
            records.insert(id, record.clone());
            Ok(record)
        })
        .await
    }

    pub async fn append(
        &self,
        id: String,
        caption: CaptureLiveCaption,
    ) -> Result<Record, ApplicationError> {
        self.run(move |inner, records| {
            let old = records.get(&id).ok_or_else(not_found)?.clone();
            if caption.generation_id != id
                || caption.sequence != old.snapshot.next_caption_sequence
                || validate_capture_live_caption(&caption, &old.format.channels).is_err()
            {
                return Err(storage("Invalid live caption"));
            }
            let bytes = serde_json::to_vec(&CaptionRecord {
                previous: old.caption_digest.clone(),
                caption,
            })
            .map_err(storage)?;
            let next_bytes = old
                .caption_bytes
                .checked_add(bytes.len() as u64)
                .ok_or_else(capacity)?;
            if bytes.len() > CAPTURE_LIVE_CAPTION_LIMIT
                || next_bytes > MAX_CAPTION_BYTES
                || old.snapshot.next_caption_sequence >= MAX_CAPTIONS
            {
                return Err(capacity());
            }
            let path = inner
                .root
                .join(&id)
                .join(caption_name(old.snapshot.next_caption_sequence));
            if let Err(cause) = write_new(&path, &bytes) {
                inner
                    .failed
                    .store(true, std::sync::atomic::Ordering::Release);
                return Err(cause);
            }
            let mut record = old.clone();
            record.caption_bytes = next_bytes;
            record.caption_digest = hash(&bytes);
            record.snapshot.next_caption_sequence += 1;
            checked_persist(inner, &old, &record)?;
            records.insert(id, record.clone());
            Ok(record)
        })
        .await
    }

    pub async fn page(
        &self,
        member: String,
        id: String,
        after: Option<u64>,
        limit: u32,
    ) -> Result<CaptureLiveCaptionPage, ApplicationError> {
        if !(1..=128).contains(&limit) {
            return Err(capacity());
        }
        self.run(move |inner, records| {
            let record = records
                .get(&id)
                .filter(|r| r.identity.scope.member_id == member)
                .ok_or_else(not_found)?;
            let start = after
                .map(|n| n.checked_add(1).ok_or_else(capacity))
                .transpose()?
                .unwrap_or(0);
            if start > record.snapshot.next_caption_sequence {
                return Err(conflict("Invalid live caption cursor"));
            }
            let mut previous = if start == 0 {
                "0".repeat(64)
            } else {
                hash(&read(
                    &inner.root.join(&id).join(caption_name(start - 1)),
                    CAPTURE_LIVE_CAPTION_LIMIT as u64,
                )?)
            };
            let mut captions = Vec::new();
            let mut total = 0;
            for sequence in start
                ..record
                    .snapshot
                    .next_caption_sequence
                    .min(start + u64::from(limit))
            {
                let bytes = read(
                    &inner.root.join(&id).join(caption_name(sequence)),
                    CAPTURE_LIVE_CAPTION_LIMIT as u64,
                )?;
                let item: CaptionRecord = serde_json::from_slice(&bytes).map_err(storage)?;
                if item.previous != previous
                    || item.caption.sequence != sequence
                    || item.caption.generation_id != id
                    || validate_capture_live_caption(&item.caption, &record.format.channels)
                        .is_err()
                    || serde_json::to_vec(&item).map_err(storage)? != bytes
                {
                    return Err(storage("Live caption chain changed"));
                }
                previous = hash(&bytes);
                total += bytes.len();
                if total > CAPTURE_LIVE_PAGE_LIMIT {
                    break;
                }
                captions.push(item.caption);
            }
            let end = start + captions.len() as u64;
            if end == record.snapshot.next_caption_sequence && previous != record.caption_digest {
                return Err(storage("Live caption digest changed"));
            }
            Ok(CaptureLiveCaptionPage {
                snapshot: record.snapshot.clone(),
                next_cursor: (end < record.snapshot.next_caption_sequence)
                    .then(|| end.checked_sub(1))
                    .flatten(),
                captions,
            })
        })
        .await
    }

    pub async fn memo(&self, record: &Record) -> Result<String, ApplicationError> {
        let record = record.clone();
        self.run(move |inner, _| {
            let bytes = read(
                &inner
                    .root
                    .join(&record.snapshot.generation_id)
                    .join("memo.json"),
                MAX_MEMO_BYTES,
            )?;
            if hash(&bytes) != record.memo_sha256 {
                return Err(storage("Live memo changed"));
            }
            String::from_utf8(bytes).map_err(storage)
        })
        .await
    }

    pub async fn retain_result(
        &self,
        id: String,
        transcript: TranscriptWithData,
        seal: CaptureSeal,
    ) -> Result<Record, ApplicationError> {
        self.run(move |inner, records| {
            let old = records.get(&id).ok_or_else(not_found)?.clone();
            let bytes = encode_result(&transcript)?;
            let mut record = old.clone();
            record.result = Some(hash(&bytes));
            record.processed_seal = Some(seal);
            record.snapshot.result_available = true;
            validate(&record, &inner.vault, &id)?;
            write_new(&inner.root.join(&id).join("result.json"), &bytes)?;
            checked_persist(inner, &old, &record)?;
            records.insert(id, record.clone());
            Ok(record)
        })
        .await
    }

    pub async fn result(
        &self,
        member: String,
        id: String,
    ) -> Result<Option<TranscriptWithData>, ApplicationError> {
        self.run(move |inner, records| {
            let record = records
                .get(&id)
                .filter(|r| r.identity.scope.member_id == member)
                .ok_or_else(not_found)?;
            let Some(expected) = &record.result else {
                return Ok(None);
            };
            let bytes = read(
                &inner.root.join(&id).join("result.json"),
                TRANSCRIPTION_RESULT_LIMIT as u64,
            )?;
            if hash(&bytes) != *expected {
                return Err(storage("Live result changed"));
            }
            let mut data: TranscriptJson = serde_json::from_slice(&bytes).map_err(storage)?;
            if data.transcripts.len() != 1 {
                return Err(storage("Invalid live result"));
            }
            let transcript = data.transcripts.pop().unwrap();
            if encode_result(&transcript)? != bytes
                || transcript.session_id != record.snapshot.session_id
                || transcript.user_id != record.identity.scope.member_id
                || transcript.created_at != record.created_at
                || transcript.started_at != record.observation_ms as f64
                || transcript.ended_at.is_some()
                || hash(transcript.memo_md.as_bytes()) != record.memo_sha256
            {
                return Err(storage("Live result metadata changed"));
            }
            Ok(Some(transcript))
        })
        .await
    }

    pub fn blocks(&self, session: &str) -> bool {
        self.0.failed.load(std::sync::atomic::Ordering::Acquire)
            || self.0.records.lock().map_or(true, |records| {
                records.values().any(|r| record_blocks(r, session))
            })
    }
}

fn record_blocks(record: &Record, session: &str) -> bool {
    record.snapshot.session_id == session
        && (record.commit_pending
            || !record.snapshot.state.is_terminal()
            || record.snapshot.state == CaptureLiveState::Uncertain)
}

fn read_records(
    root: &std::path::Path,
    vault: &str,
) -> Result<BTreeMap<String, Record>, ApplicationError> {
    let mut records = BTreeMap::new();
    for (index, entry) in std::fs::read_dir(root).map_err(storage)?.enumerate() {
        if index >= 256 {
            return Err(capacity());
        }
        let entry = entry.map_err(storage)?;
        let name = entry.file_name();
        let Some(id) = name.to_str().filter(|id| valid_uuid(id)) else {
            if name
                .to_str()
                .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
            {
                return Err(storage("Noncanonical live ledger identity"));
            }
            continue;
        };
        if records.len() >= MAX_GENERATIONS {
            return Err(capacity());
        }
        let metadata = std::fs::symlink_metadata(entry.path()).map_err(storage)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(storage("Unsafe live ledger entry"));
        }
        let record: Record =
            serde_json::from_slice(&read(&entry.path().join("record.json"), MANIFEST_BYTES)?)
                .map_err(storage)?;
        validate(&record, vault, id)?;
        records.insert(id.to_owned(), record);
    }
    Ok(records)
}

pub(super) async fn ensure_idle_unopened(
    root: PathBuf,
    vault: String,
    session: String,
    vault_lease: Option<File>,
) -> Result<(), ApplicationError> {
    tokio::task::spawn_blocking(move || {
        let owned_vault_lease = vault_lease;
        if !root.is_absolute() {
            return Err(storage("Live capture path is not absolute"));
        }
        let mut exists = false;
        for parent in root.ancestors() {
            match std::fs::symlink_metadata(parent) {
                Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
                    exists |= parent == root
                }
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(storage("Unsafe retained live capture directory")),
            }
        }
        if !exists {
            return Ok(());
        }
        if owned_vault_lease.is_none() {
            return Err(unavailable(
                "Vault ownership required for retained live capture review",
            ));
        }
        let lock = root.join(".lock");
        let owned_ledger_lease = if check_file(&lock)? {
            let file = File::options()
                .read(true)
                .write(true)
                .open(&lock)
                .map_err(storage)?;
            file.try_lock()
                .map_err(|_| unavailable("Retained live capture still has an owner"))?;
            Some(file)
        } else {
            None
        };
        let records = read_records(&root, &vault)?;
        if (owned_ledger_lease.is_none() && !records.is_empty())
            || records
                .values()
                .any(|record| record_blocks(record, &session))
        {
            return Err(unavailable(
                "Retained live capture requires recovery or review",
            ));
        }
        Ok(())
    })
    .await
    .map_err(storage)?
}

fn caption_name(sequence: u64) -> String {
    format!("{sequence:016x}.caption.json")
}

fn validate(record: &Record, vault: &str, id: &str) -> Result<(), ApplicationError> {
    if record.version != 1
        || !valid_uuid(id)
        || record.snapshot.generation_id != id
        || record.identity.scope.vault_id != vault
        || record.identity.capture_id != record.snapshot.capture_id
        || record.snapshot.capture_fingerprint != record.request.capture_fingerprint
        || hypr_loofah_protocol::capture_fingerprint(&record.identity, &record.format)
            != record.request.capture_fingerprint
        || validate_capture_format(&record.format).is_err()
        || !valid_uuid(&record.identity.capture_id)
        || !valid_uuid(&record.identity.request_id)
        || validate_capture_live_request(&record.request).is_err()
        || record.observation_ms > (1_u64 << 53) - 1
        || chrono::DateTime::parse_from_rfc3339(&record.created_at).is_err()
        || record.snapshot.next_caption_sequence > MAX_CAPTIONS
        || record.caption_bytes > MAX_CAPTION_BYTES
        || !is_transcription_sha256(&record.caption_digest)
        || !is_transcription_sha256(&record.memo_sha256)
        || record
            .result
            .as_ref()
            .is_some_and(|s| !is_transcription_sha256(s))
        || record.snapshot.result_available != record.result.is_some()
        || (record.result.is_some() && record.processed_seal.is_none())
        || record.processed_seal.as_ref().is_some_and(|seal| {
            seal.capture_fingerprint != record.request.capture_fingerprint
                || seal.frames != record.snapshot.admitted_end_frame
                || seal
                    .frames
                    .checked_mul(record.format.channels.len() as u64 * 4)
                    != Some(seal.pcm_bytes)
                || !is_transcription_sha256(&seal.source_sha256)
                || !is_transcription_sha256(&seal.chain_digest)
        })
        || record.snapshot.admitted_end_frame > (1_u64 << 53) - 1
        || record.snapshot.input_consumed_end_frame > record.snapshot.admitted_end_frame
        || (record.snapshot.completed_through_frame.is_some()
            && matches!(
                record.snapshot.state,
                CaptureLiveState::Starting
                    | CaptureLiveState::Running
                    | CaptureLiveState::Finishing
            ))
        || record.snapshot.completed_through_frame.is_some_and(|n| {
            n != record.snapshot.admitted_end_frame || n != record.snapshot.input_consumed_end_frame
        })
        || (record.commit_pending
            && !matches!(
                record.snapshot.state,
                CaptureLiveState::Publishing | CaptureLiveState::Uncertain
            ))
        || (record.snapshot.state == CaptureLiveState::Publishing && !record.commit_pending)
        || record.snapshot.publication.is_some()
            != (record.snapshot.state == CaptureLiveState::Completed)
        || record
            .snapshot
            .publication
            .as_ref()
            .is_some_and(|p| !is_transcription_sha256(&p.revision) || !valid_uuid(&p.transcript_id))
        || (matches!(
            record.snapshot.state,
            CaptureLiveState::Completed | CaptureLiveState::Empty
        ) && (!record.snapshot.result_available
            || record.snapshot.completed_through_frame.is_none()))
        || (record.snapshot.next_caption_sequence == 0
            && (record.caption_bytes != 0 || record.caption_digest != "0".repeat(64)))
        || !matches!(&record.identity.target, CaptureTarget::ExistingSession { session_id, expected_audio_sha256: None } if session_id == &record.snapshot.session_id)
    {
        return Err(storage("Invalid live capture manifest"));
    }
    Ok(())
}

fn persist(directory: &std::path::Path, record: &Record) -> Result<(), ApplicationError> {
    let bytes = serde_json::to_string(record).map_err(storage)?;
    if bytes.len() as u64 > MANIFEST_BYTES {
        return Err(capacity());
    }
    hypr_storage::fs::atomic_write(&directory.join("record.json"), &bytes).map_err(storage)?;
    sync_parent(&directory.join("record.json"))
}

fn checked_persist(inner: &Inner, old: &Record, record: &Record) -> Result<(), ApplicationError> {
    let directory = inner.root.join(&record.snapshot.generation_id);
    let current: Record =
        serde_json::from_slice(&read(&directory.join("record.json"), MANIFEST_BYTES)?)
            .map_err(storage)?;
    if &current != old {
        inner
            .failed
            .store(true, std::sync::atomic::Ordering::Release);
        return Err(storage("Live manifest changed outside its owner"));
    }
    if let Err(cause) = persist(&directory, record) {
        inner
            .failed
            .store(true, std::sync::atomic::Ordering::Release);
        return Err(cause);
    }
    Ok(())
}
