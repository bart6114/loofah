//! Device-owned recording recovery. This stores audio bytes and upload receipts;
//! it never opens a vault or dispatches uploads. Raw retirement requires an
//! explicit immutable request and an owned host verification/commit guard.

use std::{
    collections::BTreeMap,
    fs::File,
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock, Weak},
};

use hypr_loofah_protocol::{ApplicationError, ErrorCode, MutationOutcome};
use sha2::{Digest, Sha256};

use crate::{Result, failure};

mod store;
mod types;
pub use types::*;

const MAX_JS_SAFE_MS: u64 = 9_007_199_254_740_991;

pub struct CaptureSpool {
    root: PathBuf,
    limits: CaptureSpoolLimits,
    state: Mutex<store::State>,
    _lease: File,
    io: Arc<tokio::sync::Semaphore>,
    uses: Mutex<BTreeMap<String, Weak<tokio::sync::RwLock<()>>>>,
    #[cfg(test)]
    fault: std::sync::atomic::AtomicU8,
    #[cfg(test)]
    pause: Mutex<Option<TestPause>>,
}

#[derive(Clone)]
pub struct CaptureWriter {
    spool: Arc<CaptureSpool>,
    identity: CaptureIdentity,
    usage: Arc<tokio::sync::OwnedRwLockReadGuard<()>>,
}

/// Exclusive source admission, not proof or permission to delete any audio.
/// Retain this owner through the actual commit, including detached filesystem work.
pub struct CaptureRetirementAdmission {
    snapshot: CaptureSnapshot,
    verified: store::VerifiedSource,
    _spool: Arc<CaptureSpool>,
    _usage: Arc<tokio::sync::OwnedRwLockWriteGuard<()>>,
}

/// Trusted host authority. Implementations own canonical verification and all
/// original scope/resource guards, and reject revocation before each unlink.
/// Storage admission alone must never be used as this authorization.
pub trait CaptureRetirementAuthority: Send + 'static {
    fn check_current(&self) -> Result<()>;
    /// Only Core may supply an original declaration from its durable coordinator
    /// ledger when a server raw manifest predates processing admission.
    fn permits_external_processing(
        &self,
        _: &hypr_loofah_protocol::CaptureRetirementRequest,
    ) -> bool {
        false
    }
}

impl CaptureRetirementAdmission {
    pub fn snapshot(&self) -> &CaptureSnapshot {
        &self.snapshot
    }

    pub async fn retire_verified(
        self,
        request: hypr_loofah_protocol::CaptureRetirementRequest,
        authority: impl CaptureRetirementAuthority,
    ) -> Result<CaptureSnapshot> {
        hypr_loofah_protocol::validate_capture_retirement_request(&request).map_err(invalid)?;
        if request.identity != self.snapshot.identity {
            return Err(conflict("Retirement admission belongs to another capture"));
        }
        let spool = self._spool.clone();
        spool
            .run(move |spool, state| {
                let _admission = self;
                store::retire(spool, state, request, &_admission.verified, authority)
            })
            .await
    }
}

impl CaptureSpool {
    /// Join already admitted blocking I/O without closing capture admission.
    pub async fn wait_for_io(&self) {
        let _drained = self.io.clone().acquire_many_owned(2).await;
    }

