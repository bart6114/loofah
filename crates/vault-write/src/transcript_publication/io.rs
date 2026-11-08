use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{StoreError, file_snapshot::FileSnapshot, media::open_regular_file};

use super::{TranscriptPublicationIdentity, valid_text};

pub(super) const RESULT_LIMIT: usize = 64 * 1024 * 1024;
const RECEIPT_LIMIT: usize = 64 * 1024;

pub(super) fn encode_result(file: &hypr_fs_format::TranscriptJson) -> Result<Vec<u8>, StoreError> {
    struct Output {
        bytes: Vec<u8>,
        exceeded: bool,
    }
    impl Write for Output {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > RESULT_LIMIT.saturating_sub(self.bytes.len()) {
                self.exceeded = true;
                return Err(std::io::Error::other(
                    "Transcript result exceeds its size limit",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = Output {
        bytes: Vec::new(),
        exceeded: false,
    };
    let result = serde_json::to_writer_pretty(&mut output, file);
    if output.exceeded {
        return Err(StoreError::ResourceLimit(
            "Generated transcript exceeds its size limit".into(),
        ));
    }
    result.map_err(serialization)?;
    Ok(output.bytes)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Binding {
    pub session_id: String,
    pub recording_selection: String,
    pub recording_sha256: String,
    pub transcript_revision: String,
    pub audio: hypr_vault_read::SessionAudio,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub schema_version: u32,
    pub identity: TranscriptPublicationIdentity,
    pub binding: Binding,
    pub result_sha256: String,
    pub transcript_id: String,
    pub stage_name: String,
    pub state: State,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum State {
    Prepared,
    Pending,
    Committed,
    Empty,
    Conflict,
    Review,
}

impl Receipt {
    pub fn validate(&self, expected: &TranscriptPublicationIdentity) -> Result<(), StoreError> {
        expected.validate()?;
        crate::validate_session_id(&self.binding.session_id)?;
        if self.schema_version != 1
            || &self.identity != expected
            || !valid_hash(&self.result_sha256)
            || !valid_hash(&self.binding.recording_sha256)
            || self.binding.transcript_revision != absent_revision()
            || !valid_text(&self.transcript_id, 512)
            || !matches!(
                self.binding.recording_selection.as_str(),
                "audio.mp3"
                    | "audio.wav"
                    | "audio.ogg"
                    | "audio/audio.mp3"
                    | "audio/audio.wav"
                    | "audio/audio.ogg"
            )
        {
            return Err(StoreError::Conflict(
                "Transcript publication receipt does not match this request".into(),
            ));
        }
        validate_stage(&self.stage_name)
    }
}

pub(super) fn absent_revision() -> String {
    crate::sha256(b"loofah/transcript-revision/v1/absent")
}

pub(super) fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

pub(super) fn paths(root: &Path, identity: &TranscriptPublicationIdentity) -> (PathBuf, PathBuf) {
    let namespace = serde_json::to_string(&(&identity.vault_id, &identity.namespace)).unwrap();
    let key = crate::operation_io::operation_key(&namespace, identity.request_id);
    (
        root.join(format!("transcript-publication-{key}.json")),
        root.join(format!("transcript-result-{key}.json")),
    )
}

pub(super) fn check_parents(path: &Path) -> Result<(), StoreError> {
    for parent in path.ancestors().skip(1) {
        match std::fs::symlink_metadata(parent) {
            Ok(metadata) if !metadata.is_dir() || metadata.file_type().is_symlink() => {
                return Err(StoreError::Io(
                    "Transcript operation directory is not regular".into(),
                ));
            }
            Ok(_) => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(error(cause)),
        }
    }
    Ok(())
}

pub(super) fn read(path: &Path, limit: usize) -> Result<Option<Vec<u8>>, StoreError> {
    check_parents(path)?;
    let Some(file) = open_regular_file(path)? else {
        return Ok(None);
    };
    if file.metadata().map_err(error)?.len() > limit as u64 {
        return Err(StoreError::ResourceLimit(
            "Transcript publication file exceeds its size limit".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() > limit {
        return Err(StoreError::ResourceLimit(
            "Transcript publication file grew beyond its size limit".into(),
        ));
    }
    Ok(Some(bytes))
}

pub(super) fn read_receipt(
    path: &Path,
    identity: &TranscriptPublicationIdentity,
) -> Result<Option<Receipt>, StoreError> {
    let Some(bytes) = read(path, RECEIPT_LIMIT)? else {
        return Ok(None);
    };
    let raw = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&bytes)
        .map_err(serialization)?
        .0;
    let receipt: Receipt = serde_json::from_value(raw).map_err(serialization)?;
    receipt.validate(identity)?;
    Ok(Some(receipt))
}

pub(super) fn persist(path: &Path, receipt: &Receipt) -> Result<(), StoreError> {
    receipt.validate(&receipt.identity)?;
    if serde_json::to_vec(receipt).map_err(serialization)?.len() > RECEIPT_LIMIT {
        return Err(StoreError::ResourceLimit(
            "Transcript publication receipt exceeds its size limit".into(),
        ));
    }
    check_parents(path)?;
    crate::operation_io::persist(path, receipt)
}

pub(super) fn save_result(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    if let Some(existing) = read(path, RESULT_LIMIT).map_err(StoreError::after_commit)? {
        if existing != bytes {
            return Err(StoreError::Conflict(
                "Transcript publication result changed".into(),
            ));
        }
        return Ok(());
    }
    check_parents(path)?;
    let parent = path
        .parent()
        .ok_or_else(|| StoreError::Io("Missing transcript result directory".into()))?;
    crate::operation_io::ensure_directory(parent)?;
    let temporary = parent.join(format!(".transcript-result-{}.tmp", uuid::Uuid::new_v4()));
    write_new(&temporary, bytes)?;
    hypr_storage::fs::rename_no_replace(&temporary, path).map_err(error)?;
    crate::operation_io::sync_parent(path)
}

pub(super) fn stage_name() -> String {
    format!(".transcript-publication-{}.tmp", uuid::Uuid::new_v4())
}

pub(super) fn validate_stage(name: &str) -> Result<(), StoreError> {
    if !name
        .strip_prefix(".transcript-publication-")
        .and_then(|value| value.strip_suffix(".tmp"))
        .is_some_and(|value| {
            uuid::Uuid::parse_str(value)
                .is_ok_and(|id| !id.is_nil() && id.hyphenated().to_string() == value)
        })
    {
        return Err(StoreError::Serialize(
            "Invalid transcript stage identity".into(),
        ));
    }
    Ok(())
}

pub(super) fn write_stage(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    validate_stage(
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default(),
    )?;
    write_new(path, bytes)
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    check_parents(path)?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(error)?;
    file.write_all(bytes).map_err(error)?;
    file.sync_all().map_err(error)?;
    crate::operation_io::sync_parent(path)
}

pub(super) fn sync_stage(path: &Path) -> Result<(), StoreError> {
    let before = snapshot(path)?
        .ok_or_else(|| StoreError::Conflict("Transcript stage disappeared".into()))?;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(error)?;
    let current = FileSnapshot::new(file)?;
    if !before.unchanged(&current)? {
        return Err(StoreError::Conflict(
            "Transcript stage changed before synchronization".into(),
        ));
    }
    current.file().sync_all().map_err(error)?;
    crate::operation_io::sync_parent(path)
}

pub(super) fn hash_bounded(file: &File, limit: u64) -> Result<String, StoreError> {
    check_source_size(file, limit)?;
    let mut reader = file;
    reader.rewind().map_err(error)?;
    let result = hypr_fs_sync_core::media::metadata_from_reader(
        "recording",
        "application/octet-stream",
        reader.take(limit.saturating_add(1)),
    )
    .map_err(error)?;
    if result.size_bytes > limit {
        return Err(StoreError::ResourceLimit(
            "Transcript source grew beyond its size limit".into(),
        ));
    }
    Ok(result.sha256)
}

pub(super) fn select(directory: &Path) -> Result<Option<(String, FileSnapshot)>, StoreError> {
    check_parents(&directory.join("transcript.json"))?;
    for (base, prefix) in [
        (directory.to_owned(), ""),
        (directory.join("audio"), "audio/"),
    ] {
        match std::fs::symlink_metadata(&base) {
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
            Err(cause) => return Err(error(cause)),
            Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                return Err(StoreError::Io("Recording directory is not regular".into()));
            }
            Ok(_) => {}
        }
        if let Some(opened) = crate::media::open_recording_in(&base)? {
            check_source_size(&opened.file, u32::MAX as u64)?;
            return Ok(Some((
                format!("{prefix}{}", opened.filename),
                FileSnapshot::new(opened.file)?,
            )));
        }
    }
    Ok(None)
}

pub(super) fn snapshot(path: &Path) -> Result<Option<FileSnapshot>, StoreError> {
    let Some(file) = open_regular_file(path)? else {
        return Ok(None);
    };
    if file.metadata().map_err(error)?.len() > RESULT_LIMIT as u64 {
        return Err(StoreError::ResourceLimit(
            "Transcript stage exceeds its size limit".into(),
        ));
    }
    FileSnapshot::new(file).map(Some)
}

pub(super) fn absent(path: &Path) -> Result<bool, StoreError> {
    match std::fs::symlink_metadata(path) {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(cause) => Err(error(cause)),
        Ok(_) => Ok(false),
    }
}

pub(super) fn serialization(cause: impl std::fmt::Display) -> StoreError {
    StoreError::Serialize(cause.to_string())
}
pub(super) fn error(cause: impl std::fmt::Display) -> StoreError {
    StoreError::Io(cause.to_string())
}

pub(super) fn clone_file(file: &File) -> Result<File, StoreError> {
    file.try_clone().map_err(error)
}

fn check_source_size(file: &File, limit: u64) -> Result<(), StoreError> {
    if file.metadata().map_err(error)?.len() > limit {
        return Err(StoreError::ResourceLimit(
            "Transcript source exceeds its size limit".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_byte_admission_accepts_exact_limit_and_rejects_growth_without_mutation() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("audio.wav");
        std::fs::write(&path, b"1234").unwrap();
        let file = File::open(&path).unwrap();
        assert_eq!(hash_bounded(&file, 4).unwrap(), crate::sha256(b"1234"));
        assert!(matches!(
            check_source_size(&file, 3),
            Err(StoreError::ResourceLimit(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"1234");
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"5")
            .unwrap();
        assert!(matches!(
            hash_bounded(&file, 4),
            Err(StoreError::ResourceLimit(_))
        ));
        assert_eq!(std::fs::read(&path).unwrap(), b"12345");
    }
}
