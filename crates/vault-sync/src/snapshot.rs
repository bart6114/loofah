use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use hypr_vault_read::transaction::VaultTransaction;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use unicode_normalization::UnicodeNormalization;

use crate::scope::{Entity, MAX_FILES, PROTOCOL_VERSION};
use crate::{Error, Result};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileDigest {
    pub bytes: u64,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub version: u32,
    pub entity: Entity,
    pub files: BTreeMap<String, FileDigest>,
}

impl Manifest {
    pub fn validate(&self) -> Result<()> {
        self.entity.root()?;
        if self.version != PROTOCOL_VERSION || self.files.is_empty() || self.files.len() > MAX_FILES
        {
            return Err(Error::Invalid("unsupported or incomplete manifest".into()));
        }
        let mut names = std::collections::BTreeSet::new();
        for (path, digest) in &self.files {
            self.entity.validate_path(path)?;
            if !names.insert(path.nfd().collect::<String>().to_lowercase()) {
                return Err(Error::Invalid("ambiguous content paths".into()));
            }
            if digest.sha256.len() != 64
                || !digest
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(Error::Invalid("invalid file digest".into()));
            }
        }
        let required = self.entity.registry_name().unwrap_or("_meta.json");
        if !self.files.contains_key(required) {
            return Err(Error::Invalid("missing entity identity".into()));
        }
        Ok(())
    }

    pub fn changed_paths(&self, previous: &Self) -> Result<Vec<String>> {
        if self.entity != previous.entity {
            return Err(Error::Invalid("different entities".into()));
        }
        Ok(self
            .files
            .keys()
            .chain(previous.files.keys())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .filter(|path| self.files.get(*path) != previous.files.get(*path))
            .cloned()
            .collect())
    }
}

pub struct Snapshot {
    manifest: Manifest,
    directory: tempfile::TempDir,
    stamps: BTreeMap<String, Stamp>,
}

impl Snapshot {
    pub(crate) fn verified(manifest: Manifest, directory: tempfile::TempDir) -> Result<Self> {
        manifest.validate()?;
        validate_content(directory.path(), &manifest)?;
        Ok(Self {
            manifest,
            directory,
            stamps: BTreeMap::new(),
        })
    }
    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn directory(&self) -> &Path {
        self.directory.path()
    }

    /// Run on a blocking worker. No hashing or file copying takes place under the guard.
    pub fn capture(vault: &Path, entity: Entity, spool: &Path) -> Result<Self> {
        Self::capture_with(vault, entity, spool, || {})
    }

    fn capture_with(
        vault: &Path,
        entity: Entity,
        spool: &Path,
        after_copy: impl FnOnce(),
    ) -> Result<Self> {
        let vault = vault.canonicalize()?;
        fs::create_dir_all(spool)?;
        let spool = spool.canonicalize()?;
        if spool.starts_with(&vault) {
            return Err(Error::Invalid(
                "snapshot spool must be outside the vault".into(),
            ));
        }
        let root = vault.join(entity.root()?);
        let stamps = {
            let _guard = VaultTransaction::shared(&vault)?;
            VaultTransaction::ensure_ready(&vault)?;
            inventory(&vault, &entity)?
        };
        let required = stamps.values().try_fold(0u64, |total, stamp| {
            total.checked_add(stamp.bytes).ok_or(Error::DiskFull)
        })?;
        if fs2::available_space(&spool)? < required.saturating_add(16 * 1024 * 1024) {
            return Err(Error::DiskFull);
        }
        let directory = tempfile::Builder::new()
            .prefix("loofah-snapshot-")
            .tempdir_in(spool)?;
        let mut files = BTreeMap::new();
        for (name, stamp) in &stamps {
            let source = root.join(name);
            let destination = directory.path().join(name);
            fs::create_dir_all(destination.parent().unwrap())?;
            let mut input = open_capture(&source)?;
            if Stamp::from_metadata(&input.metadata()?)? != *stamp {
                return Err(Error::Conflict);
            }
            let mut output = File::create_new(&destination)?;
            let digest = copy_digest(&mut input, &mut output)?;
            output.sync_all()?;
            if Stamp::from_metadata(&input.metadata()?)? != *stamp || digest.bytes != stamp.bytes {
                return Err(Error::Conflict);
            }
            files.insert(name.clone(), digest);
        }
        after_copy();
        {
            let _guard = VaultTransaction::shared(&vault)?;
            VaultTransaction::ensure_ready(&vault)?;
            if inventory(&vault, &entity)? != stamps {
                return Err(Error::Conflict);
            }
        }
        let manifest = Manifest {
            version: PROTOCOL_VERSION,
            entity,
            files,
        };
        manifest.validate()?;
        validate_content(directory.path(), &manifest)?;
        Ok(Self {
            manifest,
            directory,
            stamps,
        })
    }