    pub async fn open(root: PathBuf, limits: CaptureSpoolLimits) -> Result<Arc<Self>> {
        validate_root(&root)?;
        validate_limits(&limits)?;
        static OPEN_IO: OnceLock<Arc<tokio::sync::Semaphore>> = OnceLock::new();
        let permit = OPEN_IO
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(2)))
            .clone()
            .acquire_owned()
            .await
            .map_err(|_| busy())?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            store::prepare_directory(&root)?;
            let lease = store::owner_lease(&root)?;
            let state = store::load(&root, &limits)?;
            Ok(Arc::new(Self {
                root,
                limits,
                state: Mutex::new(state),
                _lease: lease,
                io: Arc::new(tokio::sync::Semaphore::new(2)),
                uses: Mutex::new(BTreeMap::new()),
                #[cfg(test)]
                fault: std::sync::atomic::AtomicU8::new(0),
                #[cfg(test)]
                pause: Mutex::new(None),
            }))
        })
        .await
        .map_err(|_| damaged())?
    }

    pub async fn begin(
        self: &Arc<Self>,
        identity: CaptureIdentity,
        format: CaptureFormat,
    ) -> Result<CaptureWriter> {
        self.begin_inner(identity, format, None, None).await
    }

    pub async fn begin_with_observation(
        self: &Arc<Self>,
        identity: CaptureIdentity,
        format: CaptureFormat,
        first_admitted_audio_observed_at_ms: u64,
    ) -> Result<CaptureWriter> {
        validate_observation(first_admitted_audio_observed_at_ms)?;
        self.begin_inner(
            identity,
            format,
            Some(first_admitted_audio_observed_at_ms),
            None,
        )
        .await
    }

    pub async fn begin_with_processing(
        self: &Arc<Self>,
        identity: CaptureIdentity,
        format: CaptureFormat,
        processing: Option<CaptureProcessingOptions>,
    ) -> Result<CaptureWriter> {
        validate_identity(&identity)?;
        validate_format(&format)?;
        let declaration = processing.map(|options| CaptureProcessingDeclaration {
            request: options.request(hypr_loofah_protocol::capture_fingerprint(
                &identity, &format,
            )),
            cancel_requested: false,
        });
        if let Some(declaration) = &declaration {
            hypr_loofah_protocol::validate_capture_processing_request(&declaration.request)
                .map_err(invalid)?;
            if matches!(
                identity.target,
                CaptureTarget::ExistingSession {
                    expected_audio_sha256: Some(_),
                    ..
                }
            ) {
                return Err(invalid(
                    "Capture processing does not support appending recordings",
                ));
            }
        }
        self.begin_inner(identity, format, None, declaration).await
    }

    async fn begin_inner(
        self: &Arc<Self>,
        identity: CaptureIdentity,
        format: CaptureFormat,
        observation: Option<u64>,
        processing: Option<CaptureProcessingDeclaration>,
    ) -> Result<CaptureWriter> {
        validate_identity(&identity)?;
        validate_format(&format)?;
        if let CaptureTarget::AppendSession { admission, .. } = &identity.target
            && format != admission.baseline.format
        {
            return Err(invalid("Append format differs from its admitted baseline"));
        }
        let usage = Arc::new(
            self.use_gate(&identity)?
                .try_read_owned()
                .map_err(|_| busy())?,
        );
        let admitted = usage.clone();
        let coordinate = identity.clone();
        self.run(move |spool, state| {
            let _admitted = admitted;
            store::begin(spool, state, coordinate, format, observation, processing)
        })
        .await?;
        Ok(CaptureWriter {
            spool: self.clone(),
            identity,
            usage,
        })
    }

    pub async fn resume(self: &Arc<Self>, identity: CaptureIdentity) -> Result<CaptureWriter> {
        validate_identity(&identity)?;
        let usage = Arc::new(
            self.use_gate(&identity)?
                .try_read_owned()
                .map_err(|_| busy())?,
        );
        let admitted = usage.clone();
        let coordinate = identity.clone();
        self.run(move |spool, state| {
            let _admitted = admitted;
            store::resume(spool, state, &coordinate)
        })
        .await?;
        Ok(CaptureWriter {
            spool: self.clone(),
            identity,
            usage,
        })
    }

    fn use_gate(&self, identity: &CaptureIdentity) -> Result<Arc<tokio::sync::RwLock<()>>> {
        let key = hash(
            serde_json::to_vec(&(&identity.scope, &identity.capture_id)).map_err(|_| damaged())?,
        );
        let mut uses = self.uses.lock().map_err(|_| damaged())?;
        uses.retain(|_, gate| gate.strong_count() != 0);
        if let Some(gate) = uses.get(&key).and_then(Weak::upgrade) {
            return Ok(gate);
        }
        let gate = Arc::new(tokio::sync::RwLock::new(()));
        uses.insert(key, Arc::downgrade(&gate));
        Ok(gate)
    }

    pub async fn admit_retirement(
        self: &Arc<Self>,
        identity: CaptureIdentity,
    ) -> Result<CaptureRetirementAdmission> {
        validate_identity(&identity)?;
        let usage = Arc::new(
            self.use_gate(&identity)?
                .try_write_owned()
                .map_err(|_| busy())?,
        );
        let admitted = usage.clone();
        let (snapshot, verified) = self
            .run(move |spool, state| {
                let _admitted = admitted;
                let snapshot = store::snapshot(spool, state, &identity)?;
                if snapshot.seal.is_none() || snapshot.publication.is_none() {
                    return Err(invalid(
                        "Source retirement requires a sealed published capture",
                    ));
                }
                let verified = store::verify_source(spool, state, &identity)?;
                Ok((snapshot, verified))
            })
            .await?;
        Ok(CaptureRetirementAdmission {
            snapshot,
            verified,
            _spool: self.clone(),
            _usage: usage,
        })
    }

    pub async fn snapshot(self: &Arc<Self>, identity: CaptureIdentity) -> Result<CaptureSnapshot> {
        validate_identity(&identity)?;
        self.run(move |spool, state| store::snapshot(spool, state, &identity))
            .await
    }

    pub async fn inventory(self: &Arc<Self>, scope: CaptureScope) -> Result<Vec<CaptureSnapshot>> {
        validate_scope(&scope)?;
        self.run(move |spool, state| store::inventory(spool, state, Some(&scope)))
            .await
    }

    /// Device recovery remains available when a profile no longer selects the
    /// original account or vault. This is not a remotely authorized listing.
    pub async fn recovery_inventory(self: &Arc<Self>) -> Result<Vec<CaptureSnapshot>> {
        self.run(move |spool, state| store::inventory(spool, state, None))
            .await
    }

    /// A device profile must recover any prior start, even if its selected vault
    /// or member changed. This lookup never exposes another scope's contents.
    pub async fn request_exists(self: &Arc<Self>, request_id: &str) -> Result<bool> {
        validate_uuid(request_id)?;
        let request_id = request_id.to_owned();
        self.run(move |_, state| Ok(store::request_exists(state, &request_id)))
            .await
    }

    async fn run<T: Send + 'static>(
        self: &Arc<Self>,
        action: impl FnOnce(&Self, &mut store::State) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self.io.clone().try_acquire_owned().map_err(|_| busy())?;
        let spool = self.clone();
        tokio::task::spawn_blocking(move || {
            // Neither a dropped caller nor cancellation releases storage ownership
            // while a filesystem operation is still running.
            let _permit = permit;
            let mut state = spool.state.lock().map_err(|_| damaged())?;
            if state.dirty {
                *state = store::load(&spool.root, &spool.limits)?;
            }
            let result = action(&spool, &mut state);
            if result.as_ref().is_err_and(|error| {
                error.mutation_outcome == Some(MutationOutcome::Unknown)
                    || error.code == ErrorCode::Internal
            }) {
                state.dirty = true;
            }
            result
        })
        .await
        .map_err(|_| damaged())?
    }
}

