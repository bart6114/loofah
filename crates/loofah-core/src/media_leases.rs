#[cfg(test)]
mod attachment_tests;
#[cfg(test)]
mod shutdown_tests;
mod waveform;

use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Seek, SeekFrom},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use hypr_loofah_protocol::{
    ApplicationError, AttachmentLease, ErrorCode, FileMetadata, RecordingLease,
};
use sha2::{Digest, Sha256};
use tokio::sync::{OnceCell, OwnedSemaphorePermit, Semaphore};

use crate::{VaultContext, media::MediaSource, vault::error};

pub use hypr_loofah_protocol::RECORDING_CHUNK_LIMIT;
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);
const HASH_CHUNK_SIZE: usize = 64 * 1024;
const HASH_INDEX_BUDGET: usize = 64 * 1024 * 1024;
const HASH_INDEX_LIMIT: usize = 16 * 1024 * 1024;

/// Hosts supply an authenticated member or device-window generation as the owner.
/// A lease ID alone never grants access. Hosts must recheck vault authorization on every read.
#[derive(Clone)]
pub struct MediaLeases(Arc<Registry>);

struct Registry {
    stopping: AtomicBool,
    state: Mutex<State>,
    slots: Arc<Semaphore>,
    slot_capacity: u32,
    waveforms: Arc<Semaphore>,
    index_budget: Arc<Semaphore>,
    index_limit: usize,
    owner_limit: usize,
    idle_timeout: Duration,
}

#[derive(Default)]
struct State {
    active: BTreeMap<String, Arc<Entry>>,
    live: Vec<Weak<Entry>>,
}

struct Entry {
    id: String,
    owner: String,
    vault: String,
    session: String,
    source: MediaSource,
    touched: Mutex<Instant>,
    idle_timeout: Duration,
    closed: AtomicBool,
    recording: OnceCell<Result<Option<Arc<PinnedMedia>>, ApplicationError>>,
    // Close cached descriptors before releasing vault ownership and descriptor admission.
    context: Arc<VaultContext>,
    _slot: OwnedSemaphorePermit,
}

struct PinnedMedia {
    #[cfg(test)]
    reads_started: std::sync::atomic::AtomicU64,
    file: Mutex<File>,
    metadata: FileMetadata,
    modified: Option<SystemTime>,
    chunks: ChunkIndex,
    peaks: OnceCell<Arc<hypr_loofah_protocol::AudioPeaks>>,
}

struct ChunkIndex {
    hashes: Vec<[u8; 32]>,
    // Retain memory admission until all readers and cancelled blocking work finish.
    _memory: OwnedSemaphorePermit,
}

struct IndexedReader<R> {
    reader: R,
    hashes: Vec<[u8; 32]>,
    hasher: Sha256,
    pending: usize,
}

#[derive(Clone)]
pub struct MediaReader {
    recording: Arc<PinnedMedia>,
    entry: Arc<Entry>,
    waveforms: Arc<Semaphore>,
}

impl Default for MediaLeases {
    fn default() -> Self {
        Self::new(64, 8, IDLE_TIMEOUT)
    }
}

impl MediaLeases {
    fn new(capacity: usize, owner_limit: usize, idle_timeout: Duration) -> Self {
        Self::with_index_limits(
            capacity,
            owner_limit,
            idle_timeout,
            HASH_INDEX_BUDGET,
            HASH_INDEX_LIMIT,
        )
    }

    fn with_index_limits(
        capacity: usize,
        owner_limit: usize,
        idle_timeout: Duration,
        index_budget: usize,
        index_limit: usize,
    ) -> Self {
        Self(Arc::new(Registry {
            stopping: AtomicBool::new(false),
            state: Mutex::new(State::default()),
            slots: Arc::new(Semaphore::new(capacity)),
            slot_capacity: u32::try_from(capacity).expect("bounded media lease capacity"),
            waveforms: Arc::new(Semaphore::new(2)),
            index_budget: Arc::new(Semaphore::new(index_budget)),
            index_limit,
            owner_limit,
            idle_timeout,
        }))
    }

    /// The caller-generated UUID makes a lost open response safely retryable within this process.
    /// Handles are ephemeral and must be reopened after service restart or idle expiry.
    pub async fn open(
        &self,
        context: Arc<VaultContext>,
        owner: &str,
        id: &str,
        session: &str,
    ) -> Result<Option<RecordingLease>, ApplicationError> {
        self.open_media(context, owner, id, session, MediaSource::Recording)
            .await
            .map(|opened| opened.map(|(entry, media)| entry.info(&media, self.0.idle_timeout)))
    }

