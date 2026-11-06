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

use hypr_loofah_protocol::{ApplicationError, AudioFileMetadata, ErrorCode, RecordingLease};
use tokio::sync::{OnceCell, OwnedSemaphorePermit, Semaphore};

use crate::{VaultContext, vault::error};

pub use hypr_loofah_protocol::RECORDING_CHUNK_LIMIT;
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);

/// Hosts supply an authenticated member or device-window generation as the owner.
/// A lease ID alone never grants access. Hosts must recheck vault authorization on every read.
#[derive(Clone)]
pub struct RecordingLeases(Arc<Registry>);

struct Registry {
    state: Mutex<State>,
    slots: Arc<Semaphore>,
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
    touched: Mutex<Instant>,
    idle_timeout: Duration,
    closed: AtomicBool,
    recording: OnceCell<Result<Option<Arc<Recording>>, ApplicationError>>,
    // Close cached descriptors before releasing vault ownership and descriptor admission.
    context: Arc<VaultContext>,
    _slot: OwnedSemaphorePermit,
}

struct Recording {
    file: Mutex<File>,
    metadata: AudioFileMetadata,
    modified: Option<SystemTime>,
}

#[derive(Clone)]
pub struct RecordingReader {
    recording: Arc<Recording>,
    entry: Arc<Entry>,
}

impl Default for RecordingLeases {
    fn default() -> Self {
        Self::new(64, 8, IDLE_TIMEOUT)
    }
}

impl RecordingLeases {
    fn new(capacity: usize, owner_limit: usize, idle_timeout: Duration) -> Self {
        Self(Arc::new(Registry {
            state: Mutex::new(State::default()),
            slots: Arc::new(Semaphore::new(capacity)),
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
        context.validate(session)?;
        let id = normalize_id(id)?;
        let entry = {
            let mut state = self.0.state.lock().unwrap();
            self.prune(&mut state);
            if let Some(entry) = state.active.get(&id) {
                entry.check_scope(owner, &context.events.vault_id())?;
                if entry.session != session || !Arc::ptr_eq(&entry.context, &context) {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Recording lease identifies another request",
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
                    entry
                        .context
                        .read_recording(&entry.session, |source| {
                            let (source, metadata, after) =
                                crate::media::recording_with_metadata(source)?;
                            Ok(Arc::new(Recording {
                                file: Mutex::new(source.file),
                                metadata,
                                modified: after.modified().ok(),
                            }))
                        })
                        .await
                })
                .await
                .clone();
            entry.touch()?;
            match result {
                Ok(Some(recording)) => Ok(Some(entry.info(&recording, leases.0.idle_timeout))),
                other => {
                    leases.remove(&entry);
                    other.map(|_| None)
                }
            }
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Recording lease request stopped"))?
    }

    pub fn reader(
        &self,
        owner: &str,
        vault: &str,
        id: &str,
    ) -> Result<RecordingReader, ApplicationError> {
        let id = normalize_id(id)?;
        let mut state = self.0.state.lock().unwrap();
        self.prune(&mut state);
        let entry = state.active.get(&id).ok_or_else(not_found)?;
        entry.check_scope(owner, vault)?;
        entry.touch()?;
        let recording = match entry.recording.get() {
            Some(Ok(Some(recording))) => recording.clone(),
            _ => {
                return Err(error(
                    ErrorCode::Unavailable,
                    "Recording lease is not ready",
                ));
            }
        };
        Ok(RecordingReader {
            entry: entry.clone(),
            recording,
        })
    }

    pub fn close(&self, owner: &str, vault: &str, id: &str) -> Result<(), ApplicationError> {
        let id = normalize_id(id)?;
        let mut state = self.0.state.lock().unwrap();
        self.prune(&mut state);
        if let Some(entry) = state.active.get(&id) {
            entry.check_scope(owner, vault)?;
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

    fn info(&self, recording: &Recording, idle_timeout: Duration) -> RecordingLease {
        RecordingLease {
            id: self.id.clone(),
            vault_id: self.vault.clone(),
            session_id: self.session.clone(),
            metadata: recording.metadata.clone(),
            idle_timeout_secs: idle_timeout.as_secs() as u32,
        }
    }
}

impl RecordingReader {
    pub fn metadata(&self) -> &AudioFileMetadata {
        &self.recording.metadata
    }

    pub async fn read_chunk(
        &self,
        offset: u64,
        length: usize,
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
            reader.entry.touch()?;
            let mut file = reader.recording.file.lock().unwrap();
            reader.verify(&file)?;
            file.seek(SeekFrom::Start(offset)).map_err(read_error)?;
            let length =
                (reader.recording.metadata.size_bytes - offset).min(length as u64) as usize;
            let mut bytes = vec![0; length];
            file.read_exact(&mut bytes).map_err(read_error)?;
            reader.verify(&file)?;
            reader.entry.touch()?;
            Ok(bytes)
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Recording reader stopped"))?
    }

    fn verify(&self, file: &File) -> Result<(), ApplicationError> {
        let metadata = file.metadata().map_err(read_error)?;
        if metadata.len() != self.recording.metadata.size_bytes
            || metadata.modified().ok() != self.recording.modified
        {
            self.entry.closed.store(true, Ordering::Release);
            return Err(changed_error());
        }
        Ok(())
    }
}

fn normalize_id(id: &str) -> Result<String, ApplicationError> {
    uuid::Uuid::parse_str(id)
        .map(|id| id.to_string())
        .map_err(|_| error(ErrorCode::Validation, "Invalid recording lease identifier"))
}
fn not_found() -> ApplicationError {
    error(ErrorCode::NotFound, "Recording lease not found or expired")
}
fn changed_error() -> ApplicationError {
    error(
        ErrorCode::Unavailable,
        "Recording changed; reopen it before continuing",
    )
}
fn capacity_error() -> ApplicationError {
    let mut cause = error(
        ErrorCode::ResourceLimit,
        "Recording lease capacity reached; close idle recordings and retry",
    );
    cause.retryable = true;
    cause
}
fn read_error(cause: std::io::Error) -> ApplicationError {
    tracing::error!(%cause, "recording read failed");
    error(ErrorCode::Internal, "Cannot read recording")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_vault_write::SessionStore;

    async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>) {
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
        let leases = RecordingLeases::default();
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
        let leases = RecordingLeases::default();
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
        let leases = RecordingLeases::default();
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
        let leases = RecordingLeases::new(1, 1, IDLE_TIMEOUT);
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_chunk_retains_ownership_and_admission_until_blocking_io_finishes() {
        let (_root, context) = fixture().await;
        let leases = RecordingLeases::new(1, 1, IDLE_TIMEOUT);
        let id = id();
        leases
            .open(context.clone(), "alice", &id, "note")
            .await
            .unwrap();
        let reader = leases.reader("alice", "vault", &id).unwrap();
        let recording = reader.recording.clone();
        let block = recording.file.lock().unwrap();
        let caller = reader.clone();
        let request = tokio::spawn(async move { caller.read_chunk(0, 4).await });
        tokio::time::timeout(Duration::from_secs(2), async {
            while Arc::strong_count(&reader.entry) < 4 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        let entry = Arc::downgrade(&reader.entry);
        leases.close("alice", "vault", &id).unwrap();
        drop(reader);
        assert!(entry.upgrade().is_some());
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
        let leases = RecordingLeases::new(1, 1, IDLE_TIMEOUT);
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
        let leases = RecordingLeases::new(2, 1, IDLE_TIMEOUT);
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
