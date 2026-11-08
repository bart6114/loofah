pub(super) mod publication;
#[cfg(test)]
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
