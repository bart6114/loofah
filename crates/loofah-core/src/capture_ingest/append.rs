#[path = "append/publication.rs"]
pub(super) mod publication;
#[cfg(test)]
#[path = "append/tests.rs"]
mod tests;
use std::{
    fs::{File, OpenOptions},
    future::Future,
    io::{Read, Write},
    path::Path,
    sync::Arc,
};

use hypr_loofah_protocol::{
    ApplicationError, CaptureAppendAdmission, CaptureAppendBaseline, CaptureAppendRequest,
    CaptureIdentity, CaptureTarget, ErrorCode,
};
use hypr_vault_write::recording_append::{AppendIdentity, AppendSource};

use super::{VaultContext, rejected};
use crate::vault::storage_error;

impl VaultContext {
    pub(crate) async fn capture_append_source(
        self: &Arc<Self>,
        capture: &CaptureIdentity,
        check_current: bool,
    ) -> Result<AppendSource, ApplicationError> {
        checked_source(self, capture, check_current).await
    }

    pub async fn prepare_capture_append_authorized<G, F>(
        self: &Arc<Self>,
        member: String,
        input: CaptureAppendRequest,
        authorize: impl Fn() -> F + Send + 'static,
    ) -> Result<CaptureAppendAdmission, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        hypr_loofah_protocol::validate_capture_append_request(&input)
            .map_err(|message| rejected(ErrorCode::Validation, message))?;
        self.validate(&input.session_id)?;
        if input.processing.is_some()
            && (self.capture_processing.get().is_none() || self.transcription.get().is_none())
        {
            return Err(rejected(
                ErrorCode::Unavailable,
                "Append processing engines are not configured",
            ));
        }
        let settings = self.capture_ingestion.get().ok_or_else(unavailable)?;
        if self.lease.is_none()
            || input.scope.server != settings.server
            || input.scope.member_id != member
            || input.scope.vault_id != self.events.vault_id()
        {
            return Err(rejected(
                ErrorCode::Validation,
                "Append identity does not match the authenticated original owner",
            ));
        }
        let permit = settings
            .operations
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable())?;
        let root = self
            .operations
            .as_ref()
            .ok_or_else(unavailable)?
            .root
            .join("recording-appends");
        let request = self.admit_session_lifecycle_request(
            "capture-append",
            &member,
            uuid::Uuid::parse_str(&input.admission_id).map_err(|_| unavailable())?,
        )?;
        let context = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let _request = request;
            let grant = authorize().await?;
            let _session = context.try_session_lifecycle_change(&input.session_id)?;
            context.ensure_recording_publication_ready(&input.session_id)?;
            let source = context
                .store
                .prepare_recording_append_source(&root, identity(&input)?, &input.session_id)
                .await
                .map_err(|cause| storage_error(cause, false))?;
            let retained = source.clone();
            let requested = input.clone();
            let pin = context.clone();
            let existing = tokio::task::spawn_blocking(move || {
                let _pin = pin;
                let existing = read_admission(retained.directory())?;
                if let Some(value) = &existing {
                    validate_source(value, &requested, &retained)?;
                }
                Ok::<_, ApplicationError>(existing)
            })
            .await
            .map_err(io)??;
            if let Some(existing) = existing {
                return Ok(existing);
            }
            context.ensure_session_jobs_idle(&input.session_id).await?;
            context
                .ensure_capture_session_lifecycle_idle(&input.session_id)
                .await?;
            let directory = source.directory().to_owned();
            let retained = source.clone();
            let requested = input.clone();
            drop(grant);
            let pin = context.clone();
            let admission = tokio::task::spawn_blocking(move || {
                let _pin = pin;
                let suffix = uuid::Uuid::new_v4().to_string();
                let decoded = directory.join(format!("core-decoded-{suffix}.wav"));
                let normalized = directory.join(format!("core-normalized-{suffix}.wav"));
                let audio = crate::capture_append::normalize_baseline(
                    retained.recording_file().try_clone().map_err(io)?,
                    retained.recording_sha256(),
                    retained.audio().layout,
                    &decoded,
                    &normalized,
                )?;
                let canonical = directory.join("core-normalized.wav");
                // The only replacement permitted here is our private, incomplete
                // pre-admission normalization. No returned admission can exist yet.
                std::fs::rename(&normalized, &canonical).map_err(io)?;
                let _ = std::fs::remove_file(decoded);
                sync(&directory)?;
                let admission = CaptureAppendAdmission {
                    request: requested,
                    baseline: CaptureAppendBaseline {
                        recording_selection: retained.recording_selection().into(),
                        recording_sha256: retained.recording_sha256().into(),
                        transcript_present: retained.raw_transcript().is_some(),
                        transcript_revision: retained.transcript_revision().into(),
                        normalized_sha256: audio.sha256,
                        normalized_bytes: audio.bytes,
                        frames: audio.frames,
                        format: audio.format,
                    },
                };
                hypr_loofah_protocol::validate_capture_append_admission(&admission)
                    .map_err(|message| rejected(ErrorCode::Conflict, message))?;
                Ok(admission)
            })
            .await
            .map_err(io)??;
            let _grant = authorize().await?;
            if !source
                .recheck()
                .await
                .map_err(|cause| storage_error(cause, false))?
            {
                return Err(rejected(
                    ErrorCode::Conflict,
                    "Append source changed during preparation",
                ));
            }
            let pin = context.clone();
            let retained = source.clone();
            tokio::task::spawn_blocking(move || {
                let _pin = pin;
                persist_admission(retained.directory(), &admission)?;
                Ok(admission)
            })
            .await
            .map_err(io)?
        })
        .await
        .map_err(io)?
    }
}

