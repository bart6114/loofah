use std::{
    fs::Metadata,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
    time::UNIX_EPOCH,
};

use hypr_vault_read::reserved::{SESSION_OWNED_DIRS, SESSION_OWNED_FILES, SESSION_TRANSIENT_FILES};
use sha2::{Digest, Sha256};

use super::{LifecycleError, io_error, rejected};
use crate::{
    ContentLimits, SessionMeta, StoreError, file_snapshot::FileSnapshot, media::open_regular_file,
};

type SnapshotFiles = Vec<(PathBuf, FileSnapshot)>;

#[derive(PartialEq, Eq)]
struct Entry {
    relative: PathBuf,
    kind: u8,
    stamp: Vec<u8>,
}
pub(super) struct Tree {
    pub revision: String,
    entries: Vec<Entry>,
    files: SnapshotFiles,
}

fn stamp(meta: &Metadata) -> Vec<u8> {
    let mut value = meta.len().to_le_bytes().to_vec();
    for time in [meta.modified().ok(), meta.created().ok()] {
        match time.and_then(|time| time.duration_since(UNIX_EPOCH).ok()) {
            Some(time) => {
                value.push(1);
                value.extend(time.as_secs().to_le_bytes());
                value.extend(time.subsec_nanos().to_le_bytes());
            }
            None => value.push(0),
        }
    }
    value
}
fn push(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn manifest(
    directory: &Path,
    open: bool,
    limits: ContentLimits,
) -> Result<(Vec<Entry>, SnapshotFiles), StoreError> {
    let limit = if limits.metadata_bytes == usize::MAX {
        usize::MAX
    } else {
        4096
    };
    let mut entries = Vec::new();
    let mut files = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(parent) = pending.pop() {
        for entry in std::fs::read_dir(directory.join(&parent)).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name();
            if name.as_encoded_bytes().starts_with(b".") {
                continue;
            }
            let relative = parent.join(&name);
            let meta = entry.path().symlink_metadata().map_err(io_error)?;
            if parent.as_os_str().is_empty()
                && name
                    .to_str()
                    .is_some_and(|name| SESSION_TRANSIENT_FILES.contains(&name))
            {
                return Err(StoreError::Conflict(
                    "Recording intermediates require review".into(),
                ));
            }
            let owned = !parent.as_os_str().is_empty()
                || name.to_str().is_some_and(|name| {
                    SESSION_OWNED_FILES.contains(&name) || SESSION_OWNED_DIRS.contains(&name)
                });
            if owned && meta.file_type().is_symlink() {
                return Err(StoreError::Io(
                    "Session-owned paths must not be symlinks".into(),
                ));
            }
            let kind = if meta.file_type().is_symlink() {
                3
            } else if meta.is_dir() {
                2
            } else if meta.is_file() {
                1
            } else {
                return Err(StoreError::Io("Unsupported session entry".into()));
            };
            let mut fingerprint = stamp(&meta);
            if kind == 3 {
                fingerprint.extend(
                    std::fs::read_link(entry.path())
                        .map_err(io_error)?
                        .as_os_str()
                        .as_encoded_bytes(),
                );
            }
            if entries.len() >= limit {
                return Err(StoreError::ResourceLimit(
                    "Session has too many entries for lifecycle inspection".into(),
                ));
            }
            entries.push(Entry {
                relative: relative.clone(),
                kind,
                stamp: fingerprint,
            });
            if owned && kind == 2 {
                pending.push(relative);
            } else if owned && kind == 1 && open {
                let file = open_regular_file(&entry.path())?.ok_or_else(|| {
                    StoreError::Conflict("Session changed during inspection".into())
                })?;
                files.push((relative, FileSnapshot::new(file)?));
            }
        }
    }
    entries.sort_by(|a, b| a.relative.cmp(&b.relative));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok((entries, files))
}