impl CaptureWriter {
    async fn run<T: Send + 'static>(
        &self,
        action: impl FnOnce(&CaptureSpool, &mut store::State) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let usage = self.usage.clone();
        self.spool
            .run(move |spool, state| {
                let _usage = usage;
                action(spool, state)
            })
            .await
    }

    /// Join already admitted blocking spool I/O, including a dropped async waiter.
    pub async fn wait_for_io(&self) {
        self.spool.wait_for_io().await;
    }

    pub fn identity(&self) -> &CaptureIdentity {
        &self.identity
    }

    pub async fn snapshot(&self) -> Result<CaptureSnapshot> {
        let id = self.identity.clone();
        self.run(move |spool, state| store::snapshot(spool, state, &id))
            .await
    }

    pub async fn cancel_processing(&self) -> Result<CaptureSnapshot> {
        let id = self.identity.clone();
        self.run(move |spool, state| store::cancel_processing(spool, state, &id))
            .await
    }

    pub async fn record_first_audio_observation(&self, ms: u64) -> Result<CaptureSnapshot> {
        validate_observation(ms)?;
        let id = self.identity.clone();
        self.run(move |spool, state| store::record_first_audio_observation(spool, state, &id, ms))
            .await
    }

    pub async fn append_chunk(
        &self,
        sequence: u64,
        frames: u32,
        bytes: Vec<u8>,
    ) -> Result<CaptureChunkDescriptor> {
        if frames == 0
            || bytes.len() > self.spool.limits.max_chunk_bytes
            || bytes.len() > u32::MAX as usize
        {
            return Err(invalid("Invalid capture chunk size"));
        }
        let id = self.identity.clone();
        self.run(move |spool, state| store::append(spool, state, &id, sequence, frames, bytes))
            .await
    }

    pub async fn chunks(
        &self,
        after: Option<u64>,
        limit: u32,
    ) -> Result<Vec<CaptureChunkDescriptor>> {
        if !(1..=200).contains(&limit) {
            return Err(invalid("Invalid capture chunk page size"));
        }
        let id = self.identity.clone();
        self.run(move |spool, state| store::chunks(spool, state, &id, after, limit))
            .await
    }

    pub async fn read_chunk(&self, sequence: u64) -> Result<CaptureChunk> {
        let id = self.identity.clone();
        self.run(move |spool, state| store::read_chunk(spool, state, &id, sequence))
            .await
    }

    pub async fn seal(&self) -> Result<CaptureSeal> {
        let id = self.identity.clone();
        self.run(move |spool, state| store::seal(spool, state, &id, None))
            .await
    }

    pub async fn seal_if_matches(&self, expected: CaptureSeal) -> Result<CaptureSeal> {
        let id = self.identity.clone();
        self.run(move |spool, state| store::seal(spool, state, &id, Some(expected)))
            .await
    }

    pub async fn bind_session(&self, session_id: String) -> Result<CaptureSnapshot> {
        validate_session(&session_id)?;
        let id = self.identity.clone();
        self.run(move |spool, state| store::bind_session(spool, state, &id, session_id))
            .await
    }

    pub async fn acknowledge_prefix(
        &self,
        receipt: CapturePrefixReceipt,
    ) -> Result<CaptureSnapshot> {
        let id = self.identity.clone();
        self.run(move |spool, state| store::acknowledge(spool, state, &id, receipt))
            .await
    }

    /// Records a verifier's observation. All chunk files remain on the device.
    pub async fn observe_publication(
        &self,
        publication: CapturePublication,
    ) -> Result<CaptureSnapshot> {
        let id = self.identity.clone();
        self.run(move |spool, state| store::publication(spool, state, &id, publication))
            .await
    }
}

