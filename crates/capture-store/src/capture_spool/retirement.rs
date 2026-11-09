use super::*;
use hypr_loofah_protocol::{
    CaptureRetirementRequest, CaptureRetirementSnapshot, CaptureRetirementState,
};

pub(super) const MAX_RECEIPTS: usize = 4096;
const JOURNAL_LIMIT: usize = 32 * 1024 * 1024;
const JOURNAL: &str = "retirement.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Journal {
    schema_version: u32,
    request: CaptureRetirementRequest,
    state: CaptureRetirementState,
    // Persist once before removing any source. A crash may leave any subset;
    // retries use these original descriptors rather than inferring a new source.
    chunks: Vec<CaptureChunkDescriptor>,
    #[serde(skip)]
    serialized: Vec<u8>,
}

#[derive(Clone, PartialEq, Eq)]
struct FileStamp {
    volume: u64,
    index: u64,
    bytes: u64,
    modified: SystemTime,
    #[cfg(unix)]
    changed: (i64, i64),
}
fn stamp(file: &File) -> Result<FileStamp> {
    let metadata = file.metadata().map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() != 1 {
            return Err(damaged());
        }
        Ok(FileStamp {
            volume: metadata.dev(),
            index: metadata.ino(),
            bytes: metadata.len(),
            modified: metadata.modified().map_err(io_error)?,
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
    #[cfg(windows)]
    {
        let info = winapi_util::file::information(file).map_err(io_error)?;
        if info.number_of_links() != 1 {
            return Err(damaged());
        }
        Ok(FileStamp {
            volume: info.volume_serial_number(),
            index: info.file_index(),
            bytes: metadata.len(),
            modified: metadata.modified().map_err(io_error)?,
        })
    }
    #[cfg(not(any(unix, windows)))]
    Err(conflict(
        "Source retirement is unsupported on this platform",
    ))
}

#[derive(Clone)]
pub(in crate::capture_spool) struct VerifiedSource {
    manifest: Vec<u8>,
    journal: Option<Vec<u8>>,
    files: BTreeMap<u64, FileStamp>,
}

pub(in crate::capture_spool) fn verify_source(
    spool: &CaptureSpool,
    state: &State,
    identity: &CaptureIdentity,
) -> Result<VerifiedSource> {
    let entry = state
        .entries
        .get(&key(identity))
        .ok_or_else(|| failure(ErrorCode::NotFound, "Capture not found"))?;
    guard(&spool.root, entry)?;
    let dir = directory(&spool.root, identity);
    let expected = entry
        .retirement
        .as_ref()
        .map_or(&entry.chunks, |journal| &journal.chunks);
    let files = exact_files(&dir, expected, entry.retirement.is_some())?;
    if !fenced(entry) && files.len() != expected.len() {
        return Err(damaged());
    }
    let mut source = Sha256::new();
    let mut verified = BTreeMap::new();
    for (sequence, path) in files {
        let before = stamp(&private_file(&path)?)?;
        let chunk = read_file(&path, &spool.limits)?;
        if chunk.descriptor != expected[sequence as usize]
            || stamp(&private_file(&path)?)? != before
        {
            return Err(damaged());
        }
        source.update(&chunk.bytes);
        verified.insert(sequence, before);
    }
    if !fenced(entry)
        && entry
            .manifest
            .snapshot
            .seal
            .as_ref()
            .is_none_or(|seal| seal.source_sha256 != source_digest(&source))
    {
        return Err(damaged());
    }
    Ok(VerifiedSource {
        manifest: entry.serialized.clone(),
        journal: entry
            .retirement
            .as_ref()
            .map(|journal| journal.serialized.clone()),
        files: verified,
    })
}

pub(super) fn fenced(entry: &Entry) -> bool {
    entry
        .retirement
        .as_ref()
        .is_some_and(|journal| journal.state != CaptureRetirementState::Declared)
}

pub(super) fn completed(entry: &Entry) -> bool {
    entry
        .retirement
        .as_ref()
        .is_some_and(|value| value.state == CaptureRetirementState::Retired)
}

fn validate_request(entry: &Entry, request: &CaptureRetirementRequest) -> Result<()> {
    hypr_loofah_protocol::validate_capture_retirement_request(request).map_err(invalid)?;
    let snapshot = &entry.manifest.snapshot;
    if snapshot.identity != request.identity
        || snapshot.format != request.format
        || snapshot.seal.as_ref() != Some(&request.source)
        || snapshot.publication.as_ref() != Some(&request.publication)
        || snapshot
            .processing
            .as_ref()
            .is_some_and(|value| Some(&value.request) != request.processing.as_ref())
    {
        return Err(conflict(
            "Retirement requires the original immutable capture declaration and publication",
        ));
    }
    validate_publication(entry, &request.publication)?;
    Ok(())
}

fn validate_journal(entry: &Entry, journal: &Journal, limits: &CaptureSpoolLimits) -> Result<()> {
    validate_request(entry, &journal.request).map_err(|_| damaged())?;
    if journal.schema_version != 1 {
        return Err(damaged());
    }
    let seal = &journal.request.source;
    let snapshot = &entry.manifest.snapshot;
    if seal.chunk_count != snapshot.chunk_count
        || seal.frames != snapshot.frames
        || seal.pcm_bytes != snapshot.pcm_bytes
        || seal.chain_digest != snapshot.chain_digest
        || !valid_hash(&seal.source_sha256)
    {
        return Err(damaged());
    }
    if journal.state == CaptureRetirementState::Retired {
        return if journal.chunks.is_empty() {
            Ok(())
        } else {
            Err(damaged())
        };
    }
    if journal.chunks.len() as u64 != seal.chunk_count || journal.chunks.len() > limits.max_chunks {
        return Err(damaged());
    }
    let mut frames = 0u64;
    let mut bytes = 0u64;
    let mut chain = seed(&snapshot.capture_fingerprint);
    for (sequence, descriptor) in journal.chunks.iter().enumerate() {
        if descriptor.sequence != sequence as u64
            || descriptor.capture_fingerprint != snapshot.capture_fingerprint
            || descriptor.first_frame != frames
            || descriptor.pcm_offset != bytes
            || descriptor.previous_digest != chain
            || descriptor.frames == 0
            || descriptor.bytes as usize > limits.max_chunk_bytes
            || u64::from(descriptor.bytes)
                != u64::from(descriptor.frames) * snapshot.format.channels.len() as u64 * 4
            || !valid_hash(&descriptor.sha256)
            || descriptor.digest != digest(descriptor)
        {
            return Err(damaged());
        }
        frames = frames
            .checked_add(descriptor.frames.into())
            .ok_or_else(damaged)?;
        bytes = bytes
            .checked_add(descriptor.bytes.into())
            .ok_or_else(damaged)?;
        chain = descriptor.digest.clone();
    }
    if frames != seal.frames || bytes != seal.pcm_bytes || chain != seal.chain_digest {
        return Err(damaged());
    }
    Ok(())
}

// This is deliberately stricter than recovery discovery: a private source may
// only be retired when its entire namespace is understood. Unknown files stay.
fn exact_files(
    dir: &Path,
    expected: &[CaptureChunkDescriptor],
    journal_exists: bool,
) -> Result<Vec<(u64, PathBuf)>> {
    check_ancestors(&dir.join("chunks"))?;
    let mut found_manifest = false;
    let mut found_chunks = false;
    let mut found_journal = false;
    for item in std::fs::read_dir(dir).map_err(io_error)? {
        let item = item.map_err(io_error)?;
        match item.file_name().to_str() {
            Some("manifest.json") => {
                private_file(&item.path())?;
                found_manifest = true;
            }
            Some("chunks") => {
                check_ancestors(&item.path())?;
                found_chunks = true;
            }
            Some(JOURNAL) if journal_exists => {
                private_file(&item.path())?;
                found_journal = true;
            }
            _ => return Err(conflict("Unknown capture recovery files must be retained")),
        }
    }
    if !found_manifest || !found_chunks || found_journal != journal_exists {
        return Err(damaged());
    }
    let mut files = Vec::new();
    for item in std::fs::read_dir(dir.join("chunks")).map_err(io_error)? {
        let item = item.map_err(io_error)?;
        let name = item.file_name();
        let sequence = name
            .to_str()
            .and_then(|name| chunk_name(name, ".chunk"))
            .ok_or_else(|| conflict("Unknown capture source files must be retained"))?;
        if expected
            .get(usize::try_from(sequence).map_err(|_| damaged())?)
            .is_none_or(|chunk| chunk.sequence != sequence)
        {
            return Err(damaged());
        }
        private_file(&item.path())?;
        files.push((sequence, item.path()));
        if files.len() > expected.len() {
            return Err(damaged());
        }
    }
    files.sort_by_key(|(sequence, _)| *sequence);
    Ok(files)
}

fn private_file(path: &Path) -> Result<File> {
    check_file(path, false)?;
    let file = File::open(path).map_err(io_error)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let opened = file.metadata().map_err(io_error)?;
        let current = std::fs::symlink_metadata(path).map_err(io_error)?;
        if opened.nlink() != 1
            || current.nlink() != 1
            || opened.dev() != current.dev()
            || opened.ino() != current.ino()
            || !current.is_file()
            || current.file_type().is_symlink()
        {
            return Err(damaged());
        }
    }
    #[cfg(windows)]
    {
        let opened = winapi_util::file::information(&file).map_err(io_error)?;
        check_file(path, false)?;
        let current = File::open(path).map_err(io_error)?;
        let current = winapi_util::file::information(&current).map_err(io_error)?;
        if opened.number_of_links() != 1
            || current.number_of_links() != 1
            || opened.volume_serial_number() != current.volume_serial_number()
            || opened.file_index() != current.file_index()
        {
            return Err(damaged());
        }
    }
    #[cfg(not(any(unix, windows)))]
    return Err(conflict(
        "Source retirement is unsupported on this platform",
    ));
    Ok(file)
}

fn journal_guard(dir: &Path, journal: &Journal) -> Result<()> {
    private_file(&dir.join(JOURNAL))?;
    if read_bytes(&dir.join(JOURNAL), JOURNAL_LIMIT)? != journal.serialized {
        return Err(damaged());
    }
    Ok(())
}

pub(super) fn snapshot(root: &Path, entry: &Entry) -> Result<CaptureSnapshot> {
    let mut snapshot = entry.manifest.snapshot.clone();
    if let Some(journal) = &entry.retirement {
        let dir = directory(root, &snapshot.identity);
        journal_guard(&dir, journal)?;
        let remaining = exact_files(&dir, &journal.chunks, true)?.len() as u64;
        snapshot.retirement = Some(CaptureRetirementSnapshot {
            request: journal.request.clone(),
            state: journal.state,
            remaining_chunks: remaining,
        });
    }
    Ok(snapshot)
}

pub(super) fn load(
    root: &Path,
    limits: &CaptureSpoolLimits,
    candidate: &Candidate,
    entry: &mut Entry,
) -> Result<bool> {
    let path = candidate.directory.join(JOURNAL);
    match std::fs::symlink_metadata(&path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(io_error(error)),
        Ok(_) => {}
    }
    private_file(&path)?;
    let serialized = read_bytes(&path, JOURNAL_LIMIT)?;
    let mut journal: Journal = serde_json::from_slice(&serialized).map_err(|_| damaged())?;
    validate_journal(entry, &journal, limits)?;
    let files = exact_files(&candidate.directory, &journal.chunks, true)?;
    if journal.state == CaptureRetirementState::Declared && files.len() != journal.chunks.len() {
        return Err(damaged());
    }
    for (sequence, path) in files {
        let chunk = read_file(&path, limits)?;
        if chunk.descriptor != journal.chunks[sequence as usize] {
            return Err(damaged());
        }
        if journal.state == CaptureRetirementState::Declared {
            entry.source.update(&chunk.bytes);
        }
    }
    if journal.state == CaptureRetirementState::Declared {
        if source_digest(&entry.source) != journal.request.source.source_sha256 {
            return Err(damaged());
        }
        entry.chunks = journal.chunks.clone();
    }
    journal.serialized = serialized;
    entry.retirement = Some(journal);
    guard(root, entry)?;
    // Opening never deletes files or resumes an incomplete retirement.
    Ok(true)
}

fn persist_journal(spool: &CaptureSpool, state: &mut State, entry: &mut Entry) -> Result<()> {
    let journal = entry.retirement.as_mut().ok_or_else(damaged)?;
    let bytes = serde_json::to_vec(journal).map_err(|_| damaged())?;
    if bytes.len() > JOURNAL_LIMIT {
        return Err(limit());
    }
    room(state, &spool.limits, bytes.len() as u64)?;
    room_entries(state, &spool.limits, 1)?;
    let dir = directory(&spool.root, &entry.manifest.snapshot.identity);
    let path = dir.join(JOURNAL);
    let existed = !journal.serialized.is_empty();
    if existed {
        journal_guard(&dir, journal)?;
    } else {
        check_file(&path, true)?;
        if path.exists() {
            return Err(damaged());
        }
    }
    hypr_storage::fs::atomic_write(&path, std::str::from_utf8(&bytes).expect("JSON is UTF-8"))
        .map_err(io_error)?;
    state.bytes = state
        .bytes
        .checked_sub(journal.serialized.len() as u64)
        .and_then(|value| value.checked_add(bytes.len() as u64))
        .ok_or_else(damaged)?;
    journal.serialized = bytes;
    if !existed {
        state.directory_entries += 1;
    }
    sync_file(&path)?;
    sync_directory(&dir)
}

pub(in crate::capture_spool) fn retire(
    spool: &CaptureSpool,
    state: &mut State,
    request: CaptureRetirementRequest,
    verified: &VerifiedSource,
    authority: impl CaptureRetirementAuthority,
) -> Result<CaptureSnapshot> {
    if state.entries.values().any(|entry| {
        entry.retirement.as_ref().is_some_and(|journal| {
            journal.request.cleanup_request_id == request.cleanup_request_id
                && journal.request.identity.scope == request.identity.scope
                && journal.request != request
        })
    }) {
        return Err(conflict(
            "Cleanup request already belongs to another immutable capture",
        ));
    }
    let coordinate = key(&request.identity);
    let mut entry = state
        .entries
        .remove(&coordinate)
        .ok_or_else(|| failure(ErrorCode::NotFound, "Capture not found in this scope"))?;
    let mut result = retire_entry(spool, state, &mut entry, request, verified, &authority);
    if let Err(error) = &mut result {
        if entry.retirement.is_some() {
            error.mutation_outcome = Some(MutationOutcome::Unknown);
            error.message = "Capture retirement stopped; verify and retry its original cleanup request to confirm the remaining raw source".into();
        }
    }
    // Even a rejected check after a successful unlink requires a fresh reload;
    // the durable intent, not the caller outcome, remains the recovery authority.
    if result.is_err() {
        state.dirty = true;
    }
    state.entries.insert(coordinate, entry);
    result
}

fn retire_entry(
    spool: &CaptureSpool,
    state: &mut State,
    entry: &mut Entry,
    request: CaptureRetirementRequest,
    verified: &VerifiedSource,
    authority: &impl CaptureRetirementAuthority,
) -> Result<CaptureSnapshot> {
    authority.check_current()?;
    guard(&spool.root, entry)?;
    validate_request(entry, &request)?;
    if !completed(entry)
        && state
            .entries
            .values()
            .filter(|entry| completed(entry))
            .count()
            >= MAX_RECEIPTS
    {
        return Err(limit());
    }
    if entry.manifest.snapshot.processing.is_none()
        && request.processing.is_some()
        && !authority.permits_external_processing(&request)
    {
        return Err(conflict(
            "Cleanup processing declaration requires its original durable owner",
        ));
    }

    if entry.serialized != verified.manifest
        || entry.retirement.as_ref().map(|journal| &journal.serialized) != verified.journal.as_ref()
    {
        return Err(damaged());
    }
    let dir = directory(&spool.root, &request.identity);
    if let Some(journal) = &entry.retirement {
        if journal.request != request {
            return Err(conflict(
                "Retirement is already bound to a different immutable cleanup request",
            ));
        }
        journal_guard(&dir, journal)?;
    } else {
        if state
            .entries
            .values()
            .filter(|entry| completed(entry))
            .count()
            >= MAX_RECEIPTS
        {
            return Err(limit());
        }
        let files = exact_files(&dir, &entry.chunks, false)?;
        if files.len() != entry.chunks.len() {
            return Err(damaged());
        }
        for (sequence, path) in files {
            if verified.files.get(&sequence) != Some(&stamp(&private_file(&path)?)?) {
                return Err(damaged());
            }
        }
        entry.retirement = Some(Journal {
            schema_version: 1,
            request,
            state: CaptureRetirementState::Pending,
            chunks: entry.chunks.clone(),
            serialized: vec![],
        });
        authority.check_current()?;
        persist_journal(spool, state, entry)?;
    }
    if entry
        .retirement
        .as_ref()
        .is_some_and(|journal| journal.state == CaptureRetirementState::Declared)
    {
        entry.retirement.as_mut().ok_or_else(damaged)?.state = CaptureRetirementState::Pending;
        authority.check_current()?;
        persist_journal(spool, state, entry)?;
    }
    let journal = entry.retirement.as_ref().ok_or_else(damaged)?;
    let files = exact_files(&dir, &journal.chunks, true)?;
    // Full hashes were verified under exclusive source admission before slow
    // remote verification. The short scope commit rechecks every original file
    // identity and byte-generation stamp before deleting any remaining source.
    for (sequence, path) in &files {
        authority.check_current()?;
        if verified.files.get(sequence) != Some(&stamp(&private_file(path)?)?) {
            return Err(damaged());
        }
    }
    let journal_stamp = stamp(&private_file(&dir.join(JOURNAL))?)?;
    let manifest_stamp = stamp(&private_file(&dir.join("manifest.json"))?)?;
    for (sequence, path) in files {
        authority.check_current()?;
        check_ancestors(&dir.join("chunks"))?;
        if stamp(&private_file(&dir.join(JOURNAL))?)? != journal_stamp
            || stamp(&private_file(&dir.join("manifest.json"))?)? != manifest_stamp
        {
            return Err(damaged());
        }
        let file = private_file(&path)?;
        if verified.files.get(&sequence) != Some(&stamp(&file)?) {
            return Err(damaged());
        }
        let size = file.metadata().map_err(io_error)?.len();
        authority.check_current()?;
        std::fs::remove_file(&path).map_err(io_error)?;
        sync_directory(&dir.join("chunks"))?;
        state.bytes = state.bytes.checked_sub(size).ok_or_else(damaged)?;
        state.chunks = state.chunks.checked_sub(1).ok_or_else(damaged)?;
        state.directory_entries = state.directory_entries.checked_sub(1).ok_or_else(damaged)?;
    }
    authority.check_current()?;
    if !exact_files(&dir, &[], true)?.is_empty() {
        return Err(damaged());
    }
    let journal = entry.retirement.as_mut().ok_or_else(damaged)?;
    if journal.state != CaptureRetirementState::Retired {
        journal.state = CaptureRetirementState::Retired;
        journal.chunks.clear();
        persist_journal(spool, state, entry)?;
    }
    entry.chunks.clear();
    snapshot(&spool.root, entry)
}

pub(in crate::capture_spool) fn declare(
    spool: &CaptureSpool,
    state: &mut State,
    request: CaptureRetirementRequest,
    verified: &VerifiedSource,
) -> Result<(CaptureSnapshot, VerifiedSource)> {
    if state.entries.values().any(|entry| {
        entry.retirement.as_ref().is_some_and(|journal| {
            journal.request.cleanup_request_id == request.cleanup_request_id
                && journal.request.identity.scope == request.identity.scope
                && journal.request != request
        })
    }) {
        return Err(conflict(
            "Cleanup request already belongs to another capture",
        ));
    }
    let coordinate = key(&request.identity);
    let mut entry = state
        .entries
        .remove(&coordinate)
        .ok_or_else(|| failure(ErrorCode::NotFound, "Capture not found"))?;
    let result = (|| {
        guard(&spool.root, &entry)?;
        validate_request(&entry, &request)?;
        if entry
            .manifest
            .snapshot
            .processing
            .as_ref()
            .map(|declaration| &declaration.request)
            != request.processing.as_ref()
        {
            return Err(conflict(
                "Cleanup declaration does not match the original device processing intent",
            ));
        }
        if entry.serialized != verified.manifest
            || entry.retirement.as_ref().map(|journal| &journal.serialized)
                != verified.journal.as_ref()
        {
            return Err(damaged());
        }
        if let Some(journal) = &entry.retirement {
            if journal.request != request {
                return Err(conflict("Retry the original cleanup request"));
            }
        } else {
            let dir = directory(&spool.root, &request.identity);
            for (sequence, path) in exact_files(&dir, &entry.chunks, false)? {
                if verified.files.get(&sequence) != Some(&stamp(&private_file(&path)?)?) {
                    return Err(damaged());
                }
            }
            entry.retirement = Some(Journal {
                schema_version: 1,
                request,
                state: CaptureRetirementState::Declared,
                chunks: entry.chunks.clone(),
                serialized: vec![],
            });
            persist_journal(spool, state, &mut entry)?;
        }
        let mut verified = verified.clone();
        verified.journal = entry
            .retirement
            .as_ref()
            .map(|journal| journal.serialized.clone());
        Ok((snapshot(&spool.root, &entry)?, verified))
    })();
    if result.is_err() {
        state.dirty = true;
    }
    state.entries.insert(coordinate, entry);
    result
}

pub(super) fn validate_receipts(state: &State) -> Result<()> {
    let mut requests = std::collections::BTreeSet::new();
    for entry in state.entries.values() {
        if let Some(journal) = &entry.retirement {
            if !requests.insert((
                scope_key(&journal.request.identity.scope),
                journal.request.cleanup_request_id.clone(),
            )) {
                return Err(damaged());
            }
        }
    }
    Ok(())
}