pub(super) fn safe_directory(vault: &Path, relative: &Path) -> Result<bool, StoreError> {
    let mut path = vault.to_path_buf();
    for part in relative.components() {
        if !matches!(part, Component::Normal(_)) {
            return Err(StoreError::Io("Invalid session lifecycle path".into()));
        }
        path.push(part);
        match path.symlink_metadata() {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Ok(_) => {
                return Err(StoreError::Io(
                    "Session lifecycle parent is not a regular directory".into(),
                ));
            }
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(cause) => return Err(io_error(cause)),
        }
    }
    Ok(true)
}

pub(super) fn prepare(
    vault: &Path,
    relative: &Path,
    id: &str,
    limits: ContentLimits,
) -> Result<Option<Tree>, LifecycleError> {
    if !safe_directory(vault, relative).map_err(rejected)? {
        return Ok(None);
    }
    let directory = vault.join(relative);
    let (entries, files) = manifest(&directory, true, limits).map_err(rejected)?;
    let meta = files
        .iter()
        .find(|(path, _)| path == Path::new("_meta.json"))
        .ok_or_else(|| rejected(StoreError::Io("Session metadata is missing".into())))?;
    if meta
        .1
        .file()
        .metadata()
        .map_err(io_error)
        .map_err(rejected)?
        .len()
        > limits.metadata_bytes as u64
    {
        return Err(rejected(StoreError::ResourceLimit(
            "Session metadata is too large".into(),
        )));
    }
    let mut reader = meta.1.file();
    reader
        .seek(SeekFrom::Start(0))
        .map_err(io_error)
        .map_err(rejected)?;
    let mut raw = Vec::new();
    reader
        .take((limits.metadata_bytes as u64).saturating_add(1))
        .read_to_end(&mut raw)
        .map_err(io_error)
        .map_err(rejected)?;
    if raw.len() > limits.metadata_bytes {
        return Err(rejected(StoreError::ResourceLimit(
            "Session metadata is too large".into(),
        )));
    }
    let strict = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&raw)
        .map_err(|cause| rejected(StoreError::Serialize(cause.to_string())))?;
    let metadata: SessionMeta = serde_json::from_value(strict.0)
        .map_err(|cause| rejected(StoreError::Serialize(cause.to_string())))?;
    if metadata.id != id {
        return Err(rejected(StoreError::Conflict(
            "Session identity changed".into(),
        )));
    }
    if metadata
        .extra
        .get("audio_import_pending")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return Err(LifecycleError::Busy);
    }
    let mut hash = Sha256::new();
    hash.update(b"loofah-session-lifecycle-v1");
    for entry in &entries {
        push(&mut hash, entry.relative.as_os_str().as_encoded_bytes());
        hash.update([entry.kind]);
        push(&mut hash, &entry.stamp);
    }
    // Loose user attachments stay opaque. Only exact app-owned namespaces are read.
    for (relative, file) in &files {
        if !file.lock_for_recheck().map_err(rejected)? {
            return Err(LifecycleError::Busy);
        }
        push(&mut hash, relative.as_os_str().as_encoded_bytes());
        push(&mut hash, file.hash().map_err(rejected)?.as_bytes());
    }
    let revision = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let tree = Tree {
        revision,
        entries,
        files,
    };
    if !tree.unchanged(vault, relative, limits).map_err(rejected)? {
        return Err(rejected(StoreError::Conflict(
            "Session changed during inspection".into(),
        )));
    }
    Ok(Some(tree))
}

impl Tree {
    pub fn unchanged(
        &self,
        vault: &Path,
        relative: &Path,
        limits: ContentLimits,
    ) -> Result<bool, StoreError> {
        if !safe_directory(vault, relative)? {
            return Ok(false);
        }
        let directory = vault.join(relative);
        let (entries, _) = manifest(&directory, false, limits)?;
        if entries != self.entries {
            return Ok(false);
        }
        for (relative, original) in &self.files {
            let Some(file) = open_regular_file(&directory.join(relative))? else {
                return Ok(false);
            };
            if !original.unchanged(&FileSnapshot::new(file)?)? {
                return Ok(false);
            }
        }
        Ok(true)
    }
}