fn validate_observation(ms: u64) -> Result<()> {
    if ms > MAX_JS_SAFE_MS {
        return Err(invalid(
            "Capture observation must be a JavaScript-safe UTC millisecond integer",
        ));
    }
    Ok(())
}

fn validate_root(root: &std::path::Path) -> Result<()> {
    if !root.is_absolute()
        || root.file_name().is_none()
        || root
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(invalid(
            "Capture spool storage must be an absolute device path",
        ));
    }
    Ok(())
}

fn validate_limits(limits: &CaptureSpoolLimits) -> Result<()> {
    if limits.max_bytes == 0
        || !(1..=128).contains(&limits.max_captures)
        || !(1..=131_072).contains(&limits.max_chunks)
        || !(1..=1024 * 1024).contains(&limits.max_chunk_bytes)
        || !(1..=262_144).contains(&limits.max_directory_entries)
    {
        return Err(invalid("Invalid capture spool limits"));
    }
    Ok(())
}

fn validate_scope(scope: &CaptureScope) -> Result<()> {
    if scope.server.len() > 4096
        || [scope.member_id.as_str(), scope.vault_id.as_str()]
            .iter()
            .any(|value| {
                value.is_empty() || value.len() > 512 || value.chars().any(char::is_control)
            })
    {
        return Err(invalid("Invalid capture server, account or vault scope"));
    }
    let url =
        url::Url::parse(&scope.server).map_err(|_| invalid("Invalid capture server origin"))?;
    crate::validate_endpoint(&url, true)?;
    if url.path() != "/" || url.query().is_some() || url.as_str() != scope.server {
        return Err(invalid(
            "Capture scope requires the canonical server origin",
        ));
    }
    Ok(())
}

fn validate_identity(id: &CaptureIdentity) -> Result<()> {
    validate_scope(&id.scope)?;
    validate_uuid(&id.capture_id)?;
    validate_uuid(&id.request_id)?;
    match &id.target {
        CaptureTarget::ExistingSession {
            session_id,
            expected_audio_sha256,
        } => {
            validate_session(session_id)?;
            if expected_audio_sha256
                .as_ref()
                .is_some_and(|value| !valid_hash(value))
            {
                return Err(invalid("Invalid capture audio baseline"));
            }
        }
        CaptureTarget::AppendSession {
            session_id,
            admission,
        } => {
            hypr_loofah_protocol::validate_capture_append_admission(admission).map_err(invalid)?;
            if session_id != &admission.request.session_id
                || id.scope != admission.request.scope
                || id.capture_id != admission.request.capture_id
                || id.request_id != admission.request.request_id
            {
                return Err(invalid("Append admission belongs to another capture"));
            }
        }
        CaptureTarget::PendingSession { create_request_id } => validate_uuid(create_request_id)?,
    }
    Ok(())
}