pub(super) async fn checked_source(
    context: &Arc<VaultContext>,
    capture: &CaptureIdentity,
    check_current: bool,
) -> Result<AppendSource, ApplicationError> {
    let CaptureTarget::AppendSession {
        session_id,
        admission,
    } = &capture.target
    else {
        return Err(unavailable());
    };
    hypr_loofah_protocol::validate_capture_append_admission(admission)
        .map_err(|message| rejected(ErrorCode::Validation, message))?;
    if admission.request.scope != capture.scope
        || admission.request.capture_id != capture.capture_id
        || admission.request.request_id != capture.request_id
        || &admission.request.session_id != session_id
    {
        return Err(rejected(
            ErrorCode::Conflict,
            "Append admission belongs to another capture",
        ));
    }
    let root = context
        .operations
        .as_ref()
        .ok_or_else(unavailable)?
        .root
        .join("recording-appends");
    let source = context
        .store
        .prepare_recording_append_source(&root, identity(&admission.request)?, session_id)
        .await
        .map_err(|cause| storage_error(cause, false))?;
    let retained = source.clone();
    let expected = admission.clone();
    let pin = context.clone();
    tokio::task::spawn_blocking(move || {
        let _pin = pin;
        let actual = read_admission(retained.directory())?.ok_or_else(|| {
            rejected(
                ErrorCode::Conflict,
                "Append was not admitted before capture",
            )
        })?;
        if actual != expected {
            return Err(rejected(ErrorCode::Conflict, "Append admission changed"));
        }
        validate_source(&actual, &expected.request, &retained)
    })
    .await
    .map_err(io)??;
    if check_current
        && !source
            .recheck()
            .await
            .map_err(|cause| storage_error(cause, false))?
    {
        return Err(rejected(
            ErrorCode::Conflict,
            "Original append recording or transcript changed",
        ));
    }
    Ok(source)
}

