use std::{
    collections::BTreeMap,
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use hypr_loofah_protocol::{
    capture_chain_seed as seed, capture_chunk_digest as digest, capture_fingerprint as fingerprint,
};
use serde::{Deserialize, Serialize};

use super::*;

#[path = "retirement.rs"]
mod retirement;
pub(super) use retirement::{VerifiedSource, declare, retire, verify_source};

const MANIFEST_LIMIT: usize = 16 * 1024;
const HEADER_LIMIT: usize = 2048;
const MAGIC: &[u8; 8] = b"LFCAP001";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    snapshot: CaptureSnapshot,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ChunkHeader {
    schema_version: u32,
    descriptor: CaptureChunkDescriptor,
}

struct Entry {
    manifest: Manifest,
    serialized: Vec<u8>,
    chunks: Vec<CaptureChunkDescriptor>,
    source: Sha256,
    retirement: Option<retirement::Journal>,
}

#[derive(Default)]
pub(super) struct State {
    entries: BTreeMap<String, Entry>,
    bytes: u64,
    chunks: usize,
    directory_entries: usize,
    pub(super) dirty: bool,
}

pub(super) fn request_exists(state: &State, request_id: &str) -> bool {
    state
        .entries
        .values()
        .any(|entry| entry.manifest.snapshot.identity.request_id == request_id)
}

pub(super) fn owner_lease(root: &Path) -> Result<File> {
    let path = root.join(".lock");
    check_file(&path, true)?;
    let mut options = File::options();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(path).map_err(io_error)?;
    file.try_lock().map_err(|error| match error {
        std::fs::TryLockError::WouldBlock => busy(),
        std::fs::TryLockError::Error(error) => io_error(error),
    })?;
    sync_directory(root)?;
    Ok(file)
}

pub(super) fn prepare_directory(path: &Path) -> Result<()> {
    let parent = path.parent();
    if let Some(parent) = parent {
        check_ancestors(parent)?;
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
        Ok(_) => return Err(damaged()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = parent
                && !parent.exists()
            {
                prepare_directory(parent)?;
            }
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            builder.create(path).map_err(io_error)?;
            sync_directory(path)?;
            if let Some(parent) = parent {
                sync_directory(parent)?;
            }
        }
        Err(error) => return Err(io_error(error)),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(io_error)?;
    }
    Ok(())
}

fn check_ancestors(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match std::fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => return Err(damaged()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}

fn check_file(path: &Path, missing: bool) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(damaged()),
        Err(error) if missing && error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(io_error)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn sync_file(path: &Path) -> Result<()> {
    check_file(path, false)?;
    File::options()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(io_error)
}

fn read_bytes(path: &Path, limit: usize) -> Result<Vec<u8>> {
    check_file(path, false)?;
    let file = File::open(path).map_err(io_error)?;
    if file.metadata().map_err(io_error)?.len() > limit as u64 {
        return Err(damaged());
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > limit {
        return Err(damaged());
    }
    Ok(bytes)
}

fn scope_key(scope: &CaptureScope) -> String {
    hash(serde_json::to_vec(scope).expect("capture scope serializes"))
}
fn key(id: &CaptureIdentity) -> String {
    format!("{}/{}", scope_key(&id.scope), id.capture_id)
}
fn directory(root: &Path, id: &CaptureIdentity) -> PathBuf {
    root.join(scope_key(&id.scope)).join(&id.capture_id)
}
fn chunk_path(dir: &Path, sequence: u64) -> PathBuf {
    dir.join("chunks").join(format!("{sequence:020}.chunk"))
}
fn pending_path(dir: &Path, sequence: u64) -> PathBuf {
    dir.join("chunks").join(format!("{sequence:020}.pending"))
}

fn source_digest(source: &Sha256) -> String {
    source
        .clone()
        .finalize()
        .iter()
        .map(|c| format!("{c:02x}"))
        .collect()
}

fn room(state: &State, limits: &CaptureSpoolLimits, extra: u64) -> Result<()> {
    if state
        .bytes
        .checked_add(extra)
        .is_none_or(|bytes| bytes > limits.max_bytes)
    {
        return Err(limit());
    }
    Ok(())
}

fn room_entries(state: &State, limits: &CaptureSpoolLimits, extra: usize) -> Result<()> {
    if state
        .directory_entries
        .checked_add(extra)
        .is_none_or(|count| count > limits.max_directory_entries)
    {
        return Err(limit());
    }
    Ok(())
}

fn prepare_counted_directory(
    path: &Path,
    state: &mut State,
    limits: &CaptureSpoolLimits,
) -> Result<()> {
    let exists = path.exists();
    if !exists {
        room_entries(state, limits, 1)?;
    }
    prepare_directory(path)?;
    if !exists {
        state.directory_entries += 1;
    }
    Ok(())
}

fn persist(
    root: &Path,
    limits: &CaptureSpoolLimits,
    state: &mut State,
    entry: &mut Entry,
    spool: Option<&CaptureSpool>,
) -> Result<()> {
    let bytes = serde_json::to_vec(&entry.manifest).map_err(|_| damaged())?;
    if bytes.len() > MANIFEST_LIMIT {
        return Err(limit());
    }
    room(state, limits, bytes.len() as u64)?;
    room_entries(state, limits, 1)?;
    let dir = directory(root, &entry.manifest.snapshot.identity);
    let path = dir.join("manifest.json");
    check_file(&path, true)?;
    let existed = path.exists();
    #[cfg(test)]
    if let Some(spool) = spool {
        hit(spool, Fault::BeforeManifestWrite)?;
        if spool.fault.load(std::sync::atomic::Ordering::SeqCst) == Fault::ShortManifestWrite as u8
        {
            let mut options = File::options();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut partial = options
                .open(dir.join(".capture-observation-incomplete.tmp"))
                .map_err(io_error)?;
            partial
                .write_all(&bytes[..bytes.len() / 2])
                .map_err(io_error)?;
            partial.sync_all().map_err(io_error)?;
            hit(spool, Fault::ShortManifestWrite)?;
        }
    }
    hypr_storage::fs::atomic_write(&path, std::str::from_utf8(&bytes).expect("JSON is UTF-8"))
        .map_err(io_error)?;
    state.bytes = state
        .bytes
        .checked_sub(entry.serialized.len() as u64)
        .and_then(|value| value.checked_add(bytes.len() as u64))
        .ok_or_else(damaged)?;
    entry.serialized = bytes;
    if !existed {
        state.directory_entries += 1;
    }
    #[cfg(test)]
    if let Some(spool) = spool {
        hit(spool, Fault::AfterManifestRename)?;
    }
    #[cfg(not(test))]
    let _ = spool;
    sync_file(&path)?;
    sync_directory(&dir)?;
    #[cfg(test)]
    if let Some(spool) = spool {
        hit(spool, Fault::AfterManifestDirectorySync)?;
    }
    Ok(())
}

fn guard(root: &Path, entry: &Entry) -> Result<()> {
    let dir = directory(root, &entry.manifest.snapshot.identity);
    check_ancestors(&dir.join("chunks"))?;
    if !std::fs::symlink_metadata(dir.join("chunks"))
        .map_err(io_error)?
        .is_dir()
    {
        return Err(damaged());
    }
    if read_bytes(&dir.join("manifest.json"), MANIFEST_LIMIT)? != entry.serialized {
        return Err(damaged());
    }
    Ok(())
}

fn with_entry<T>(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
    action: impl FnOnce(&mut State, &mut Entry) -> Result<T>,
) -> Result<T> {
    let key = key(id);
    let mut entry = state.entries.remove(&key).ok_or_else(|| {
        failure(
            ErrorCode::NotFound,
            "Capture not found in this device scope",
        )
    })?;
    let before = entry.manifest.snapshot.clone();
    let before_version = entry.manifest.schema_version;
    let result = if entry.manifest.snapshot.identity != *id {
        Err(conflict("Capture identity cannot be rebound"))
    } else {
        guard(&spool.root, &entry).and_then(|()| {
            if retirement::fenced(&entry) {
                return Err(conflict(
                    "Capture raw source has been fenced for retirement",
                ));
            }
            action(state, &mut entry)
        })
    };
    if result
        .as_ref()
        .is_err_and(|error| error.mutation_outcome == Some(MutationOutcome::NotCommitted))
    {
        entry.manifest.snapshot = before;
        entry.manifest.schema_version = before_version;
    }
    state.entries.insert(key, entry);
    result
}

pub(super) fn begin(
    spool: &CaptureSpool,
    state: &mut State,
    id: CaptureIdentity,
    format: CaptureFormat,
    observation: Option<u64>,
    processing: Option<CaptureProcessingDeclaration>,
) -> Result<()> {
    if state.entries.contains_key(&key(&id)) {
        return with_entry(spool, state, &id, |_, entry| {
            if entry
                .manifest
                .snapshot
                .processing
                .as_ref()
                .map(|value| &value.request)
                != processing.as_ref().map(|value| &value.request)
            {
                return Err(conflict("Capture processing declaration cannot change"));
            }
            if entry.manifest.snapshot.format != format {
                return Err(conflict("Capture audio format cannot change"));
            }
            if let Some(observation) = observation
                && entry.manifest.snapshot.first_admitted_audio_observed_at_ms != Some(observation)
            {
                return Err(conflict("Capture first audio observation cannot change"));
            }
            confirm(&spool.root, entry)
        });
    }
    if state
        .entries
        .values()
        .filter(|entry| !retirement::completed(entry))
        .count()
        >= spool.limits.max_captures
    {
        return Err(limit());
    }
    if state.entries.values().any(|entry| {
        entry.manifest.snapshot.identity.scope == id.scope
            && entry.manifest.snapshot.identity.request_id == id.request_id
    }) {
        return Err(conflict(
            "This capture request already belongs to another recording",
        ));
    }
    let dir = directory(&spool.root, &id);
    if dir.join("manifest.json").exists() {
        return Err(damaged());
    }
    let capture_fingerprint = fingerprint(&id, &format);
    let session_id = match &id.target {
        CaptureTarget::ExistingSession { session_id, .. }
        | CaptureTarget::AppendSession { session_id, .. } => Some(session_id.clone()),
        CaptureTarget::PendingSession { .. } => None,
    };
    let created_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| damaged())?
        .as_millis()
        .try_into()
        .map_err(|_| damaged())?;
    let mut entry = Entry {
        manifest: Manifest {
            schema_version: if processing.is_some() {
                3
            } else if observation.is_some() {
                2
            } else {
                1
            },
            snapshot: CaptureSnapshot {
                identity: id.clone(),
                format,
                capture_fingerprint: capture_fingerprint.clone(),
                created_at_ms,
                session_id,
                chunk_count: 0,
                frames: 0,
                pcm_bytes: 0,
                chain_digest: seed(&capture_fingerprint),
                acknowledged: None,
                seal: None,
                publication: None,
                retirement: None,
                processing,
                first_admitted_audio_observed_at_ms: observation,
            },
        },
        serialized: vec![],
        chunks: vec![],
        source: Sha256::new(),
        retirement: None,
    };
    let initial_size = serde_json::to_vec(&entry.manifest)
        .map_err(|_| damaged())?
        .len();
    if initial_size > MANIFEST_LIMIT {
        return Err(limit());
    }
    room(state, &spool.limits, initial_size as u64)?;
    let paths = [
        spool.root.join(scope_key(&id.scope)),
        dir.clone(),
        dir.join("chunks"),
    ];
    room_entries(
        state,
        &spool.limits,
        paths.iter().filter(|path| !path.exists()).count() + 1,
    )?;
    for path in &paths {
        prepare_counted_directory(path, state, &spool.limits)?;
    }
    persist(&spool.root, &spool.limits, state, &mut entry, Some(spool))?;
    state.entries.insert(key(&id), entry);
    Ok(())
}

fn confirm(root: &Path, entry: &Entry) -> Result<()> {
    let dir = directory(root, &entry.manifest.snapshot.identity);
    check_file(&dir.join("manifest.json"), false)?;
    sync_file(&dir.join("manifest.json"))?;
    sync_directory(&dir.join("chunks"))?;
    sync_directory(&dir)
}

pub(super) fn resume(spool: &CaptureSpool, state: &mut State, id: &CaptureIdentity) -> Result<()> {
    with_entry(spool, state, id, |_, entry| confirm(&spool.root, entry))
}

pub(super) fn inventory(
    spool: &CaptureSpool,
    state: &mut State,
    scope: Option<&CaptureScope>,
) -> Result<Vec<CaptureSnapshot>> {
    let mut result = vec![];
    for entry in state
        .entries
        .values()
        .filter(|entry| scope.is_none_or(|scope| entry.manifest.snapshot.identity.scope == *scope))
    {
        guard(&spool.root, entry)?;
        result.push(retirement::snapshot(&spool.root, entry)?);
    }
    result.sort_by(|left, right| {
        right
            .created_at_ms
            .cmp(&left.created_at_ms)
            .then_with(|| left.identity.capture_id.cmp(&right.identity.capture_id))
    });
    Ok(result)
}

pub(super) fn snapshot(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
) -> Result<CaptureSnapshot> {
    let entry = state
        .entries
        .get(&key(id))
        .ok_or_else(|| failure(ErrorCode::NotFound, "Capture not found in this scope"))?;
    if entry.manifest.snapshot.identity != *id {
        return Err(conflict("Capture identity cannot be rebound"));
    }
    guard(&spool.root, entry)?;
    retirement::snapshot(&spool.root, entry)
}

pub(super) fn cancel_processing(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
) -> Result<CaptureSnapshot> {
    with_entry(spool, state, id, |state, entry| {
        let declaration = entry
            .manifest
            .snapshot
            .processing
            .as_mut()
            .ok_or_else(|| conflict("This capture has no processing declaration"))?;
        if declaration.cancel_requested {
            confirm(&spool.root, entry)?;
        } else {
            declaration.cancel_requested = true;
            persist(&spool.root, &spool.limits, state, entry, Some(spool))?;
        }
        Ok(entry.manifest.snapshot.clone())
    })
}

pub(super) fn record_first_audio_observation(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
    ms: u64,
) -> Result<CaptureSnapshot> {
    with_entry(spool, state, id, |state, entry| {
        if let Some(existing) = entry.manifest.snapshot.first_admitted_audio_observed_at_ms {
            if existing != ms {
                return Err(conflict(
                    "Capture observation cannot be changed after recording",
                ));
            }
            confirm(&spool.root, entry)?;
        } else {
            entry.manifest.snapshot.first_admitted_audio_observed_at_ms = Some(ms);
            entry.manifest.schema_version = entry.manifest.schema_version.max(2);
            persist(&spool.root, &spool.limits, state, entry, Some(spool))?;
        }
        Ok(entry.manifest.snapshot.clone())
    })
}

pub(super) fn chunks(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
    after: Option<u64>,
    limit: u32,
) -> Result<Vec<CaptureChunkDescriptor>> {
    with_entry(spool, state, id, |_, entry| {
        Ok(entry
            .chunks
            .iter()
            .filter(|chunk| after.is_none_or(|after| chunk.sequence > after))
            .take(limit as usize)
            .cloned()
            .collect())
    })
}

fn validate_pcm(format: &CaptureFormat, frames: u32, bytes: &[u8]) -> Result<()> {
    if frames == 0
        || (frames as usize).checked_mul(format.channels.len() * 4) != Some(bytes.len())
        || bytes.chunks_exact(4).any(|sample| {
            !f32::from_le_bytes(sample.try_into().expect("four-byte sample")).is_finite()
        })
    {
        return Err(invalid(
            "Capture chunks require finite, interleaved, frame-aligned PCM f32 samples",
        ));
    }
    Ok(())
}

pub(super) fn append(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
    sequence: u64,
    frames: u32,
    bytes: Vec<u8>,
) -> Result<CaptureChunkDescriptor> {
    with_entry(spool, state, id, |state, entry| {
        validate_pcm(&entry.manifest.snapshot.format, frames, &bytes)?;
        if let Some(previous) = entry
            .chunks
            .get(usize::try_from(sequence).map_err(|_| invalid("Invalid chunk sequence"))?)
        {
            if previous.frames != frames || previous.sha256 != hash(&bytes) {
                return Err(conflict(
                    "Capture chunk replay changed its bytes or declaration",
                ));
            }
            let chunk = read_file(
                &chunk_path(&directory(&spool.root, id), sequence),
                &spool.limits,
            )?;
            if chunk.descriptor != *previous || chunk.bytes != bytes {
                return Err(damaged());
            }
            confirm(&spool.root, entry)?;
            return Ok(previous.clone());
        }
        let snapshot = &entry.manifest.snapshot;
        if sequence != snapshot.chunk_count {
            return Err(conflict("Capture chunks must be persisted in sequence"));
        }
        if snapshot.seal.is_some() {
            return Err(conflict("A sealed capture cannot accept more audio"));
        }
        if state.chunks >= spool.limits.max_chunks {
            return Err(limit());
        }
        let mut descriptor = CaptureChunkDescriptor {
            capture_fingerprint: snapshot.capture_fingerprint.clone(),
            sequence,
            first_frame: snapshot.frames,
            pcm_offset: snapshot.pcm_bytes,
            frames,
            bytes: bytes.len() as u32,
            sha256: hash(&bytes),
            previous_digest: snapshot.chain_digest.clone(),
            digest: String::new(),
        };
        descriptor.digest = digest(&descriptor);
        let header = serde_json::to_vec(&ChunkHeader {
            schema_version: 1,
            descriptor: descriptor.clone(),
        })
        .map_err(|_| damaged())?;
        if header.len() > HEADER_LIMIT {
            return Err(limit());
        }
        let file_bytes = 12u64 + header.len() as u64 + bytes.len() as u64;
        room(state, &spool.limits, file_bytes + MANIFEST_LIMIT as u64)?;
        room_entries(state, &spool.limits, 2)?;
        #[cfg(test)]
        hit(spool, Fault::BeforeChunkWrite)?;
        let dir = directory(&spool.root, id);
        write_chunk(&dir, &header, &bytes, sequence, spool)?;
        state.bytes = state.bytes.checked_add(file_bytes).ok_or_else(damaged)?;
        state.chunks += 1;
        state.directory_entries += 1;
        entry.source.update(&bytes);
        entry.chunks.push(descriptor.clone());
        let snapshot = &mut entry.manifest.snapshot;
        snapshot.chunk_count += 1;
        snapshot.frames = snapshot
            .frames
            .checked_add(frames as u64)
            .ok_or_else(damaged)?;
        snapshot.pcm_bytes = snapshot
            .pcm_bytes
            .checked_add(bytes.len() as u64)
            .ok_or_else(damaged)?;
        snapshot.chain_digest = descriptor.digest.clone();
        persist(&spool.root, &spool.limits, state, entry, Some(spool))?;
        Ok(descriptor)
    })
}

fn write_chunk(
    dir: &Path,
    header: &[u8],
    bytes: &[u8],
    sequence: u64,
    spool: &CaptureSpool,
) -> Result<()> {
    let target = chunk_path(dir, sequence);
    let pending = pending_path(dir, sequence);
    check_file(&target, true)?;
    check_file(&pending, true)?;
    if target.exists() || pending.exists() {
        return Err(damaged());
    }
    let mut options = File::options();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&pending).map_err(io_error)?;
    file.write_all(MAGIC)
        .and_then(|()| file.write_all(&(header.len() as u32).to_le_bytes()))
        .and_then(|()| file.write_all(header))
        .and_then(|()| file.write_all(bytes))
        .map_err(io_error)?;
    file.sync_all().map_err(io_error)?;
    #[cfg(test)]
    hit(spool, Fault::AfterChunkFileSync)?;
    drop(file);
    hypr_storage::fs::rename_with_retry(&pending, &target).map_err(io_error)?;
    #[cfg(test)]
    hit(spool, Fault::AfterChunkRename)?;
    sync_file(&target)?;
    sync_directory(&dir.join("chunks"))?;
    #[cfg(test)]
    hit(spool, Fault::AfterChunkDirectorySync)?;
    #[cfg(not(test))]
    let _ = spool;
    Ok(())
}

fn read_file(path: &Path, limits: &CaptureSpoolLimits) -> Result<CaptureChunk> {
    let bytes = read_bytes(path, 12 + HEADER_LIMIT + limits.max_chunk_bytes)?;
    if bytes.len() < 12 || &bytes[..8] != MAGIC {
        return Err(damaged());
    }
    let header_len =
        u32::from_le_bytes(bytes[8..12].try_into().expect("four-byte length")) as usize;
    if header_len > HEADER_LIMIT || bytes.len() < 12 + header_len {
        return Err(damaged());
    }
    let header: ChunkHeader =
        serde_json::from_slice(&bytes[12..12 + header_len]).map_err(|_| damaged())?;
    let descriptor = header.descriptor;
    let body = &bytes[12 + header_len..];
    if header.schema_version != 1
        || body.len() != descriptor.bytes as usize
        || body.len() > limits.max_chunk_bytes
        || ![
            &descriptor.capture_fingerprint,
            &descriptor.sha256,
            &descriptor.previous_digest,
            &descriptor.digest,
        ]
        .into_iter()
        .all(|value| valid_hash(value))
        || descriptor.digest != digest(&descriptor)
        || hash(body) != descriptor.sha256
    {
        return Err(damaged());
    }
    Ok(CaptureChunk {
        descriptor,
        bytes: body.to_vec(),
    })
}

pub(super) fn read_chunk(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
    sequence: u64,
) -> Result<CaptureChunk> {
    with_entry(spool, state, id, |_, entry| {
        let expected = entry
            .chunks
            .get(usize::try_from(sequence).map_err(|_| invalid("Invalid capture sequence"))?)
            .ok_or_else(|| failure(ErrorCode::NotFound, "Capture chunk not found"))?;
        let chunk = read_file(
            &chunk_path(&directory(&spool.root, id), sequence),
            &spool.limits,
        )?;
        if chunk.descriptor != *expected {
            return Err(damaged());
        }
        Ok(chunk)
    })
}

pub(super) fn seal(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
    expected: Option<CaptureSeal>,
) -> Result<CaptureSeal> {
    with_entry(spool, state, id, |state, entry| {
        // Immutable chunks are hashed on append and verified on read/reopen.
        // Rereading hours of prior audio here would block the next capture's disk writes.
        let snapshot = &entry.manifest.snapshot;
        let seal = CaptureSeal {
            capture_fingerprint: snapshot.capture_fingerprint.clone(),
            chunk_count: snapshot.chunk_count,
            frames: snapshot.frames,
            pcm_bytes: snapshot.pcm_bytes,
            chain_digest: snapshot.chain_digest.clone(),
            source_sha256: source_digest(&entry.source),
        };
        if expected.as_ref().is_some_and(|expected| expected != &seal) {
            return Err(conflict("Capture seal does not match its admitted audio"));
        }
        if let Some(existing) = &snapshot.seal {
            if *existing != seal {
                return Err(damaged());
            }
            confirm(&spool.root, entry)?;
            return Ok(seal);
        }
        entry.manifest.snapshot.seal = Some(seal.clone());
        persist(&spool.root, &spool.limits, state, entry, Some(spool))?;
        Ok(seal)
    })
}

pub(super) fn bind_session(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
    session_id: String,
) -> Result<CaptureSnapshot> {
    with_entry(spool, state, id, |state, entry| {
        if let Some(existing) = &entry.manifest.snapshot.session_id {
            if *existing != session_id {
                return Err(conflict(
                    "Capture destination cannot be changed after admission",
                ));
            }
            confirm(&spool.root, entry)?;
        } else {
            entry.manifest.snapshot.session_id = Some(session_id);
            persist(&spool.root, &spool.limits, state, entry, Some(spool))?;
        }
        Ok(entry.manifest.snapshot.clone())
    })
}

fn prefix(entry: &Entry, count: u64) -> Result<CapturePrefixReceipt> {
    let snapshot = &entry.manifest.snapshot;
    if count > entry.chunks.len() as u64 {
        return Err(conflict(
            "Capture receipt refers to audio not present on this device",
        ));
    }
    let (frames, pcm_bytes, chain_digest) = if count == 0 {
        (0, 0, seed(&snapshot.capture_fingerprint))
    } else {
        let last = &entry.chunks[count as usize - 1];
        (
            last.first_frame
                .checked_add(last.frames as u64)
                .ok_or_else(damaged)?,
            last.pcm_offset
                .checked_add(last.bytes as u64)
                .ok_or_else(damaged)?,
            last.digest.clone(),
        )
    };
    Ok(CapturePrefixReceipt {
        capture_fingerprint: snapshot.capture_fingerprint.clone(),
        chunk_count: count,
        frames,
        pcm_bytes,
        chain_digest,
    })
}

pub(super) fn acknowledge(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
    receipt: CapturePrefixReceipt,
) -> Result<CaptureSnapshot> {
    with_entry(spool, state, id, |state, entry| {
        if prefix(entry, receipt.chunk_count)? != receipt {
            return Err(conflict(
                "Capture acknowledgment does not match the immutable audio prefix",
            ));
        }
        if entry
            .manifest
            .snapshot
            .acknowledged
            .as_ref()
            .is_none_or(|old| old.chunk_count < receipt.chunk_count)
        {
            entry.manifest.snapshot.acknowledged = Some(receipt);
            persist(&spool.root, &spool.limits, state, entry, Some(spool))?;
        } else {
            confirm(&spool.root, entry)?;
        }
        Ok(entry.manifest.snapshot.clone())
    })
}

fn validate_publication(entry: &Entry, publication: &CapturePublication) -> Result<()> {
    if entry.manifest.snapshot.seal.as_ref() != Some(&publication.source)
        || entry.manifest.snapshot.session_id.as_deref() != Some(&publication.session_id)
        || entry
            .manifest
            .snapshot
            .acknowledged
            .as_ref()
            .is_none_or(|ack| ack.chunk_count != publication.source.chunk_count)
        || !valid_hash(&publication.canonical_audio_sha256)
        || publication.canonical_audio_bytes == 0
    {
        return Err(conflict(
            "Capture publication does not match the sealed, admitted and acknowledged recording",
        ));
    }
    Ok(())
}

pub(super) fn publication(
    spool: &CaptureSpool,
    state: &mut State,
    id: &CaptureIdentity,
    publication: CapturePublication,
) -> Result<CaptureSnapshot> {
    with_entry(spool, state, id, |state, entry| {
        validate_publication(entry, &publication)?;
        if let Some(existing) = &entry.manifest.snapshot.publication {
            if *existing != publication {
                return Err(conflict("Capture publication identity cannot change"));
            }
            confirm(&spool.root, entry)?;
        } else {
            entry.manifest.snapshot.publication = Some(publication);
            persist(&spool.root, &spool.limits, state, entry, Some(spool))?;
        }
        Ok(entry.manifest.snapshot.clone())
    })
}

struct Candidate {
    scope: String,
    capture: String,
    directory: PathBuf,
    files: BTreeMap<u64, PathBuf>,
    pending: BTreeMap<u64, PathBuf>,
}

fn list_directory(
    path: &Path,
    entries: &mut usize,
    limits: &CaptureSpoolLimits,
) -> Result<Vec<std::fs::DirEntry>> {
    check_ancestors(path)?;
    let mut result = vec![];
    for entry in std::fs::read_dir(path).map_err(io_error)? {
        *entries = entries.checked_add(1).ok_or_else(limit)?;
        if *entries > limits.max_directory_entries {
            return Err(limit());
        }
        result.push(entry.map_err(io_error)?);
    }
    Ok(result)
}

fn account_file(
    state: &mut State,
    meta: &std::fs::Metadata,
    limits: &CaptureSpoolLimits,
) -> Result<()> {
    if meta.is_file() {
        room(state, limits, meta.len())?;
        state.bytes += meta.len();
    }
    Ok(())
}

fn chunk_name(name: &str, suffix: &str) -> Option<u64> {
    let number = name.strip_suffix(suffix)?;
    if number.len() != 20 || !number.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let sequence = number.parse::<u64>().ok()?;
    (format!("{sequence:020}") == number).then_some(sequence)
}

fn candidates(
    root: &Path,
    limits: &CaptureSpoolLimits,
    state: &mut State,
) -> Result<Vec<Candidate>> {
    let mut entries = 0usize;
    let mut result = vec![];
    for scope in list_directory(root, &mut entries, limits)? {
        let name = scope.file_name().to_string_lossy().to_string();
        let meta = std::fs::symlink_metadata(scope.path()).map_err(io_error)?;
        if !valid_hash(&name) {
            if name.len() == 64 && name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(damaged());
            }
            account_file(state, &meta, limits)?;
            continue;
        }
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(damaged());
        }
        for capture in list_directory(&scope.path(), &mut entries, limits)? {
            let capture_id = capture.file_name().to_string_lossy().to_string();
            let meta = std::fs::symlink_metadata(capture.path()).map_err(io_error)?;
            if validate_uuid(&capture_id).is_err() {
                if uuid::Uuid::parse_str(&capture_id).is_ok() {
                    return Err(damaged());
                }
                account_file(state, &meta, limits)?;
                continue;
            }
            if !meta.is_dir() || meta.file_type().is_symlink() {
                return Err(damaged());
            }
            let mut candidate = Candidate {
                scope: name.clone(),
                capture: capture_id,
                directory: capture.path(),
                files: BTreeMap::new(),
                pending: BTreeMap::new(),
            };
            let mut manifest = false;
            let mut chunk_directory = false;
            for item in list_directory(&candidate.directory, &mut entries, limits)? {
                let name = item.file_name().to_string_lossy().to_string();
                let meta = std::fs::symlink_metadata(item.path()).map_err(io_error)?;
                if name == "chunks" {
                    if !meta.is_dir() || meta.file_type().is_symlink() {
                        return Err(damaged());
                    }
                    chunk_directory = true;
                    for chunk in list_directory(&item.path(), &mut entries, limits)? {
                        let name = chunk.file_name().to_string_lossy().to_string();
                        let meta = std::fs::symlink_metadata(chunk.path()).map_err(io_error)?;
                        account_file(state, &meta, limits)?;
                        let destination = if let Some(sequence) = chunk_name(&name, ".chunk") {
                            Some((sequence, &mut candidate.files))
                        } else {
                            chunk_name(&name, ".pending")
                                .map(|sequence| (sequence, &mut candidate.pending))
                        };
                        if let Some((sequence, destination)) = destination {
                            if !meta.is_file() || meta.file_type().is_symlink() {
                                return Err(damaged());
                            }
                            state.chunks = state.chunks.checked_add(1).ok_or_else(limit)?;
                            if state.chunks > limits.max_chunks {
                                return Err(limit());
                            }
                            destination.insert(sequence, chunk.path());
                        }
                    }
                } else {
                    account_file(state, &meta, limits)?;
                    if name == "manifest.json" {
                        if !meta.is_file() || meta.file_type().is_symlink() {
                            return Err(damaged());
                        }
                        manifest = true;
                    }
                }
            }
            if !manifest {
                if candidate.files.is_empty() && candidate.pending.is_empty() {
                    continue;
                }
                return Err(damaged());
            }
            if !chunk_directory {
                return Err(damaged());
            }
            if result.len() >= limits.max_captures.saturating_add(retirement::MAX_RECEIPTS) {
                return Err(limit());
            }
            result.push(candidate);
        }
    }
    state.directory_entries = entries;
    Ok(result)
}

