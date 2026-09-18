//! Flat session identity: a session lives only at `sessions/<id>`.
use crate::{Error, Result, SessionMeta, paths};
use std::borrow::Cow;
use std::path::{Path, PathBuf};
use unicode_normalization::{UnicodeNormalization, is_nfc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionLocation {
    pub id: String,
    pub relative_dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SessionDiscoveryError {
    #[error("invalid session metadata in '{}': {reason}", dir.display())]
    CorruptMeta { dir: PathBuf, reason: String },
    #[error("failed to scan '{}': {reason}", dir.display())]
    Unreadable { dir: PathBuf, reason: String },
}

#[derive(Debug, Default)]
pub struct SessionDiscovery {
    pub sessions: Vec<(SessionLocation, SessionMeta)>,
    pub errors: Vec<SessionDiscoveryError>,
    pub ghost_dirs: Vec<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum SessionLookupError {
    #[error("invalid session metadata in '{}': {reason}", dir.display())]
    Corrupt { dir: PathBuf, reason: String },
    #[error("I/O error: {0}")]
    Io(String),
}

/// NFC-normalize a name for comparison. macOS APFS preserves whatever form it is
/// given, but sync providers and cross-machine copies can return a differently
/// composed form of the same name, so byte-for-byte comparison is a live bug once
/// names contain user text.
pub fn nfc(value: &str) -> Cow<'_, str> {
    if is_nfc(value) {
        Cow::Borrowed(value)
    } else {
        Cow::Owned(value.nfc().collect())
    }
}

/// NFC-normalized equality for file/directory names and identifiers.
pub fn eq_nfc(a: &str, b: &str) -> bool {
    nfc(a) == nfc(b)
}

/// The one session-boundary rule, shared by every layout consumer: `_meta.json`
/// absent → no session metadata; present and parseable → a migration candidate; present but
/// unreadable/corrupt → still a session boundary, never a folder (its content
/// must not be traversed, misread as nested sessions, or treated as deletable).
pub enum SessionDirKind {
    Session(Box<SessionMeta>),
    /// `_meta.json` exists but cannot be read or parsed; carries the reason.
    Corrupt(String),
    Folder,
}

pub fn classify_session_dir(abs_dir: &Path) -> SessionDirKind {
    match std::fs::read(abs_dir.join("_meta.json")) {
        Ok(bytes) => match serde_json::from_slice::<SessionMeta>(&bytes) {
            Ok(meta) => SessionDirKind::Session(Box::new(meta)),
            Err(e) => SessionDirKind::Corrupt(format!("failed to deserialize meta: {e}")),
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SessionDirKind::Folder,
        Err(e) => SessionDirKind::Corrupt(format!("failed to read meta file: {e}")),
    }
}

/// A discovery scan is one `_meta.json` read per candidate directory; on a
/// high-latency filesystem (network mounts, cold sync caches) those reads are
/// round-trips and doing them serially makes the scan minutes long. Bounded so
/// throttling backends are not hammered; local filesystems finish either way.
const CLASSIFY_WORKERS: usize = 16;
/// Below this many children the thread-spawn overhead is not worth paying.
const CLASSIFY_INLINE_MAX: usize = 4;

/// `classify_session_dir` over many candidates, fanning the `_meta.json` reads
/// out over a bounded set of worker threads. Results are returned in input
/// order, so callers observe exactly the sequential behavior.
fn classify_session_dirs(child_dirs: &[(PathBuf, PathBuf)]) -> Vec<SessionDirKind> {
    if child_dirs.len() <= CLASSIFY_INLINE_MAX {
        return child_dirs
            .iter()
            .map(|(_, abs_dir)| classify_session_dir(abs_dir))
            .collect();
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut indexed: Vec<(usize, SessionDirKind)> = std::thread::scope(|scope| {
        let workers = (0..child_dirs.len().min(CLASSIFY_WORKERS))
            .map(|_| {
                scope.spawn(|| {
                    let mut classified = Vec::new();
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((_, abs_dir)) = child_dirs.get(index) else {
                            return classified;
                        };
                        classified.push((index, classify_session_dir(abs_dir)));
                    }
                })
            })
            .collect::<Vec<_>>();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("classify worker panicked"))
            .collect()
    });
    indexed.sort_by_key(|(index, _)| *index);
    indexed.into_iter().map(|(_, kind)| kind).collect()
}

/// Cheap boundary probe for traversals that only need "may recurse" versus "must
/// stop": stats `_meta.json` without parsing it. `NotFound` is the only answer that
/// makes a folder; any other stat error is conservatively a boundary — the same
/// rule as `classify_session_dir`, and deliberately not `Path::exists()`, which
/// would hide a permission error as absence and let a traversal walk into a
/// session directory it cannot actually judge.
pub fn has_session_boundary(abs_dir: &Path) -> bool {
    match std::fs::metadata(abs_dir.join("_meta.json")) {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(_) => true,
    }
}

/// One bounded, shallow metadata pass. Only desktop startup migration should use
/// the unfiltered snapshot; normal discovery requires canonical directory names.
pub fn scan_top_level_directories(
    vault: &Path,
    mut on_sessions_found: impl FnMut(usize),
) -> Result<SessionDiscovery> {
    let root = paths::sessions_root();
    let entries = match std::fs::read_dir(vault.join(&root)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(SessionDiscovery::default());
        }
        Err(e) => return Err(Error::Io(format!("failed to read sessions dir: {e}"))),
    };
    let mut scan = SessionDiscovery::default();
    let mut children = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                scan.errors.push(SessionDiscoveryError::Unreadable {
                    dir: root.clone(),
                    reason: e.to_string(),
                });
                continue;
            }
        };
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if name.starts_with('.') {
            continue;
        }
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => children.push((root.join(name), entry.path())),
            Ok(_) => {}
            Err(e) => scan.errors.push(SessionDiscoveryError::Unreadable {
                dir: root.join(name),
                reason: e.to_string(),
            }),
        }
    }
    children.sort_by(|a, b| a.0.cmp(&b.0));
    // Batches keep progress flowing while bounding concurrent metadata hydration.
    for batch in children.chunks(64) {
        for ((relative, absolute), kind) in batch.iter().zip(classify_session_dirs(batch)) {
            match kind {
                SessionDirKind::Session(meta) => {
                    scan.sessions.push((
                        SessionLocation {
                            id: meta.id.clone(),
                            relative_dir: relative.clone(),
                        },
                        *meta,
                    ));
                }
                SessionDirKind::Corrupt(reason) => {
                    scan.errors.push(SessionDiscoveryError::CorruptMeta {
                        dir: relative.clone(),
                        reason,
                    })
                }
                SessionDirKind::Folder => {
                    if ["notes.md", "_memo.md", "transcript.json"]
                        .iter()
                        .any(|name| absolute.join(name).is_file())
                    {
                        scan.ghost_dirs.push(relative.clone());
                    }
                }
            }
        }
        on_sessions_found(scan.sessions.len());
    }
    Ok(scan)
}