pub(super) fn identity(input: &CaptureAppendRequest) -> Result<AppendIdentity, ApplicationError> {
    let parse = |value: &str| {
        uuid::Uuid::parse_str(value)
            .map_err(|_| rejected(ErrorCode::Validation, "Invalid append identity"))
    };
    Ok(AppendIdentity {
        server: input.scope.server.clone(),
        vault_id: input.scope.vault_id.clone(),
        member_id: input.scope.member_id.clone(),
        capture_id: parse(&input.capture_id)?,
        request_id: parse(&input.request_id)?,
        admission_id: parse(&input.admission_id)?,
    })
}
fn validate_source(
    admission: &CaptureAppendAdmission,
    request: &CaptureAppendRequest,
    source: &AppendSource,
) -> Result<(), ApplicationError> {
    hypr_loofah_protocol::validate_capture_append_admission(admission)
        .map_err(|_| unavailable())?;
    let baseline = &admission.baseline;
    if &admission.request != request
        || baseline.recording_selection != source.recording_selection()
        || baseline.recording_sha256 != source.recording_sha256()
        || baseline.transcript_revision != source.transcript_revision()
        || baseline.transcript_present != source.raw_transcript().is_some()
    {
        return Err(rejected(
            ErrorCode::Conflict,
            "Append source binding changed",
        ));
    }
    Ok(())
}
fn read_admission(directory: &Path) -> Result<Option<CaptureAppendAdmission>, ApplicationError> {
    let path = directory.join("core-admission.json");
    let metadata = match std::fs::symlink_metadata(&path) {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => return Err(io(cause)),
        Ok(value) => value,
    };
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() > 65536 {
        return Err(unavailable());
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io)?
        .take(65537)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() > 65536 {
        return Err(unavailable());
    }
    serde_json::from_slice(&bytes).map(Some).map_err(io)
}
fn persist_admission(
    directory: &Path,
    admission: &CaptureAppendAdmission,
) -> Result<(), ApplicationError> {
    if let Some(existing) = read_admission(directory)? {
        return if &existing == admission {
            Ok(())
        } else {
            Err(rejected(ErrorCode::Conflict, "Append admission changed"))
        };
    }
    let path = directory.join("core-admission.json");
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(io)?;
    file.write_all(&serde_json::to_vec(admission).map_err(io)?)
        .map_err(io)?;
    file.sync_all().map_err(io)?;
    sync(directory)
}
fn sync(path: &Path) -> Result<(), ApplicationError> {
    #[cfg(unix)]
    File::open(path).map_err(io)?.sync_all().map_err(io)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn unavailable() -> ApplicationError {
    rejected(
        ErrorCode::Unavailable,
        "Append admission storage is unavailable or needs review",
    )
}
fn io(cause: impl std::fmt::Display) -> ApplicationError {
    rejected(
        ErrorCode::Unavailable,
        &format!("Append preparation could not be confirmed: {cause}"),
    )
}

pub(crate) struct CaptureSuffixSource {
    pub original: AppendSource,
    pub path: std::path::PathBuf,
    pub sha256: String,
    pub seal: hypr_loofah_protocol::CaptureSeal,
}
impl VaultContext {
    pub(crate) async fn prepare_capture_suffix(
        self: &Arc<Self>,
        identity: &CaptureIdentity,
    ) -> Result<CaptureSuffixSource, ApplicationError> {
        let original = checked_source(self, identity, true).await?;
        let writer = self
            .capture_writer(
                identity.scope.member_id.clone(),
                identity.capture_id.clone(),
            )
            .await?;
        let snapshot = writer.snapshot().await?;
        if &snapshot.identity != identity {
            return Err(rejected(
                ErrorCode::Conflict,
                "Append suffix identity changed",
            ));
        }
        let seal = snapshot
            .seal
            .ok_or_else(|| rejected(ErrorCode::Unavailable, "Append suffix is not sealed"))?;
        let verified = super::wav::verify(&writer, &seal).await?;
        let path = original.directory().join("core-inference.wav");
        let temporary = original
            .directory()
            .join(format!("core-inference-{}.wav", uuid::Uuid::new_v4()));
        let (mut reader, producer) = verified.reader(self.clone(), writer);
        let pin = self.clone();
        let expected_sha = verified.artifact_sha256.clone();
        let expected_bytes = verified.artifact_size_bytes;
        let output_path = path.clone();
        let prepared = tokio::task::spawn_blocking(move || {
            let _pin = pin;
            if std::fs::symlink_metadata(&output_path).is_ok() {
                verify_private_wave(&output_path, &expected_sha, expected_bytes)?;
                return Ok::<_, ApplicationError>(());
            }
            let mut file = File::create_new(&temporary).map_err(io)?;
            std::io::copy(&mut reader, &mut file).map_err(io)?;
            file.sync_all().map_err(io)?;
            drop(file);
            verify_private_wave(&temporary, &expected_sha, expected_bytes)?;
            let mut permissions = std::fs::metadata(&temporary).map_err(io)?.permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&temporary, permissions).map_err(io)?;
            hypr_storage::fs::rename_no_replace(&temporary, &output_path).map_err(io)?;
            crate::transcription_jobs::ledger::sync_parent(&output_path)
        })
        .await;
        let produced = producer.await;
        prepared.map_err(io)??;
        produced.map_err(io)??;
        Ok(CaptureSuffixSource {
            original,
            path,
            sha256: verified.artifact_sha256,
            seal,
        })
    }
}
fn verify_private_wave(path: &Path, expected: &str, bytes: u64) -> Result<(), ApplicationError> {
    use sha2::Digest;
    let metadata = path.symlink_metadata().map_err(io)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != bytes {
        return Err(unavailable());
    }
    let mut file = File::open(path).map_err(io)?;
    let mut hash = sha2::Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer).map_err(io)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    let hash: String = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if hash != expected {
        return Err(rejected(
            ErrorCode::Conflict,
            "Retained append suffix changed",
        ));
    }
    Ok(())
}