    pub async fn open_attachment(
        &self,
        context: Arc<VaultContext>,
        owner: &str,
        id: &str,
        session: &str,
        attachment_id: &str,
    ) -> Result<Option<AttachmentLease>, ApplicationError> {
        self.open_media(
            context,
            owner,
            id,
            session,
            MediaSource::Attachment(attachment_id.into()),
        )
        .await
        .map(|opened| {
            opened.map(|(entry, media)| AttachmentLease {
                id: entry.id.clone(),
                vault_id: entry.vault.clone(),
                session_id: entry.session.clone(),
                attachment_id: attachment_id.into(),
                metadata: media.metadata.clone(),
                idle_timeout_secs: self.0.idle_timeout.as_secs() as u32,
            })
        })
    }

    async fn open_media(
        &self,
        context: Arc<VaultContext>,
        owner: &str,
        id: &str,
        session: &str,
        source: MediaSource,
    ) -> Result<Option<(Arc<Entry>, Arc<PinnedMedia>)>, ApplicationError> {
        context.validate_media(session, &source)?;
        let id = normalize_id(id)?;
        let entry = {
            let mut state = self.0.state.lock().unwrap();
            if self.0.stopping.load(Ordering::Acquire) {
                return Err(error(
                    ErrorCode::Unavailable,
                    "Media service is shutting down",
                ));
            }
            self.prune(&mut state);
            if let Some(entry) = state.active.get(&id) {
                entry.check_scope(owner, &context.events.vault_id())?;
                if entry.session != session
                    || entry.source != source
                    || !Arc::ptr_eq(&entry.context, &context)
                {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Media lease identifies another request",
                    ));
                }
                entry.touch()?;
                entry.clone()
            } else {
                if owner.is_empty() || owner.len() > 512 {
                    return Err(error(
                        ErrorCode::Validation,
                        "Invalid recording lease owner",
                    ));
                }
                let owned = state
                    .live
                    .iter()
                    .filter_map(Weak::upgrade)
                    .filter(|entry| entry.owner == owner)
                    .count();
                if owned >= self.0.owner_limit {
                    return Err(capacity_error());
                }
                let slot = self
                    .0
                    .slots
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| capacity_error())?;
                let entry = Arc::new(Entry {
                    id: id.clone(),
                    owner: owner.to_owned(),
                    vault: context.events.vault_id(),
                    session: session.to_owned(),
                    source,
                    context,
                    _slot: slot,
                    touched: Mutex::new(Instant::now()),
                    idle_timeout: self.0.idle_timeout,
                    closed: AtomicBool::new(false),
                    recording: OnceCell::new(),
                });
                state.live.push(Arc::downgrade(&entry));
                state.active.insert(id, entry.clone());
                entry
            }
        };
        let leases = self.clone();
        // Opening and hashing retain admission and the vault lease after caller cancellation.
        tokio::spawn(async move {
            let result = entry
                .recording
                .get_or_init(|| async {
                    let budget = leases.0.index_budget.clone();
                    let limit = leases.0.index_limit;
                    entry
                        .context
                        .read_media(&entry.session, entry.source.clone(), move |source| {
                            let (source, metadata, after, chunks) =
                                crate::media::media_with_metadata(
                                    source,
                                    |filename, content_type, file, size| {
                                        ChunkIndex::collect(
                                            filename,
                                            content_type,
                                            file,
                                            size,
                                            budget,
                                            limit,
                                        )
                                    },
                                )?;
                            Ok(Arc::new(PinnedMedia {
                                #[cfg(test)]
                                reads_started: Default::default(),
                                file: Mutex::new(source.file),
                                metadata,
                                modified: after.modified().ok(),
                                chunks,
                                peaks: OnceCell::new(),
                            }))
                        })
                        .await
                })
                .await
                .clone();
            entry.touch()?;
            match result {
                Ok(Some(recording)) => Ok(Some((entry.clone(), recording))),
                other => {
                    leases.remove(&entry);
                    other.map(|_| None)
                }
            }
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Media lease request stopped"))?
    }

    pub fn reader(
        &self,
        owner: &str,
        vault: &str,
        id: &str,
    ) -> Result<MediaReader, ApplicationError> {
        self.reader_inner(owner, vault, id, false)
    }

    pub fn attachment_reader(
        &self,
        owner: &str,
        vault: &str,
        id: &str,
    ) -> Result<MediaReader, ApplicationError> {
        self.reader_inner(owner, vault, id, true)
    }

    fn reader_inner(
        &self,
        owner: &str,
        vault: &str,
        id: &str,
        attachment: bool,
    ) -> Result<MediaReader, ApplicationError> {
        let id = normalize_id(id)?;
        let mut state = self.0.state.lock().unwrap();
        if self.0.stopping.load(Ordering::Acquire) {
            return Err(error(
                ErrorCode::Unavailable,
                "Media service is shutting down",
            ));
        }
        self.prune(&mut state);
        let entry = state.active.get(&id).ok_or_else(not_found)?;
        entry.check_scope(owner, vault)?;
        if matches!(entry.source, MediaSource::Attachment(_)) != attachment {
            return Err(not_found());
        }
        entry.touch()?;
        let recording = match entry.recording.get() {
            Some(Ok(Some(recording))) => recording.clone(),
            _ => {
                return Err(error(ErrorCode::Unavailable, "Media lease is not ready"));
            }
        };
        Ok(MediaReader {
            entry: entry.clone(),
            recording,
            waveforms: self.0.waveforms.clone(),
        })
    }

    pub fn close(&self, owner: &str, vault: &str, id: &str) -> Result<(), ApplicationError> {
        self.close_inner(owner, vault, id, false)
    }

    pub fn close_attachment(
        &self,
        owner: &str,
        vault: &str,
        id: &str,
    ) -> Result<(), ApplicationError> {
        self.close_inner(owner, vault, id, true)
    }

    fn close_inner(
        &self,
        owner: &str,
        vault: &str,
        id: &str,
        attachment: bool,
    ) -> Result<(), ApplicationError> {
        let id = normalize_id(id)?;
        let mut state = self.0.state.lock().unwrap();
        self.prune(&mut state);
        if let Some(entry) = state.active.get(&id) {
            entry.check_scope(owner, vault)?;
            if matches!(entry.source, MediaSource::Attachment(_)) != attachment {
                return Err(not_found());
            }
            entry.closed.store(true, Ordering::Release);
            state.active.remove(&id);
        }
        Ok(())
    }

    pub fn close_owner(&self, owner: &str) {
        let mut state = self.0.state.lock().unwrap();
        state.active.retain(|_, entry| {
            if entry.owner == owner {
                entry.closed.store(true, Ordering::Release);
                false
            } else {
                true
            }
        });
        self.prune(&mut state);
    }

    pub fn close_recording_session(&self, vault: &str, session: &str) {
        self.close_source(vault, session, &MediaSource::Recording, None);
    }

    pub fn close_attachment_source(
        &self,
        vault: &str,
        session: &str,
        attachment: &str,
        expected_sha256: &str,
    ) {
        self.close_source(
            vault,
            session,
            &MediaSource::Attachment(attachment.into()),
            Some(expected_sha256),
        );
    }

    fn close_source(
        &self,
        vault: &str,
        session: &str,
        source: &MediaSource,
        expected_sha256: Option<&str>,
    ) {
        let mut state = self.0.state.lock().unwrap();
        state.active.retain(|_, entry| {
            if entry.vault == vault
                && entry.session == session
                && &entry.source == source
                && expected_sha256.is_none_or(|sha| match entry.recording.get() {
                    Some(Ok(Some(media))) => media.metadata.sha256 == sha,
                    _ => true,
                })
            {
                entry.closed.store(true, Ordering::Release);
                false
            } else {
                true
            }
        });
        self.prune(&mut state);
    }

    /// Terminal host shutdown, unlike reusable close_all. Pending opens register
    /// Entry before I/O; native hash/chunk/waveform workers retain it through exit.
    pub async fn shutdown(&self) {
        {
            let _state = self.0.state.lock().unwrap();
            self.0.stopping.store(true, Ordering::Release);
        }
        self.close_all();
        // _slot is Entry's final field: all permits return only AFTER descriptor
        // and Context teardown, unlike a weak strong-count reaching zero.
        let _drained = self
            .0
            .slots
            .clone()
            .acquire_many_owned(self.0.slot_capacity)
            .await
            .expect("media lease slots remain open");
        let mut state = self.0.state.lock().unwrap();
        state.live.retain(|entry| entry.strong_count() > 0);
    }

    pub fn close_all(&self) {
        let mut state = self.0.state.lock().unwrap();
        for entry in state.active.values() {
            entry.closed.store(true, Ordering::Release);
        }
        state.active.clear();
        self.prune(&mut state);
    }

    pub fn expire_idle(&self) {
        self.prune(&mut self.0.state.lock().unwrap());
    }

    fn prune(&self, state: &mut State) {
        state.active.retain(|_, entry| {
            let touched = entry.touched.lock().unwrap();
            let keep =
                !entry.closed.load(Ordering::Acquire) && touched.elapsed() < self.0.idle_timeout;
            if !keep {
                entry.closed.store(true, Ordering::Release);
            }
            keep
        });
        state.live.retain(|entry| entry.strong_count() > 0);
    }

    fn remove(&self, entry: &Arc<Entry>) {
        let mut state = self.0.state.lock().unwrap();
        entry.closed.store(true, Ordering::Release);
        if state
            .active
            .get(&entry.id)
            .is_some_and(|current| Arc::ptr_eq(current, entry))
        {
            state.active.remove(&entry.id);
        }
    }
}

