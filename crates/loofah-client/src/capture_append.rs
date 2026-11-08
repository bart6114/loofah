use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use hypr_loofah_protocol::{
    CaptureAppendAdmission, CaptureAppendRequest, CaptureScope, ErrorCode,
    validate_capture_append_admission, validate_capture_append_request,
};
use sha2::{Digest, Sha256};

use crate::{Result, failure};

/// Private durable identity created before the first append preparation request.
/// Loading it never replaces the original declaration with the current session.
#[derive(Clone)]
pub struct CaptureAppendPreparation {
    request: CaptureAppendRequest,
    directory: PathBuf,
}

impl CaptureAppendPreparation {
    pub async fn prepare(root: PathBuf, request: CaptureAppendRequest) -> Result<Self> {
        validate_capture_append_request(&request)
            .map_err(|message| failure(ErrorCode::Validation, message))?;
        if !root.is_absolute()
            || root
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(failure(
                ErrorCode::Validation,
                "Append recovery storage must be absolute",
            ));
        }
        tokio::task::spawn_blocking(move || {
            let hash = Sha256::digest(serde_json::to_vec(&request.scope).map_err(io)?)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let directory = root.join(hash).join(&request.admission_id);
            ensure_directory(&directory)?;
            immutable(&directory.join("request.json"), &request)?;
            Ok(Self { request, directory })
        })
        .await
        .map_err(io)?
    }

    /// Discovers original requests without making a network request or changing their authority.
    pub async fn inventory(
        root: PathBuf,
        scope: CaptureScope,
    ) -> Result<Vec<CaptureAppendRequest>> {
        if !root.is_absolute()
            || root
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(damaged());
        }
        tokio::task::spawn_blocking(move || {
            let hash = Sha256::digest(serde_json::to_vec(&scope).map_err(io)?)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let directory = root.join(hash);
            match std::fs::symlink_metadata(&directory) {
                Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                Err(cause) => return Err(io(cause)),
                Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
                    return Err(damaged());
                }
                Ok(_) => {}
            }
            ensure_directory(&directory)?;
            let mut requests = Vec::new();
            let mut bytes = 0u64;
            for entry in std::fs::read_dir(&directory).map_err(io)? {
                if requests.len() >= 4096 {
                    return Err(damaged());
                }
                let entry = entry.map_err(io)?;
                let name = entry.file_name().into_string().map_err(|_| damaged())?;
                if uuid::Uuid::parse_str(&name)
                    .ok()
                    .filter(|id| !id.is_nil() && id.to_string() == name)
                    .is_none()
                    || !entry.file_type().map_err(io)?.is_dir()
                {
                    return Err(damaged());
                }
                let path = entry.path().join("request.json");
                bytes = bytes
                    .checked_add(std::fs::symlink_metadata(&path).map_err(io)?.len())
                    .ok_or_else(damaged)?;
                if bytes > 64 * 1024 * 1024 {
                    return Err(damaged());
                }
                let raw: serde_json::Value = read(&path)?.ok_or_else(damaged)?;
                let request: CaptureAppendRequest =
                    serde_json::from_value(raw.clone()).map_err(|_| damaged())?;
                validate_capture_append_request(&request).map_err(|_| damaged())?;
                if request.scope != scope
                    || request.admission_id != name
                    || serde_json::to_value(&request).map_err(io)? != raw
                {
                    return Err(damaged());
                }
                let prepared = Self {
                    request: request.clone(),
                    directory: entry.path(),
                };
                if let Some(admission) =
                    read::<CaptureAppendAdmission>(&entry.path().join("admission.json"))?
                {
                    prepared.validate(&admission)?;
                }
                requests.push(request);
            }
            requests.sort_by(|a, b| a.admission_id.cmp(&b.admission_id));
            Ok(requests)
        })
        .await
        .map_err(io)?
    }

    pub fn request(&self) -> &CaptureAppendRequest {
        &self.request
    }

    pub async fn admission(&self) -> Result<Option<CaptureAppendAdmission>> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            let value: Option<CaptureAppendAdmission> =
                read(&this.directory.join("admission.json"))?;
            if let Some(value) = &value {
                this.validate(value)?;
            }
            Ok(value)
        })
        .await
        .map_err(io)?
    }

    pub(crate) async fn observe(
        &self,
        value: CaptureAppendAdmission,
    ) -> Result<CaptureAppendAdmission> {
        self.validate(&value)?;
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            immutable(&this.directory.join("admission.json"), &value)?;
            Ok(value)
        })
        .await
        .map_err(io)?
    }

    fn validate(&self, value: &CaptureAppendAdmission) -> Result<()> {
        validate_capture_append_admission(value).map_err(|_| damaged())?;
        if value.request != self.request {
            return Err(damaged());
        }
        Ok(())
    }
}