    /// Caller holds the vault transaction across this check and journaled installation.
    pub(crate) fn still_current(&self, vault: &Path) -> Result<bool> {
        Ok(inventory(vault, &self.manifest.entity)? == self.stamps)
    }

    pub(crate) fn matches_saved(&self, name: &str, path: &Path) -> Result<bool> {
        let actual = Stamp::from_metadata(&fs::symlink_metadata(path)?)?;
        let Some(expected) = self.stamps.get(name) else {
            return Ok(false);
        };
        Ok(
            actual.bytes == expected.bytes && actual.modified == expected.modified && {
                #[cfg(unix)]
                {
                    actual.identity.0 == expected.identity.0
                        && actual.identity.1 == expected.identity.1
                }
                #[cfg(not(unix))]
                {
                    true
                }
            },
        )
    }
}

pub(crate) fn validate_content(directory: &Path, manifest: &Manifest) -> Result<()> {
    let name = manifest.entity.registry_name().unwrap_or("_meta.json");
    let metadata = fs::metadata(directory.join(name))?;
    if metadata.len() > 32 * 1024 * 1024 {
        return Err(Error::Invalid("oversized entity metadata".into()));
    }
    let raw = fs::read(directory.join(name))?;
    let value: serde_json::Value =
        serde_json::from_slice(&raw).map_err(|_| Error::Invalid("invalid entity JSON".into()))?;
    if let Entity::Session(id) = &manifest.entity {
        if value.get("id").and_then(|v| v.as_str()) != Some(id) {
            return Err(Error::Invalid("session identity mismatch".into()));
        }
        serde_json::from_slice::<hypr_vault_read::SessionMeta>(&raw)
            .map_err(|_| Error::Invalid("invalid session metadata".into()))?;
    } else if !value.is_object() {
        return Err(Error::Invalid("invalid registry structure".into()));
    } else {
        match manifest.entity {
            Entity::People => {
                serde_json::from_value::<Vec<hypr_vault_read::Person>>(
                    value
                        .get("people")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .map_err(|_| Error::Invalid("invalid people registry".into()))?;
            }
            Entity::Tags => {
                serde_json::from_value::<Vec<hypr_vault_write::TagItem>>(
                    value
                        .get("tags")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!([])),
                )
                .map_err(|_| Error::Invalid("invalid tag registry".into()))?;
            }
            _ => {}
        }
    }
    for name in manifest.files.keys() {
        if name.ends_with(".md") && !name.starts_with("attachments/") {
            if fs::metadata(directory.join(name))?.len() > 32 * 1024 * 1024 {
                return Err(Error::Invalid("text content too large".into()));
            }
            let raw = fs::read_to_string(directory.join(name))?;
            if let Entity::Session(id) = &manifest.entity
                && let Some(doc) = name
                    .strip_prefix("enhanced/")
                    .and_then(|name| name.strip_suffix(".md"))
            {
                hypr_vault_read::parse_enhanced_file(doc, id, &raw)
                    .map_err(|_| Error::Invalid("invalid enhanced document".into()))?;
            }
        }
        if name == "transcript.json" || name == "tasks.json" {
            if fs::metadata(directory.join(name))?.len() > 128 * 1024 * 1024 {
                return Err(Error::Invalid("structured content too large".into()));
            }
            let bytes = fs::read(directory.join(name))?;
            if name == "transcript.json" {
                serde_json::from_slice::<hypr_vault_read::TranscriptJson>(&bytes)
                    .map_err(|_| Error::Invalid("invalid transcript structure".into()))?;
            } else {
                serde_json::from_slice::<hypr_vault_read::TasksFile>(&bytes)
                    .map_err(|_| Error::Invalid("invalid task structure".into()))?;
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Stamp {
    bytes: u64,
    modified: std::time::SystemTime,
    #[cfg(unix)]
    identity: (u64, u64, i64, i64),
}

impl Stamp {
    fn from_metadata(meta: &fs::Metadata) -> Result<Self> {
        if !meta.is_file() {
            return Err(Error::Invalid("content is not a regular file".into()));
        }
        Ok(Self {
            bytes: meta.len(),
            modified: meta.modified()?,
            #[cfg(unix)]
            identity: {
                use std::os::unix::fs::MetadataExt;
                (meta.dev(), meta.ino(), meta.ctime(), meta.ctime_nsec())
            },
        })
    }
}

pub(crate) fn inventory(vault: &Path, entity: &Entity) -> Result<BTreeMap<String, Stamp>> {
    if let Entity::Session(id) = entity
        && VaultTransaction::is_recording(vault, id)?
    {
        return Err(Error::Recording);
    }
    let relative = entity.root()?;
    checked_path(vault, &relative)?;
    let root = vault.join(relative);
    let mut files = BTreeMap::new();
    if let Some(name) = entity.registry_name() {
        add_file(&root, name, entity, &mut files)?;
    } else {
        for name in hypr_vault_read::SESSION_TRANSIENT_FILES {
            if fs::symlink_metadata(root.join(name)).is_ok() {
                return Err(Error::Recording);
            }
        }
        for name in hypr_vault_read::SESSION_OWNED_FILES {
            add_file(&root, name, entity, &mut files)?;
        }
        for name in ["enhanced", "attachments", "audio"] {
            walk_owned(&root, Path::new(name), entity, &mut files, 0)?;
        }
    }
    Ok(files)
}

fn add_file(
    root: &Path,
    name: &str,
    entity: &Entity,
    files: &mut BTreeMap<String, Stamp>,
) -> Result<()> {
    entity.validate_path(name)?;
    match fs::symlink_metadata(root.join(name)) {
        Ok(meta) => {
            files.insert(name.into(), Stamp::from_metadata(&meta)?);
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    if files.len() > MAX_FILES {
        return Err(Error::Invalid("too many content files".into()));
    }
    Ok(())
}

fn walk_owned(
    root: &Path,
    relative: &Path,
    entity: &Entity,
    files: &mut BTreeMap<String, Stamp>,
    depth: usize,
) -> Result<()> {
    if depth > 32 {
        return Err(Error::Invalid("content nesting too deep".into()));
    }
    let metadata = match fs::symlink_metadata(root.join(relative)) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    if !metadata.is_dir() {
        return Err(Error::Invalid("unsafe managed directory".into()));
    }
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| Error::Invalid("non-UTF8 content path".into()))?;
        if name.starts_with('.') {
            continue;
        }
        let path = relative.join(name);
        let name = path
            .to_str()
            .ok_or_else(|| Error::Invalid("non-UTF8 content path".into()))?;
        if entry.file_type()?.is_dir() && relative.starts_with("attachments") {
            walk_owned(root, &path, entity, files, depth + 1)?;
        } else if entity.validate_path(name).is_ok() {
            add_file(root, name, entity, files)?;
        }
    }
    Ok(())
}

pub(crate) fn checked_path(root: &Path, relative: &Path) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for part in relative.components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(Error::Invalid("unsafe path component".into()));
        }
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(Error::Invalid("symlink in content path".into()));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    Ok(path)
}

pub(crate) fn copy_digest(input: &mut impl Read, output: &mut impl Write) -> Result<FileDigest> {
    let mut hash = Sha256::new();
    let mut bytes = 0;
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        hash.update(&buffer[..count]);
        bytes += count as u64;
    }
    Ok(FileDigest {
        bytes,
        sha256: hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    })
}

pub(crate) fn open_capture(path: &Path) -> Result<File> {
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        // An external editor may replace a file after inventory. Never follow
        // that replacement as a symlink or block indefinitely opening a FIFO.
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(Error::Conflict);
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn fixture(vault: &Path) {
        let dir = vault.join("sessions/one");
        fs::create_dir_all(dir.join("attachments")).unwrap();
        fs::write(dir.join("_meta.json"), br#"{"id":"one","title":"test","created_at":"2026-09-16","tags":[],"future":{"keep":true}}"#).unwrap();
        fs::write(dir.join("notes.md"), b"original").unwrap();
        fs::write(dir.join("audio.mp3"), b"recording").unwrap();
        fs::write(dir.join("attachments/picture.png"), b"picture").unwrap();
        fs::write(dir.join("private.pdf"), b"excluded").unwrap();
        fs::write(dir.join("audio.peaks.json"), b"excluded").unwrap();
        fs::write(dir.join("attachments/.private"), b"excluded").unwrap();
    }

    #[test]
    fn duplicate_session_identity_fields_are_rejected_before_installation() {
        let vault = tempfile::tempdir().unwrap();
        let spool = tempfile::tempdir().unwrap();
        fixture(vault.path());
        fs::write(
            vault.path().join("sessions/one/_meta.json"),
            br#"{"id":"other","id":"one","title":"test","created_at":"2026-09-16","tags":[]}"#,
        )
        .unwrap();
        assert!(
            Snapshot::capture(vault.path(), Entity::Session("one".into()), spool.path()).is_err()
        );
    }

    #[test]
    fn registry_structure_is_validated_without_dropping_unknown_fields() {
        let vault = tempfile::tempdir().unwrap();
        let spool = tempfile::tempdir().unwrap();
        for (entity, name, property) in [
            (Entity::People, "people.json", "people"),
            (Entity::Tags, "tags.json", "tags"),
        ] {
            fs::write(vault.path().join(name), serde_json::to_vec(&serde_json::json!({property: [{"id": "one", "future": true}], "future": {"keep": true}})).unwrap()).unwrap();
            let snapshot = Snapshot::capture(vault.path(), entity.clone(), spool.path()).unwrap();
            assert_eq!(
                fs::read(vault.path().join(name)).unwrap(),
                fs::read(snapshot.directory().join(name)).unwrap()
            );
            fs::write(
                vault.path().join(name),
                serde_json::to_vec(&serde_json::json!({property: [42]})).unwrap(),
            )
            .unwrap();
            assert!(Snapshot::capture(vault.path(), entity, spool.path()).is_err());
        }
    }

    #[test]
    fn captures_only_owned_content_and_keeps_unchanged_recording_identity() {
        let vault = tempfile::tempdir().unwrap();
        let spool = tempfile::tempdir().unwrap();
        fixture(vault.path());
        let entity = Entity::Session("one".into());
        let before = Snapshot::capture(vault.path(), entity.clone(), spool.path()).unwrap();
        fs::write(vault.path().join("sessions/one/notes.md"), b"edited").unwrap();
        let after = Snapshot::capture(vault.path(), entity, spool.path()).unwrap();
        assert_eq!(before.manifest.files.len(), 4);
        assert_eq!(
            after.manifest.changed_paths(&before.manifest).unwrap(),
            ["notes.md"]
        );
        assert_eq!(
            before.manifest.files["audio.mp3"],
            after.manifest.files["audio.mp3"]
        );
    }

    #[test]
    fn external_edits_and_new_files_invalidate_capture() {
        let vault = tempfile::tempdir().unwrap();
        let spool = tempfile::tempdir().unwrap();
        fixture(vault.path());
        for path in ["notes.md", "attachments/new.txt"] {
            let result = Snapshot::capture_with(
                vault.path(),
                Entity::Session("one".into()),
                spool.path(),
                || {
                    fs::write(vault.path().join("sessions/one").join(path), b"competing").unwrap();
                },
            );
            assert!(matches!(result, Err(Error::Conflict)));
        }
    }

    #[test]
    fn missing_identity_never_becomes_a_deletion_and_recording_is_deferred() {
        let vault = tempfile::tempdir().unwrap();
        let spool = tempfile::tempdir().unwrap();
        assert!(
            Snapshot::capture(vault.path(), Entity::Session("absent".into()), spool.path())
                .is_err()
        );
        fixture(vault.path());
        fs::write(vault.path().join("sessions/one/audio_mic.wav"), b"live").unwrap();
        assert!(matches!(
            Snapshot::capture(vault.path(), Entity::Session("one".into()), spool.path()),
            Err(Error::Recording)
        ));
    }

    #[cfg(unix)]
    #[test]
    fn capture_open_rejects_a_symlink_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let original = directory.path().join("original");
        fs::write(&original, b"preserve").unwrap();
        let replacement = directory.path().join("replacement");
        std::os::unix::fs::symlink(&original, &replacement).unwrap();
        assert!(open_capture(&replacement).is_err());
        assert!(open_capture(directory.path()).is_err());
        assert!(open_capture(&original).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_content_is_rejected() {
        let vault = tempfile::tempdir().unwrap();
        let spool = tempfile::tempdir().unwrap();
        fixture(vault.path());
        std::os::unix::fs::symlink(
            "/etc/passwd",
            vault.path().join("sessions/one/attachments/leak.txt"),
        )
        .unwrap();
        assert!(
            Snapshot::capture(vault.path(), Entity::Session("one".into()), spool.path()).is_err()
        );
    }
}