impl Entry {
    fn check_scope(&self, owner: &str, vault: &str) -> Result<(), ApplicationError> {
        if self.owner != owner || self.vault != vault {
            return Err(not_found());
        }
        Ok(())
    }

    fn touch(&self) -> Result<(), ApplicationError> {
        let mut touched = self.touched.lock().unwrap();
        if self.closed.load(Ordering::Acquire) || touched.elapsed() >= self.idle_timeout {
            self.closed.store(true, Ordering::Release);
            return Err(not_found());
        }
        *touched = Instant::now();
        Ok(())
    }

    fn info(&self, recording: &PinnedMedia, idle_timeout: Duration) -> RecordingLease {
        RecordingLease {
            id: self.id.clone(),
            vault_id: self.vault.clone(),
            session_id: self.session.clone(),
            metadata: recording.metadata.clone(),
            idle_timeout_secs: idle_timeout.as_secs() as u32,
        }
    }
}

impl MediaReader {
    pub fn metadata(&self) -> &FileMetadata {
        &self.recording.metadata
    }

    pub async fn read_chunk(
        &self,
        offset: u64,
        length: usize,
    ) -> Result<Vec<u8>, ApplicationError> {
        self.read_chunk_with_guard(offset, length, ()).await
    }

    /// Keep host admission alive until blocking I/O ends, even if the caller cancels its future.
    pub async fn read_chunk_with_guard<G: Send + 'static>(
        &self,
        offset: u64,
        length: usize,
        admission: G,
    ) -> Result<Vec<u8>, ApplicationError> {
        if length == 0
            || length > RECORDING_CHUNK_LIMIT
            || offset >= self.recording.metadata.size_bytes
        {
            return Err(error(ErrorCode::Validation, "Invalid recording byte range"));
        }
        self.entry.touch()?;
        let reader = self.clone();
        tokio::task::spawn_blocking(move || {
            let _admission = admission;
            reader.read_chunk_blocking(offset, length)
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Recording reader stopped"))?
    }

    fn read_chunk_blocking(&self, offset: u64, length: usize) -> Result<Vec<u8>, ApplicationError> {
        self.entry.touch()?;
        #[cfg(test)]
        self.recording.reads_started.fetch_add(1, Ordering::Release);
        let mut file = self.recording.file.lock().unwrap();
        self.verify(&file)?;
        let length = (self.recording.metadata.size_bytes - offset).min(length as u64) as usize;
        let end = offset + length as u64;
        let first = offset / HASH_CHUNK_SIZE as u64;
        let last = (end - 1) / HASH_CHUNK_SIZE as u64;
        file.seek(SeekFrom::Start(first * HASH_CHUNK_SIZE as u64))
            .map_err(read_error)?;
        let mut bytes = Vec::with_capacity(length);
        let mut buffer = [0; HASH_CHUNK_SIZE];
        for index in first..=last {
            let start = index * HASH_CHUNK_SIZE as u64;
            let count =
                (self.recording.metadata.size_bytes - start).min(HASH_CHUNK_SIZE as u64) as usize;
            file.read_exact(&mut buffer[..count]).map_err(|cause| {
                if cause.kind() == std::io::ErrorKind::UnexpectedEof {
                    self.changed()
                } else {
                    read_error(cause)
                }
            })?;
            let hash: [u8; 32] = Sha256::digest(&buffer[..count]).into();
            if hash != self.recording.chunks.hashes[index as usize] {
                return Err(self.changed());
            }
            let from = offset.saturating_sub(start) as usize;
            let to = (end - start).min(count as u64) as usize;
            bytes.extend_from_slice(&buffer[from..to]);
        }
        self.verify(&file)?;
        self.entry.touch()?;
        Ok(bytes)
    }

    fn verify(&self, file: &File) -> Result<(), ApplicationError> {
        let metadata = file.metadata().map_err(read_error)?;
        if metadata.len() != self.recording.metadata.size_bytes
            || metadata.modified().ok() != self.recording.modified
        {
            return Err(self.changed());
        }
        Ok(())
    }

    fn changed(&self) -> ApplicationError {
        self.entry.closed.store(true, Ordering::Release);
        changed_error()
    }
}

impl ChunkIndex {
    fn collect(
        filename: &str,
        content_type: &str,
        file: &mut File,
        size: u64,
        budget: Arc<Semaphore>,
        limit: usize,
    ) -> Result<(FileMetadata, Self), ApplicationError> {
        let count = size.div_ceil(HASH_CHUNK_SIZE as u64);
        let memory = count.checked_mul(32).ok_or_else(index_limit_error)?;
        if memory > limit as u64 || memory > u32::MAX as u64 {
            return Err(index_limit_error());
        }
        let permit = budget
            .try_acquire_many_owned(memory as u32)
            .map_err(|_| capacity_error())?;
        let mut hashes = Vec::new();
        hashes
            .try_reserve_exact(count as usize)
            .map_err(|_| capacity_error())?;
        // A growing file cannot expand the admitted index while hashing.
        let mut indexed = IndexedReader {
            reader: file.take(size),
            hashes,
            hasher: Sha256::new(),
            pending: 0,
        };
        let metadata =
            hypr_fs_sync_core::media::metadata_from_reader(filename, content_type, &mut indexed)
                .map_err(read_error)?;
        if indexed.pending != 0 {
            indexed.hashes.push(indexed.hasher.finalize().into());
        }
        Ok((
            metadata,
            Self {
                hashes: indexed.hashes,
                _memory: permit,
            },
        ))
    }
}

impl<R: Read> Read for IndexedReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let count = self.reader.read(buffer)?;
        let mut bytes = &buffer[..count];
        while !bytes.is_empty() {
            let take = bytes.len().min(HASH_CHUNK_SIZE - self.pending);
            self.hasher.update(&bytes[..take]);
            self.pending += take;
            bytes = &bytes[take..];
            if self.pending == HASH_CHUNK_SIZE {
                self.hashes
                    .push(std::mem::take(&mut self.hasher).finalize().into());
                self.pending = 0;
            }
        }
        Ok(count)
    }
}