fn ensure_directory(path: &Path) -> Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {
            if let Some(parent) = path.parent() {
                ensure_directory(parent)?;
            }
            Ok(())
        }
        Ok(_) => Err(damaged()),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
            let parent = path.parent().ok_or_else(damaged)?;
            ensure_directory(parent)?;
            let mut builder = std::fs::DirBuilder::new();
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                builder.mode(0o700);
            }
            match builder.create(path) {
                Ok(()) => {}
                Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {
                    ensure_directory(path)?
                }
                Err(cause) => return Err(io(cause)),
            }
            sync_directory(parent)
        }
        Err(cause) => Err(io(cause)),
    }
}
fn immutable<T: serde::Serialize + serde::de::DeserializeOwned + PartialEq>(
    path: &Path,
    value: &T,
) -> Result<()> {
    ensure_directory(path.parent().ok_or_else(damaged)?)?;
    let bytes = serde_json::to_vec(value).map_err(io)?;
    if bytes.len() > 64 * 1024 {
        return Err(damaged());
    }
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(&bytes).map_err(io)?;
            file.sync_all().map_err(io)?;
        }
        Err(cause) if cause.kind() == std::io::ErrorKind::AlreadyExists => {
            if read::<T>(path)?.as_ref() != Some(value) {
                return Err(damaged());
            }
            File::open(path).map_err(io)?.sync_all().map_err(io)?;
        }
        Err(cause) => return Err(io(cause)),
    }
    sync_directory(path.parent().ok_or_else(damaged)?)
}
fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    ensure_directory(path.parent().ok_or_else(damaged)?)?;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => return Err(io(cause)),
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 64 * 1024 {
        return Err(damaged());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io)?
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > 64 * 1024 {
        return Err(damaged());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| damaged())
}
fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path).map_err(io)?.sync_all().map_err(io)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn damaged() -> hypr_loofah_protocol::ApplicationError {
    failure(
        ErrorCode::Conflict,
        "Append recovery identity is invalid or changed; retain it for review",
    )
}
fn io(cause: impl std::fmt::Display) -> hypr_loofah_protocol::ApplicationError {
    failure(
        ErrorCode::Unavailable,
        &format!("Append recovery storage failed: {cause}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request() -> CaptureAppendRequest {
        CaptureAppendRequest {
            scope: hypr_loofah_protocol::CaptureScope {
                server: "https://example.test/".into(),
                member_id: "member".into(),
                vault_id: "vault".into(),
            },
            session_id: "note".into(),
            capture_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            admission_id: uuid::Uuid::new_v4().to_string(),
            processing: None,
        }
    }
    #[tokio::test]
    async fn inventory_recovers_original_scope_and_refuses_unknown_or_partial_identity() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let input = request();
        let prepared = CaptureAppendPreparation::prepare(root.clone(), input.clone())
            .await
            .unwrap();
        assert_eq!(
            CaptureAppendPreparation::inventory(root.clone(), input.scope.clone())
                .await
                .unwrap(),
            vec![input.clone()]
        );
        let mut other = input.scope.clone();
        other.member_id = "other".into();
        assert!(
            CaptureAppendPreparation::inventory(root.clone(), other)
                .await
                .unwrap()
                .is_empty()
        );
        let mut unknown = serde_json::to_value(&input).unwrap();
        unknown["future_authority"] = serde_json::json!(true);
        std::fs::write(
            prepared.directory.join("request.json"),
            serde_json::to_vec(&unknown).unwrap(),
        )
        .unwrap();
        assert!(
            CaptureAppendPreparation::inventory(root.clone(), input.scope.clone())
                .await
                .is_err()
        );
        std::fs::write(prepared.directory.join("request.json"), b"{").unwrap();
        assert!(
            CaptureAppendPreparation::inventory(root, input.scope)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn identity_is_durable_before_dispatch_and_never_retargets_on_retry() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let input = request();
        let prepared = CaptureAppendPreparation::prepare(root.clone(), input.clone())
            .await
            .unwrap();
        assert!(prepared.admission().await.unwrap().is_none());
        let restored = CaptureAppendPreparation::prepare(root.clone(), input.clone())
            .await
            .unwrap();
        assert_eq!(restored.request(), &input);
        let mut changed = input;
        changed.session_id = "other".into();
        assert!(
            CaptureAppendPreparation::prepare(root, changed)
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn original_admission_survives_reopen_and_changed_response_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let input = request();
        let prepared = CaptureAppendPreparation::prepare(root.clone(), input.clone())
            .await
            .unwrap();
        let admission = CaptureAppendAdmission {
            request: input.clone(),
            baseline: hypr_loofah_protocol::CaptureAppendBaseline {
                recording_selection: "audio.wav".into(),
                recording_sha256: "1".repeat(64),
                transcript_present: true,
                transcript_revision: "2".repeat(64),
                normalized_sha256: "3".repeat(64),
                normalized_bytes: 8044,
                frames: 1000,
                format: hypr_loofah_protocol::CaptureFormat::stereo(),
            },
        };
        prepared.observe(admission.clone()).await.unwrap();
        let restored = CaptureAppendPreparation::prepare(root, input)
            .await
            .unwrap();
        assert_eq!(restored.admission().await.unwrap(), Some(admission.clone()));
        let mut changed = admission;
        changed.baseline.frames += 1;
        assert!(restored.observe(changed).await.is_err());
    }
    #[tokio::test]
    async fn partial_request_is_never_overwritten_to_acquire_new_authority() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let input = request();
        let prepared = CaptureAppendPreparation::prepare(root.clone(), input.clone())
            .await
            .unwrap();
        std::fs::write(prepared.directory.join("request.json"), b"{").unwrap();
        assert!(
            CaptureAppendPreparation::prepare(root, input)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(prepared.directory.join("request.json")).unwrap(),
            b"{"
        );
    }
}