fn validate_session(session: &str) -> Result<()> {
    hypr_loofah_protocol::validate_session_id(session)
        .map_err(|_| invalid("Invalid capture session"))
}

fn validate_format(format: &CaptureFormat) -> Result<()> {
    hypr_loofah_protocol::validate_capture_format(format).map_err(invalid)
}

fn validate_uuid(value: &str) -> Result<()> {
    if uuid::Uuid::parse_str(value)
        .ok()
        .is_none_or(|parsed| parsed.to_string() != value)
    {
        return Err(invalid("Invalid capture identifier"));
    }
    Ok(())
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn hash(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|c| format!("{c:02x}"))
        .collect()
}
fn invalid(message: &str) -> ApplicationError {
    ApplicationError {
        mutation_outcome: Some(MutationOutcome::NotCommitted),
        ..failure(ErrorCode::Validation, message)
    }
}
fn conflict(message: &str) -> ApplicationError {
    ApplicationError {
        mutation_outcome: Some(MutationOutcome::NotCommitted),
        ..failure(ErrorCode::Conflict, message)
    }
}
fn damaged() -> ApplicationError {
    failure(
        ErrorCode::Internal,
        "Capture recovery files could not be read safely. All audio was retained.",
    )
}
fn limit() -> ApplicationError {
    ApplicationError {
        mutation_outcome: Some(MutationOutcome::NotCommitted),
        ..failure(
            ErrorCode::ResourceLimit,
            "Device capture storage limit reached. Existing audio was retained.",
        )
    }
}
fn busy() -> ApplicationError {
    ApplicationError {
        retryable: true,
        mutation_outcome: Some(MutationOutcome::NotCommitted),
        ..failure(
            ErrorCode::Unavailable,
            "Capture recovery storage is busy or already owned",
        )
    }
}
fn io_error(error: std::io::Error) -> ApplicationError {
    tracing::warn!(kind=?error.kind(),"capture_spool_io_failed");
    ApplicationError {
        mutation_outcome: Some(MutationOutcome::Unknown),
        ..failure(
            if error.kind() == std::io::ErrorKind::StorageFull {
                ErrorCode::ResourceLimit
            } else {
                ErrorCode::Unavailable
            },
            "Capture storage could not be saved safely. Audio already on this device was retained.",
        )
    }
}

#[cfg(test)]
#[derive(Clone, Copy)]
#[repr(u8)]
enum Fault {
    BeforeChunkWrite = 1,
    AfterChunkFileSync = 2,
    AfterChunkRename = 3,
    AfterChunkDirectorySync = 4,
    AfterManifestRename = 5,
    AfterManifestDirectorySync = 6,
    BeforeManifestWrite = 7,
    ShortManifestWrite = 8,
}

#[cfg(test)]
struct TestPause {
    point: u8,
    entered: tokio::sync::oneshot::Sender<()>,
    release: Arc<std::sync::Barrier>,
}

#[cfg(test)]
fn hit(spool: &CaptureSpool, point: Fault) -> Result<()> {
    use std::sync::atomic::Ordering;
    let pause = {
        let mut pause = spool.pause.lock().unwrap();
        if pause
            .as_ref()
            .is_some_and(|pause| pause.point == point as u8)
        {
            pause.take()
        } else {
            None
        }
    };
    if let Some(pause) = pause {
        let _ = pause.entered.send(());
        pause.release.wait();
    }
    if spool
        .fault
        .compare_exchange(point as u8, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        if let Some(marker) = std::env::var_os("LOOFAH_CAPTURE_CRASH_MARKER") {
            if std::env::var_os("LOOFAH_CAPTURE_CRASH_ROOT")
                .is_some_and(|root| std::path::PathBuf::from(root) == spool.root)
            {
                std::fs::write(marker, b"paused").map_err(io_error)?;
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(1));
                }
            }
        }
        return Err(io_error(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "injected capture persistence fault",
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
