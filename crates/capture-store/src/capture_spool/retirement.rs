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
    for (sequence, path) in files {
        let chunk = read_file(&path, limits)?;
        if chunk.descriptor != journal.chunks[sequence as usize] {
            return Err(damaged());
        }
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
    let mut result = retire_entry(spool, state, &mut entry, request, &authority);
    if let Err(error) = &mut result {
        if entry.retirement.is_some() {
            error.mutation_outcome = Some(MutationOutcome::Unknown);
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
    authority: &impl CaptureRetirementAuthority,
) -> Result<CaptureSnapshot> {
    authority.check_current()?;
    guard(&spool.root, entry)?;
    validate_request(entry, &request)?;
    if entry.manifest.snapshot.processing.is_none()
        && request.processing.is_some()
        && !authority.permits_external_processing(&request)
    {
        return Err(conflict(
            "Cleanup processing declaration requires its original durable owner",
        ));
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
        let mut source = Sha256::new();
        for (sequence, path) in files {
            authority.check_current()?;
            let chunk = read_file(&path, &spool.limits)?;
            if chunk.descriptor != entry.chunks[sequence as usize] {
                return Err(damaged());
            }
            source.update(&chunk.bytes);
        }
        if source_digest(&source) != request.source.source_sha256 {
            return Err(damaged());
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
    let journal = entry.retirement.as_ref().ok_or_else(damaged)?;
    let files = exact_files(&dir, &journal.chunks, true)?;
    // Verify the entire remaining set before deleting any file on an explicit retry.
    for (sequence, path) in &files {
        authority.check_current()?;
        if read_file(path, &spool.limits)?.descriptor != journal.chunks[*sequence as usize] {
            return Err(damaged());
        }
    }
    for (sequence, path) in files {
        authority.check_current()?;
        guard(&spool.root, entry)?;
        journal_guard(&dir, entry.retirement.as_ref().ok_or_else(damaged)?)?;
        let file = private_file(&path)?;
        if read_file(&path, &spool.limits)?.descriptor
            != entry.retirement.as_ref().ok_or_else(damaged)?.chunks[sequence as usize]
        {
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