pub(super) fn load(root: &Path, limits: &CaptureSpoolLimits) -> Result<State> {
    let mut state = State::default();
    let candidates = candidates(root, limits, &mut state)?;
    for candidate in candidates {
        let bytes = read_bytes(&candidate.directory.join("manifest.json"), MANIFEST_LIMIT)?;
        let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|_| damaged())?;
        let snapshot = &manifest.snapshot;
        validate_identity(&snapshot.identity).map_err(|_| damaged())?;
        validate_format(&snapshot.format).map_err(|_| damaged())?;
        if !matches!(
            (
                manifest.schema_version,
                snapshot.first_admitted_audio_observed_at_ms,
                snapshot.processing.is_some()
            ),
            (1, None, false)
                | (2, Some(0..=MAX_JS_SAFE_MS), false)
                | (3, None | Some(0..=MAX_JS_SAFE_MS), true)
        ) || snapshot.created_at_ms > MAX_JS_SAFE_MS
            || scope_key(&snapshot.identity.scope) != candidate.scope
            || snapshot.identity.capture_id != candidate.capture
            || snapshot.capture_fingerprint != fingerprint(&snapshot.identity, &snapshot.format)
            || snapshot.retirement.is_some()
            || snapshot.chunk_count > limits.max_chunks as u64
        {
            return Err(damaged());
        }
        if let Some(declaration) = &snapshot.processing {
            hypr_loofah_protocol::validate_capture_processing_request(&declaration.request)
                .map_err(|_| damaged())?;
            if declaration.request.capture_fingerprint != snapshot.capture_fingerprint
                || matches!(
                    snapshot.identity.target,
                    CaptureTarget::ExistingSession {
                        expected_audio_sha256: Some(_),
                        ..
                    }
                )
            {
                return Err(damaged());
            }
        }
        match &snapshot.identity.target {
            CaptureTarget::ExistingSession { session_id, .. }
            | CaptureTarget::AppendSession { session_id, .. }
                if snapshot.session_id.as_ref() != Some(session_id) =>
            {
                return Err(damaged());
            }
            _ => {}
        }
        if let Some(session) = &snapshot.session_id {
            validate_session(session).map_err(|_| damaged())?;
        }
        let mut entry = Entry {
            manifest,
            serialized: bytes,
            chunks: vec![],
            source: Sha256::new(),
            retirement: None,
        };
        if retirement::load(root, limits, &candidate, &mut entry)? {
            let coordinate = key(&entry.manifest.snapshot.identity);
            if state.entries.values().any(|other| {
                other.manifest.snapshot.identity.scope == entry.manifest.snapshot.identity.scope
                    && other.manifest.snapshot.identity.request_id
                        == entry.manifest.snapshot.identity.request_id
            }) {
                return Err(damaged());
            }
            state.entries.insert(coordinate, entry);
            continue;
        }
        if entry.manifest.snapshot.chunk_count > candidate.files.len() as u64 {
            return Err(damaged());
        }
        for (sequence, path) in &candidate.files {
            let chunk = read_file(path, limits)?;
            accept_loaded(&mut entry, *sequence, chunk)?;
            if *sequence >= entry.manifest.snapshot.chunk_count {
                sync_file(path)?;
            }
        }
        let before = prefix(&entry, entry.manifest.snapshot.chunk_count).map_err(|_| damaged())?;
        if before.frames != entry.manifest.snapshot.frames
            || before.pcm_bytes != entry.manifest.snapshot.pcm_bytes
            || before.chain_digest != entry.manifest.snapshot.chain_digest
        {
            return Err(damaged());
        }
        if let Some(receipt) = &entry.manifest.snapshot.acknowledged
            && (receipt.chunk_count > entry.manifest.snapshot.chunk_count
                || prefix(&entry, receipt.chunk_count).map_err(|_| damaged())? != *receipt)
        {
            return Err(damaged());
        }
        let mut changed = entry.chunks.len() as u64 != entry.manifest.snapshot.chunk_count;
        if !candidate.pending.is_empty() {
            if candidate.pending.len() != 1 || entry.manifest.snapshot.seal.is_some() {
                return Err(damaged());
            }
            let (sequence, path) = candidate
                .pending
                .first_key_value()
                .expect("one pending chunk");
            let chunk = read_file(path, limits)?;
            accept_loaded(&mut entry, *sequence, chunk)?;
            File::options()
                .read(true)
                .write(true)
                .open(path)
                .and_then(|file| file.sync_all())
                .map_err(io_error)?;
            let target = chunk_path(&candidate.directory, *sequence);
            check_file(&target, true)?;
            if target.exists() {
                return Err(damaged());
            }
            hypr_storage::fs::rename_with_retry(path, &target).map_err(io_error)?;
            sync_file(&target)?;
            sync_directory(&candidate.directory.join("chunks"))?;
            changed = true;
        }
        if entry.manifest.snapshot.seal.is_some() && changed {
            return Err(damaged());
        }
        if changed {
            let current = prefix(&entry, entry.chunks.len() as u64).map_err(|_| damaged())?;
            entry.manifest.snapshot.chunk_count = current.chunk_count;
            entry.manifest.snapshot.frames = current.frames;
            entry.manifest.snapshot.pcm_bytes = current.pcm_bytes;
            entry.manifest.snapshot.chain_digest = current.chain_digest;
            persist(root, limits, &mut state, &mut entry, None)?;
        }
        if let Some(seal) = &entry.manifest.snapshot.seal {
            let current = &entry.manifest.snapshot;
            if seal.capture_fingerprint != current.capture_fingerprint
                || seal.chunk_count != current.chunk_count
                || seal.frames != current.frames
                || seal.pcm_bytes != current.pcm_bytes
                || seal.chain_digest != current.chain_digest
                || seal.source_sha256 != source_digest(&entry.source)
            {
                return Err(damaged());
            }
        }
        if let Some(publication) = &entry.manifest.snapshot.publication {
            validate_publication(&entry, publication).map_err(|_| damaged())?;
        }
        confirm(root, &entry)?;
        let coordinate = key(&entry.manifest.snapshot.identity);
        if state.entries.values().any(|other| {
            other.manifest.snapshot.identity.scope == entry.manifest.snapshot.identity.scope
                && other.manifest.snapshot.identity.request_id
                    == entry.manifest.snapshot.identity.request_id
        }) {
            return Err(damaged());
        }
        state.entries.insert(coordinate, entry);
    }
    retirement::validate_receipts(&state)?;
    let completed = state
        .entries
        .values()
        .filter(|entry| retirement::completed(entry))
        .count();
    if completed > retirement::MAX_RECEIPTS || state.entries.len() - completed > limits.max_captures
    {
        return Err(limit());
    }
    Ok(state)
}

fn accept_loaded(entry: &mut Entry, sequence: u64, chunk: CaptureChunk) -> Result<()> {
    let expected = prefix(entry, entry.chunks.len() as u64).map_err(|_| damaged())?;
    let descriptor = &chunk.descriptor;
    if sequence != entry.chunks.len() as u64
        || descriptor.sequence != sequence
        || descriptor.capture_fingerprint != entry.manifest.snapshot.capture_fingerprint
        || descriptor.first_frame != expected.frames
        || descriptor.pcm_offset != expected.pcm_bytes
        || descriptor.previous_digest != expected.chain_digest
    {
        return Err(damaged());
    }
    validate_pcm(
        &entry.manifest.snapshot.format,
        descriptor.frames,
        &chunk.bytes,
    )
    .map_err(|_| damaged())?;
    entry.source.update(&chunk.bytes);
    entry.chunks.push(chunk.descriptor);
    Ok(())
}
