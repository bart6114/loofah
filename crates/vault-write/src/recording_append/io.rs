use super::types::*;
use crate::{StoreError, file_snapshot::FileSnapshot, media::open_regular_file};
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, Write},
    path::{Path, PathBuf},
};

pub(super) const TRANSCRIPT_LIMIT: u64 = 64 * 1024 * 1024;
pub(super) const METADATA_LIMIT: u64 = 8 * 1024 * 1024;
pub(super) const AUDIO_LIMIT: u64 = u32::MAX as u64;
pub(super) const RECORDINGS: [&str; 6] = [
    "audio.mp3",
    "audio.wav",
    "audio.ogg",
    "audio/audio.mp3",
    "audio/audio.wav",
    "audio/audio.ogg",
];
pub(super) fn invalid(message: &str) -> StoreError {
    StoreError::Conflict(message.into())
}
pub(super) fn error(error: impl std::fmt::Display) -> StoreError {
    StoreError::Io(error.to_string())
}
pub(super) fn absent_revision() -> String {
    crate::sha256(b"loofah/transcript-revision/v1/absent")
}
pub(super) fn revision(bytes: Option<&[u8]>) -> String {
    bytes.map(crate::sha256).unwrap_or_else(absent_revision)
}
pub(super) fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
pub(super) fn check(path: &Path) -> Result<(), StoreError> {
    for parent in path.ancestors().skip(1) {
        match std::fs::symlink_metadata(parent) {
            Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                return Err(invalid("Append directory is not regular"));
            }
            Ok(_) => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(error(cause)),
        }
    }
    Ok(())
}
pub(super) fn open(path: &Path) -> Result<Option<File>, StoreError> {
    check(path)?;
    open_regular_file(path)
}
pub(super) fn read(path: &Path, limit: u64) -> Result<Option<Vec<u8>>, StoreError> {
    let Some(file) = open(path)? else {
        return Ok(None);
    };
    if file.metadata().map_err(error)?.len() > limit {
        return Err(invalid("Append artifact exceeds its byte budget"));
    }
    let mut bytes = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(error)?;
    if bytes.len() as u64 > limit {
        return Err(invalid("Append artifact grew beyond its byte budget"));
    }
    Ok(Some(bytes))
}
pub(super) fn json<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, StoreError> {
    read(path, 64 * 1024)?
        .map(|bytes| {
            let value = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&bytes)
                .map_err(error)?
                .0;
            serde_json::from_value(value).map_err(error)
        })
        .transpose()
}
pub(super) fn persist(path: &Path, value: &impl Serialize) -> Result<(), StoreError> {
    check(path)?;
    crate::operation_io::persist(path, value)
}
pub(super) fn private_directory(path: &Path) -> Result<(), StoreError> {
    check(&path.join("probe"))?;
    crate::operation_io::ensure_directory(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).map_err(error)?;
    }
    Ok(())
}
pub(super) fn root(operations: &Path, identity: &AppendIdentity) -> Result<PathBuf, StoreError> {
    identity.validate()?;
    if !operations.is_absolute() {
        return Err(invalid("Append operations path must be absolute"));
    }
    let namespace =
        serde_json::to_string(&(&identity.server, &identity.vault_id, &identity.member_id))
            .map_err(error)?;
    let path = operations
        .join("recording-append")
        .join(crate::operation_io::operation_key(
            &namespace,
            identity.admission_id,
        ));
    check(&path.join("source.json"))?;
    Ok(path)
}
pub(super) fn hash(file: &File, limit: u64) -> Result<(String, u64), StoreError> {
    let before = FileSnapshot::new(file.try_clone().map_err(error)?)?;
    let mut reader = file;
    reader.rewind().map_err(error)?;
    let mut digest = Sha256::new();
    let mut length = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let count = reader.read(&mut buffer).map_err(error)?;
        if count == 0 {
            break;
        }
        length += count as u64;
        if length > limit {
            return Err(invalid("Append audio exceeds its byte budget"));
        }
        digest.update(&buffer[..count]);
    }
    let after = FileSnapshot::new(file.try_clone().map_err(error)?)?;
    if !before.unchanged(&after)? {
        return Err(invalid("Append audio changed while hashing"));
    }
    reader.rewind().map_err(error)?;
    Ok((format!("{:x}", digest.finalize()), length))
}
pub(super) fn copy(
    path: &Path,
    mut source: impl Read,
    expected: &str,
    length: u64,
) -> Result<(), StoreError> {
    if let Some(existing) = open(path)? {
        if hash(&existing, length)? == (expected.into(), length) {
            return Ok(());
        }
        return Err(invalid("Retained append artifact changed"));
    }
    check(path)?;
    let temp = path.with_file_name(format!(".append-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut output = options.open(&temp).map_err(error)?;
    let result = (|| {
        let mut digest = Sha256::new();
        let mut total = 0;
        let mut buffer = [0; 64 * 1024];
        loop {
            let count = source.read(&mut buffer).map_err(error)?;
            if count == 0 {
                break;
            }
            total += count as u64;
            if total > length {
                return Err(invalid("Append source exceeds declared length"));
            }
            output.write_all(&buffer[..count]).map_err(error)?;
            digest.update(&buffer[..count]);
        }
        if total != length || format!("{:x}", digest.finalize()) != expected {
            return Err(invalid("Append source does not match declared bytes"));
        }
        output.sync_all().map_err(error)
    })();
    drop(output);
    if let Err(cause) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(cause);
    }
    hypr_storage::fs::rename_no_replace(&temp, path).map_err(error)?;
    crate::operation_io::sync_parent(path)
}
pub(super) fn retain_bytes(path: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    copy(
        path,
        std::io::Cursor::new(bytes),
        &crate::sha256(bytes),
        bytes.len() as u64,
    )
}
pub(super) fn select(directory: &Path) -> Result<Option<(String, FileSnapshot)>, StoreError> {
    let mut selected = None;
    for name in RECORDINGS {
        if let Some(file) = open(&directory.join(name))? {
            if selected.is_some() {
                return Err(invalid(
                    "Multiple recordings require review before appending",
                ));
            }
            selected = Some((name.into(), FileSnapshot::new(file)?));
        }
    }
    Ok(selected)
}
pub(super) fn validate_source(
    receipt: &SourceReceipt,
    identity: &AppendIdentity,
    session: Option<&str>,
) -> Result<(), StoreError> {
    identity.validate()?;
    crate::validate_session_id(&receipt.session_id)?;
    if receipt.schema_version != 1
        || &receipt.identity != identity
        || session.is_some_and(|id| id != receipt.session_id)
        || !RECORDINGS.contains(&receipt.selection.as_str())
        || !valid_hash(&receipt.recording_sha256)
        || !valid_hash(&receipt.transcript_revision)
        || receipt.recording_bytes == 0
        || receipt.recording_bytes > AUDIO_LIMIT
        || (!receipt.transcript_present && receipt.transcript_revision != absent_revision())
    {
        return Err(invalid("Append source receipt does not match its owner"));
    }
    Ok(())
}
pub(super) fn validate_result(result: &ResultReceipt) -> Result<(), StoreError> {
    validate_source(&result.source, &result.source.identity, None)?;
    if result.schema_version != 1
        || !valid_hash(&result.artifact.sha256)
        || result.artifact.size_bytes == 0
        || result.artifact.size_bytes > AUDIO_LIMIT
        || result.artifact.frames == 0
        || !valid_hash(&result.transcript_revision)
        || (!result.transcript_present && result.transcript_revision != absent_revision())
        || (result.source.transcript_present && !result.transcript_present)
    {
        return Err(invalid("Invalid append result receipt"));
    }
    Ok(())
}
pub(super) fn wave(file: File, artifact: &RecordingAppendArtifact) -> Result<(), StoreError> {
    if hash(&file, AUDIO_LIMIT)? != (artifact.sha256.clone(), artifact.size_bytes) {
        return Err(invalid("Append WAV bytes do not match"));
    }
    let mut wav = hound::WavReader::new(file).map_err(error)?;
    let spec = wav.spec();
    let channels = if artifact.layout == hypr_vault_read::AudioLayout::MicSystem {
        2
    } else {
        1
    };
    if spec.sample_rate != 16000
        || spec.channels != channels
        || spec.bits_per_sample != 32
        || spec.sample_format != hound::SampleFormat::Float
        || u64::from(wav.duration()) != artifact.frames
    {
        return Err(invalid("Append WAV format or frame count does not match"));
    }
    for sample in wav.samples::<f32>() {
        if !sample.map_err(error)?.is_finite() {
            return Err(invalid("Append WAV contains nonfinite samples"));
        }
    }
    Ok(())
}

pub(super) fn sync_file(path: &Path) -> Result<(), StoreError> {
    if open(path)?.is_none() {
        return Err(invalid(
            "Append artifact disappeared before synchronization",
        ));
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(error)
}
