use super::RecordingPublicationRequest;
use crate::{SessionStore, StoreError, file_snapshot::FileSnapshot, media::open_regular_file};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Receipt {
    pub schema_version: u32,
    pub request: RecordingPublicationRequest,
    pub stage_name: String,
    pub before_metadata_sha256: String,
    pub after_metadata_sha256: String,
    pub published_started_at: String,
    pub published_ended_at: Option<String>,
    pub state: State,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum State {
    Pending,
    MetadataWritten,
    Committed,
    Uncertain,
}
impl Receipt {
    pub fn validate(&self, request: &RecordingPublicationRequest) -> Result<(), StoreError> {
        if self.schema_version != 1
            || &self.request != request
            || self.published_started_at.is_empty()
            || self.published_started_at.len() > 128
            || self
                .published_ended_at
                .as_ref()
                .is_some_and(|value| value.len() > 128)
            || !valid_hash(&self.before_metadata_sha256)
            || !valid_hash(&self.after_metadata_sha256)
        {
            return Err(StoreError::Conflict(
                "Recording publication receipt does not match this request".into(),
            ));
        }
        validate_stage_name(&self.stage_name)?;
        self.request.validate()
    }
}
pub(super) fn receipt_path(root: &Path, request: &RecordingPublicationRequest) -> PathBuf {
    let namespace = serde_json::to_string(&(&request.vault_id, &request.namespace)).unwrap();
    root.join(format!(
        "recording-publication-{}.json",
        crate::operation_io::operation_key(&namespace, request.request_id)
    ))
}
pub(super) struct Stage {
    path: PathBuf,
    retained: bool,
}
impl Stage {
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn name(&self) -> Result<&str, StoreError> {
        self.path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| StoreError::Io("Invalid recording stage name".into()))
    }
    pub fn retain(&mut self) {
        self.retained = true;
    }
    pub fn from_receipt(directory: &Path, receipt: &Receipt) -> Result<Self, StoreError> {
        validate_stage_name(&receipt.stage_name)?;
        Ok(Self {
            path: directory.join(&receipt.stage_name),
            retained: true,
        })
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if !self.retained
            && let Err(cause) = std::fs::remove_file(&self.path)
            && cause.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%cause,"Cannot remove unpublished recording stage");
        }
    }
}
fn validate_stage_name(name: &str) -> Result<(), StoreError> {
    let uuid = name
        .strip_prefix(".recording-publication-")
        .and_then(|v| v.strip_suffix(".tmp"));
    if !uuid.is_some_and(|part| {
        uuid::Uuid::parse_str(part)
            .is_ok_and(|id| !id.is_nil() && id.hyphenated().to_string() == part)
    }) {
        return Err(StoreError::Serialize(
            "Invalid recording stage identity".into(),
        ));
    }
    Ok(())
}
pub(super) fn read(path: &Path) -> Result<Option<Receipt>, StoreError> {
    read_receipt_bytes(path, 64 * 1024)?
        .map(|bytes| parse_receipt(&bytes))
        .transpose()
}
pub(super) fn read_receipt_bytes(path: &Path, budget: u64) -> Result<Option<Vec<u8>>, StoreError> {
    check_parents(path)?;
    let Some(file) = open_regular_file(path)? else {
        return Ok(None);
    };
    let length = file.metadata().map_err(io_error)?.len();
    if length > 64 * 1024 {
        return Err(StoreError::Serialize(
            "Recording receipt exceeds its size limit".into(),
        ));
    }
    let allowance = budget.min(64 * 1024);
    if length > allowance {
        return Err(StoreError::ResourceLimit(
            "Recording receipt exceeds remaining scan budget".into(),
        ));
    }
    let mut bytes = Vec::new();
    file.take(allowance + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() as u64 > allowance {
        return Err(StoreError::ResourceLimit(
            "Recording receipt changed beyond its read budget".into(),
        ));
    }
    Ok(Some(bytes))
}
pub(super) fn parse_receipt(bytes: &[u8]) -> Result<Receipt, StoreError> {
    let value = serde_json::from_slice::<crate::metadata_revision::UniqueValue>(bytes)
        .map_err(serialize_error)?
        .0;
    serde_json::from_value(value).map_err(serialize_error)
}
pub(super) fn persist(path: &Path, receipt: &Receipt) -> Result<(), StoreError> {
    check_parents(path)?;
    crate::operation_io::persist(path, receipt)
}
pub(super) fn check_parents(path: &Path) -> Result<(), StoreError> {
    for parent in path.ancestors().skip(1) {
        match std::fs::symlink_metadata(parent) {
            Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                return Err(StoreError::Io(
                    "Recording operation directory is not regular".into(),
                ));
            }
            Ok(_) => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(io_error(cause)),
        }
    }
    Ok(())
}
pub(super) fn regular_directory(path: &Path) -> Result<(), StoreError> {
    let meta = std::fs::symlink_metadata(path).map_err(io_error)?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(StoreError::Io("Recording directory is not regular".into()));
    }
    Ok(())
}
pub(super) fn open_snapshot(path: &Path) -> Result<Option<FileSnapshot>, StoreError> {
    open_regular_file(path)?.map(FileSnapshot::new).transpose()
}
pub(super) fn absent(directory: &Path, allowed: Option<&str>) -> Result<bool, StoreError> {
    regular_directory(directory)?;
    for base in [directory.to_path_buf(), directory.join("audio")] {
        match std::fs::symlink_metadata(&base) {
            Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                return Err(StoreError::Io("Recording directory is not regular".into()));
            }
            Ok(_) => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => continue,
            Err(cause) => return Err(io_error(cause)),
        }
        for name in ["audio.peaks.json", "audio.peaks.json.tmp"] {
            match std::fs::symlink_metadata(base.join(name)) {
                Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                    return Err(StoreError::Io("Recording peaks are not regular".into()));
                }
                Ok(_) => {}
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(cause) => return Err(io_error(cause)),
            }
        }
        for name in ["audio.mp3", "audio.wav", "audio.ogg"].into_iter().chain(
            hypr_vault_read::reserved::SESSION_TRANSIENT_FILES
                .into_iter()
                .filter(|name| *name != "audio.peaks.json.tmp"),
        ) {
            match std::fs::symlink_metadata(base.join(name)) {
                Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                    return Err(StoreError::Io("Recording artifact is not regular".into()));
                }
                Ok(_) if base == directory && Some(name) == allowed => {}
                Ok(_) => return Ok(false),
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(cause) => return Err(io_error(cause)),
            }
        }
    }
    Ok(true)
}
pub(super) fn write_stage(
    directory: &Path,
    source: Box<dyn Read + Send>,
    request: &RecordingPublicationRequest,
) -> Result<Stage, StoreError> {
    let (stage, mut output) = loop {
        let path = directory.join(format!(
            ".recording-publication-{}.tmp",
            uuid::Uuid::new_v4()
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(file) => {
                break (
                    Stage {
                        path,
                        retained: false,
                    },
                    file,
                );
            }
            Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(cause) => return Err(io_error(cause)),
        }
    };
    let result = write_source(&mut output, source, request);
    drop(output);
    result?;
    sync_parent(stage.path())?;
    Ok(stage)
}
fn write_source(
    output: &mut std::fs::File,
    source: Box<dyn Read + Send>,
    request: &RecordingPublicationRequest,
) -> Result<(), StoreError> {
    let mut input = source.take(request.artifact_size_bytes + 1);
    let mut hash = Sha256::new();
    let mut total = 0;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = match input.read(&mut buffer) {
            Ok(count) => count,
            Err(cause) if cause.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(cause) => return Err(io_error(cause)),
        };
        if count == 0 {
            break;
        }
        total += count as u64;
        if total > request.artifact_size_bytes {
            return Err(StoreError::Conflict(
                "Recording source exceeds declared length".into(),
            ));
        }
        output.write_all(&buffer[..count]).map_err(io_error)?;
        hash.update(&buffer[..count]);
    }
    if total != request.artifact_size_bytes
        || hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
            != request.artifact_sha256
    {
        return Err(StoreError::Conflict(
            "Recording source does not match declared artifact".into(),
        ));
    }
    output.sync_all().map_err(io_error)?;
    Ok(())
}
pub(super) fn matches(
    file: &FileSnapshot,
    request: &RecordingPublicationRequest,
) -> Result<bool, StoreError> {
    Ok(
        file.file().metadata().map_err(io_error)?.len() == request.artifact_size_bytes
            && file.hash()? == request.artifact_sha256,
    )
}
pub(super) fn busy(store: &SessionStore, id: &str, raw: &Value) -> bool {
    store.is_recording(id) || raw.get("audio_import_pending").and_then(Value::as_bool) == Some(true)
}
pub(super) fn patch_metadata(
    mut raw: Value,
    request: &RecordingPublicationRequest,
) -> Result<Value, StoreError> {
    let object = raw
        .as_object_mut()
        .ok_or_else(|| StoreError::Serialize("Session metadata must be an object".into()))?;
    let audio = object
        .entry("audio")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| StoreError::Conflict("Session audio metadata is not an object".into()))?;
    audio.insert("source".into(), serde_json::json!("recording"));
    audio.insert(
        "layout".into(),
        serde_json::to_value(request.layout).map_err(serialize_error)?,
    );
    if object.get("started_at").is_none_or(Value::is_null) {
        object.insert(
            "started_at".into(),
            Value::String(timestamp(request.first_audio_observed_at_ms)?),
        );
    }
    if let Some(end) = request.capture_ended_at_ms {
        object.insert("ended_at".into(), Value::String(timestamp(end)?));
    }
    Ok(raw)
}
pub(super) fn owned_metadata_matches(raw: &Value, receipt: &Receipt) -> Result<bool, StoreError> {
    Ok(
        raw.get("started_at").and_then(Value::as_str)
            == Some(receipt.published_started_at.as_str())
            && raw.get("ended_at").and_then(Value::as_str) == receipt.published_ended_at.as_deref()
            && patch_metadata(raw.clone(), &receipt.request)? == *raw,
    )
}
pub(super) fn timestamp(ms: u64) -> Result<String, StoreError> {
    if ms > 9_007_199_254_740_991 {
        return Err(StoreError::Serialize(
            "Recording observation exceeds safe UTC milliseconds".into(),
        ));
    }
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .map(|v| v.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or_else(|| StoreError::Serialize("Invalid recording observation timestamp".into()))
}
pub(super) fn json_hash(value: &Value) -> Result<String, StoreError> {
    Ok(crate::sha256(
        &serde_json::to_vec(value).map_err(serialize_error)?,
    ))
}
pub(super) fn valid_hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn invalidate_peaks(directory: &Path) -> Result<(), StoreError> {
    for base in [directory.to_path_buf(), directory.join("audio")] {
        for name in [
            hypr_vault_read::reserved::SESSION_PEAKS_FILE,
            "audio.peaks.json.tmp",
        ] {
            let path = base.join(name);
            match std::fs::symlink_metadata(&path) {
                Ok(meta) if !meta.is_file() || meta.file_type().is_symlink() => {
                    return Err(StoreError::Io("Recording peaks are not regular".into()));
                }
                Ok(_) => {
                    std::fs::remove_file(&path).map_err(io_error)?;
                    sync_parent(&path)?;
                }
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
                Err(cause) => return Err(io_error(cause)),
            }
        }
    }
    Ok(())
}
pub(super) fn read_metadata(directory: &Path, limit: usize) -> Result<Option<Value>, StoreError> {
    check_parents(&directory.join("_meta.json"))?;
    let Some(file) = open_regular_file(&directory.join("_meta.json"))? else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1) as u64)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    if bytes.len() > limit {
        return Err(StoreError::ResourceLimit(
            "Session metadata exceeds its size limit".into(),
        ));
    }
    serde_json::from_slice::<crate::metadata_revision::UniqueValue>(&bytes)
        .map(|value| Some(value.0))
        .map_err(serialize_error)
}
pub(super) fn sync_file(path: &Path) -> Result<(), StoreError> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .and_then(|file| file.sync_all())
        .map_err(io_error)
}
pub(super) fn sync_parent(path: &Path) -> Result<(), StoreError> {
    crate::operation_io::sync_parent(path)
}
pub(super) fn io_error(cause: std::io::Error) -> StoreError {
    StoreError::Io(cause.to_string())
}
pub(super) fn serialize_error(cause: serde_json::Error) -> StoreError {
    StoreError::Serialize(cause.to_string())
}
pub(super) fn worker_error(cause: tokio::task::JoinError) -> StoreError {
    StoreError::Io(format!("Recording publication worker stopped: {cause}"))
}

pub(super) fn transcript_absent(directory: &Path) -> Result<bool, StoreError> {
    match std::fs::symlink_metadata(directory.join("transcript.json")) {
        Ok(_) => Ok(false),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(cause) => Err(io_error(cause)),
    }
}