pub fn discover_sessions(vault: &Path) -> Result<SessionDiscovery> {
    discover_sessions_with_progress(vault, |_| {})
}

pub fn discover_sessions_with_progress(
    vault: &Path,
    on_sessions_found: impl FnMut(usize),
) -> Result<SessionDiscovery> {
    let mut scan = scan_top_level_directories(vault, on_sessions_found)?;
    scan.sessions.retain(
        |(location, meta)| match paths::validated_session_dir(&meta.id) {
            Ok(dir) if dir == location.relative_dir => true,
            result => {
                let reason = match result {
                    Err(e) => e.to_string(),
                    Ok(_) => {
                        "noncanonical directory; open the updated desktop app to migrate this vault"
                            .to_string()
                    }
                };
                scan.errors.push(SessionDiscoveryError::CorruptMeta {
                    dir: location.relative_dir.clone(),
                    reason,
                });
                false
            }
        },
    );
    Ok(scan)
}

pub fn find_session(
    vault: &Path,
    id: &str,
) -> std::result::Result<Option<(SessionLocation, SessionMeta)>, SessionLookupError> {
    let dir =
        paths::validated_session_dir(id).map_err(|e| SessionLookupError::Io(e.to_string()))?;
    match crate::meta::read_session_meta_in(vault, &dir) {
        Ok(meta) => Ok(meta.map(|meta| {
            (
                SessionLocation {
                    id: id.to_string(),
                    relative_dir: dir.clone(),
                },
                meta,
            )
        })),
        Err(Error::Parse(reason)) => Err(SessionLookupError::Corrupt { dir, reason }),
        Err(Error::Io(reason)) => Err(SessionLookupError::Io(reason)),
    }
}

/// Artifact paths are pure: misses cost the same amount of work as hits.
pub fn artifact_dir(_vault: &Path, id: &str) -> Result<PathBuf> {
    paths::validated_session_dir(id)
}