fn index_limit_error() -> ApplicationError {
    error(
        ErrorCode::ResourceLimit,
        "Media exceeds the file verification limit",
    )
}

fn normalize_id(id: &str) -> Result<String, ApplicationError> {
    uuid::Uuid::parse_str(id)
        .map(|id| id.to_string())
        .map_err(|_| error(ErrorCode::Validation, "Invalid media lease identifier"))
}
fn not_found() -> ApplicationError {
    error(ErrorCode::NotFound, "Media lease not found or expired")
}
fn changed_error() -> ApplicationError {
    error(
        ErrorCode::Unavailable,
        "Media changed; reopen it before continuing",
    )
}
fn capacity_error() -> ApplicationError {
    let mut cause = error(
        ErrorCode::ResourceLimit,
        "Media lease capacity reached; close idle recordings and retry",
    );
    cause.retryable = true;
    cause
}
fn read_error(cause: std::io::Error) -> ApplicationError {
    tracing::error!(%cause, "media read failed");
    error(ErrorCode::Internal, "Cannot read media")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_vault_write::SessionStore;

    pub(super) async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>) {
        let root = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(root.path().into()));
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Title","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Unchanged note")
            .await
            .unwrap();
        std::fs::write(root.path().join("sessions/note/audio.wav"), b"0123456789").unwrap();
        let context = Arc::new(VaultContext::new(
            "vault".into(),
            store,
            StartupState::new(Some(root.path())),
            Arc::new(ConfigState::load_or_default(root.path())),
            Arc::new(SearchService::default()),
        ));
        (root, context)
    }

    fn id() -> String {
        uuid::Uuid::new_v4().to_string()
    }

    #[tokio::test]
    async fn retry_and_ranges_keep_one_generation_without_rehashing_or_holding_the_store_lock() {
        let (root, context) = fixture().await;
        let leases = MediaLeases::default();
        let id = id();
        let info = leases
            .open(context.clone(), "alice", &id, "note")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(info.idle_timeout_secs, 600);
        assert_eq!(info.metadata.size_bytes, 10);
        let reader = leases.reader("alice", "vault", &id).unwrap();
        assert_eq!(reader.read_chunk(2, 3).await.unwrap(), b"234");
        assert_eq!(reader.read_chunk(8, 100).await.unwrap(), b"89");
        for (offset, length) in [
            (10, 1),
            (0, 0),
            (0, RECORDING_CHUNK_LIMIT + 1),
            (u64::MAX, 1),
        ] {
            assert_eq!(
                reader.read_chunk(offset, length).await.unwrap_err().code,
                ErrorCode::Validation
            );
        }
        let directory = root.path().join("sessions/note");
        std::fs::rename(
            directory.join("audio.wav"),
            directory.join("retired-user.wav"),
        )
        .unwrap();
        std::fs::write(directory.join("audio.wav"), b"Replacement").unwrap();
        assert_eq!(
            leases
                .open(context.clone(), "alice", &id, "note")
                .await
                .unwrap(),
            Some(info.clone())
        );
        assert_eq!(reader.read_chunk(0, 10).await.unwrap(), b"0123456789");
        let replacement = leases
            .open(context.clone(), "alice", &self::id(), "note")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(replacement.metadata.sha256, info.metadata.sha256);
        tokio::time::timeout(
            Duration::from_secs(2),
            context.store.save_note("note", "Still responsive", None),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            std::fs::read(directory.join("retired-user.wav")).unwrap(),
            b"0123456789"
        );
    }

    #[tokio::test]
    async fn identifiers_are_scoped_to_the_authenticated_owner_vault_and_session() {
        let (_root, context) = fixture().await;
        let leases = MediaLeases::default();
        assert_eq!(
            leases
                .open(context.clone(), "alice", "../audio.wav", "note")
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
        let id = id();
        leases
            .open(context.clone(), "alice", &id, "note")
            .await
            .unwrap();
        for (owner, vault) in [("bob", "vault"), ("alice", "another-vault")] {
            assert_eq!(
                leases.reader(owner, vault, &id).err().unwrap().code,
                ErrorCode::NotFound
            );
            assert_eq!(
                leases.close(owner, vault, &id).unwrap_err().code,
                ErrorCode::NotFound
            );
        }
        assert_eq!(
            leases
                .open(context.clone(), "bob", &id, "note")
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            leases
                .open(context, "alice", &id, "different")
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        let reader = leases.reader("alice", "vault", &id).unwrap();
        leases.close("alice", "vault", &id).unwrap();
        leases.close("alice", "vault", &id).unwrap();
        assert_eq!(
            reader.read_chunk(0, 1).await.unwrap_err().code,
            ErrorCode::NotFound
        );
    }

    #[tokio::test]
    async fn idle_expiry_close_owner_and_shutdown_release_resources_without_resurrecting_readers() {
        let (_root, context) = fixture().await;
        let leases = MediaLeases::default();
        let alice = id();
        let bob = id();
        leases
            .open(context.clone(), "alice", &alice, "note")
            .await
            .unwrap();
        leases.open(context, "bob", &bob, "note").await.unwrap();
        let reader = leases.reader("alice", "vault", &alice).unwrap();
        *reader.entry.touched.lock().unwrap() = Instant::now() - IDLE_TIMEOUT;
        assert_eq!(
            reader.read_chunk(0, 1).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        leases.expire_idle();
        assert!(leases.reader("alice", "vault", &alice).is_err());
        assert_eq!(
            reader.read_chunk(0, 1).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert!(leases.reader("bob", "vault", &bob).is_ok());
        leases.close_owner("alice");
        let bob_reader = leases.reader("bob", "vault", &bob).unwrap();
        leases.close_all();
        assert_eq!(
            bob_reader.read_chunk(0, 1).await.unwrap_err().code,
            ErrorCode::NotFound
        );
        drop(reader);
        drop(bob_reader);
        assert_eq!(leases.0.slots.available_permits(), 64);
    }

    #[tokio::test]
    async fn changed_in_place_recordings_fail_closed_and_missing_or_failed_opens_release_capacity()
    {
        let (root, context) = fixture().await;
        let leases = MediaLeases::new(1, 1, IDLE_TIMEOUT);
        assert_eq!(
            leases
                .open(context.clone(), "alice", &id(), "missing")
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        let path = root.path().join("sessions/note/audio.wav");
        std::fs::remove_file(&path).unwrap();
        assert!(
            leases
                .open(context.clone(), "alice", &id(), "note")
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(leases.0.slots.available_permits(), 1);
        std::fs::write(&path, b"Original").unwrap();
        let id = id();
        leases.open(context, "alice", &id, "note").await.unwrap();
        let reader = leases.reader("alice", "vault", &id).unwrap();
        std::fs::write(path, b"Changed length").unwrap();
        assert_eq!(
            reader.read_chunk(0, 4).await.unwrap_err().code,
            ErrorCode::Unavailable
        );
        assert!(leases.reader("alice", "vault", &id).is_err());
        drop(reader);
        assert_eq!(leases.0.slots.available_permits(), 1);
    }

    #[tokio::test]
    async fn ranges_verify_full_overlapping_chunks_even_when_size_and_timestamp_are_unchanged() {
        let (root, context) = fixture().await;
        let path = root.path().join("sessions/note/audio.wav");
        let original: Vec<u8> = (0..(HASH_CHUNK_SIZE * 2 + 17))
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&path, &original).unwrap();
        let leases = MediaLeases::default();
        let id = id();
        let info = leases
            .open(context.clone(), "alice", &id, "note")
            .await
            .unwrap()
            .unwrap();
        let reader = leases.reader("alice", "vault", &id).unwrap();
        for (offset, length) in [
            (0, 1),
            (23, 100),
            (HASH_CHUNK_SIZE - 1, 2),
            (HASH_CHUNK_SIZE, HASH_CHUNK_SIZE + 17),
            (original.len() - 1, 100),
        ] {
            assert_eq!(
                reader.read_chunk(offset as u64, length).await.unwrap(),
                original[offset..(offset + length).min(original.len())]
            );
        }

        let before = std::fs::metadata(&path).unwrap();
        let modified = before.modified().unwrap();
        let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        let mut writer = &file;
        writer
            .seek(SeekFrom::Start((HASH_CHUNK_SIZE + 7) as u64))
            .unwrap();
        std::io::Write::write_all(&mut writer, b"changed").unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let after = file.metadata().unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(after.modified().unwrap(), modified);

        // Unchanged chunks still match the opened representation. A read elsewhere in
        // the changed chunk must fail even when its requested bytes were untouched.
        assert_eq!(reader.read_chunk(0, 10).await.unwrap(), original[..10]);
        assert_eq!(
            reader
                .read_chunk(HASH_CHUNK_SIZE as u64, 1)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Unavailable
        );
        assert!(leases.reader("alice", "vault", &id).is_err());
        let replacement = leases
            .open(context, "alice", &self::id(), "note")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(replacement.metadata.sha256, info.metadata.sha256);
        assert_eq!(
            std::fs::read_to_string(root.path().join("sessions/note/notes.md")).unwrap(),
            "Unchanged note"
        );
    }

    #[tokio::test]
    async fn hash_memory_admission_counts_readers_after_close_and_releases_failed_opens() {
        let (root, context) = fixture().await;
        let file = File::options()
            .write(true)
            .open(root.path().join("sessions/note/audio.wav"))
            .unwrap();
        file.set_len(HASH_CHUNK_SIZE as u64 + 1).unwrap();
        let leases = MediaLeases::with_index_limits(4, 4, IDLE_TIMEOUT, 96, 64);
        let id = id();
        leases
            .open(context.clone(), "alice", &id, "note")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(leases.0.index_budget.available_permits(), 32);
        let reader = leases.reader("alice", "vault", &id).unwrap();
        leases.close("alice", "vault", &id).unwrap();
        let failed_id = self::id();
        let busy = leases
            .open(context.clone(), "bob", &failed_id, "note")
            .await
            .unwrap_err();
        assert_eq!(busy.code, ErrorCode::ResourceLimit);
        assert!(busy.retryable);
        assert_eq!(leases.0.slots.available_permits(), 3);
        drop(reader);
        assert_eq!(leases.0.index_budget.available_permits(), 96);
        leases
            .open(context, "bob", &failed_id, "note")
            .await
            .unwrap()
            .unwrap();
        leases.close_all();
        assert_eq!(leases.0.index_budget.available_permits(), 96);
        assert_eq!(leases.0.slots.available_permits(), 4);
    }

    #[tokio::test]
    async fn oversized_recordings_are_rejected_before_hashing_and_do_not_leak_admission() {
        let (root, context) = fixture().await;
        let file = File::options()
            .write(true)
            .open(root.path().join("sessions/note/audio.wav"))
            .unwrap();
        file.set_len(HASH_CHUNK_SIZE as u64 + 1).unwrap();
        let leases = MediaLeases::with_index_limits(1, 1, IDLE_TIMEOUT, 64, 32);
        let rejected = leases
            .open(context.clone(), "alice", &id(), "note")
            .await
            .unwrap_err();
        assert_eq!(rejected.code, ErrorCode::ResourceLimit);
        assert!(!rejected.retryable);
        assert_eq!(leases.0.index_budget.available_permits(), 64);
        assert_eq!(leases.0.slots.available_permits(), 1);
        file.set_len(0).unwrap();
        let empty = leases
            .open(context, "alice", &id(), "note")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(empty.metadata.size_bytes, 0);
        assert_eq!(leases.0.index_budget.available_permits(), 64);
        leases.close_all();
        assert_eq!(leases.0.slots.available_permits(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_chunk_retains_ownership_and_admission_until_blocking_io_finishes() {
        let (_root, context) = fixture().await;
        let leases = MediaLeases::new(1, 1, IDLE_TIMEOUT);
        let id = id();
        leases
            .open(context.clone(), "alice", &id, "note")
            .await
            .unwrap();
        let reader = leases.reader("alice", "vault", &id).unwrap();
        let recording = reader.recording.clone();
        let block = recording.file.lock().unwrap();
        let caller = reader.clone();
        let host_capacity = Arc::new(Semaphore::new(1));
        let host_admission = host_capacity.clone().try_acquire_owned().unwrap();
        let request =
            tokio::spawn(async move { caller.read_chunk_with_guard(0, 4, host_admission).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while recording.reads_started.load(Ordering::Acquire) == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        assert_eq!(host_capacity.available_permits(), 0);
        let entry = Arc::downgrade(&reader.entry);
        leases.close("alice", "vault", &id).unwrap();
        drop(reader);
        assert!(entry.upgrade().is_some());
        assert_eq!(
            leases.0.index_budget.available_permits(),
            HASH_INDEX_BUDGET - 32
        );
        assert_eq!(
            leases
                .open(context.clone(), "bob", &self::id(), "note")
                .await
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
        context
            .store
            .save_note("note", "Cancelled playback does not block editing", None)
            .await
            .unwrap();
        drop(block);
        drop(recording);
        tokio::time::timeout(Duration::from_secs(2), async {
            while entry.upgrade().is_some() || leases.0.slots.available_permits() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(leases.0.slots.available_permits(), 1);
        assert_eq!(leases.0.index_budget.available_permits(), HASH_INDEX_BUDGET);
        assert_eq!(host_capacity.available_permits(), 1);
        assert!(
            leases
                .open(context, "bob", &self::id(), "note")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn cancelled_pending_open_cannot_resurrect_a_closed_handle() {
        let (_root, context) = fixture().await;
        let leases = MediaLeases::new(1, 1, IDLE_TIMEOUT);
        let snapshot = context.store.lock_snapshot().await;
        let id = id();
        let pending = {
            let leases = leases.clone();
            let context = context.clone();
            let id = id.clone();
            tokio::spawn(async move { leases.open(context, "alice", &id, "note").await })
        };
        tokio::time::timeout(Duration::from_secs(2), async {
            while leases.0.slots.available_permits() != 0
                || context.media_readers.available_permits() != 3
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        pending.abort();
        assert!(pending.await.unwrap_err().is_cancelled());
        leases.close("alice", "vault", &id).unwrap();
        assert_eq!(leases.0.slots.available_permits(), 0);
        drop(snapshot);
        tokio::time::timeout(Duration::from_secs(2), async {
            while leases.0.slots.available_permits() != 1 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(leases.reader("alice", "vault", &id).is_err());
        assert!(
            leases
                .open(context, "alice", &self::id(), "note")
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn owner_and_global_limits_count_live_readers_after_explicit_close() {
        let (_root, context) = fixture().await;
        let leases = MediaLeases::new(2, 1, IDLE_TIMEOUT);
        let id = id();
        leases
            .open(context.clone(), "alice", &id, "note")
            .await
            .unwrap();
        let reader = leases.reader("alice", "vault", &id).unwrap();
        leases.close("alice", "vault", &id).unwrap();
        assert_eq!(
            leases
                .open(context.clone(), "alice", &self::id(), "note")
                .await
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
        leases
            .open(context.clone(), "bob", &self::id(), "note")
            .await
            .unwrap();
        assert_eq!(
            leases
                .open(context.clone(), "charlie", &self::id(), "note")
                .await
                .unwrap_err()
                .code,
            ErrorCode::ResourceLimit
        );
        drop(reader);
        assert!(
            leases
                .open(context, "charlie", &self::id(), "note")
                .await
                .unwrap()
                .is_some()
        );
    }
}
