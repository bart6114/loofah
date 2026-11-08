use crate::{
    ProfileCommitGuard, ProfileGeneration, ProfileLease, RemoteClient, RemoteOperation,
    RemoteReply, Result, cancelled, failure, invalid,
};
use hypr_capture_store::{CaptureRetirementAdmission, CaptureRetirementAuthority, CaptureSnapshot};
use hypr_loofah_protocol::{
    CapturePublication, CaptureRetirementRequest, CaptureRetirementState, ErrorCode,
    RECORDING_CHUNK_LIMIT, RecordingLease,
};
use sha2::{Digest, Sha256};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;

/// Minted only by independent authenticated canonical byte verification. This is
/// a process-local capability, never a serialized receipt or replayable flag.
pub struct VerifiedCaptureRetirement {
    client: Arc<RemoteClient>,
    profile: Arc<ProfileLease>,
    generation: ProfileGeneration,
    auth_generation: u64,
    request: CaptureRetirementRequest,
    verified_at: Instant,
    cancel: CancellationToken,
    recording: Option<RetainedRecording>,
}

struct RetainedRecording {
    client: Arc<RemoteClient>,
    lease: RecordingLease,
    runtime: tokio::runtime::Handle,
}
impl Drop for RetainedRecording {
    fn drop(&mut self) {
        let client = self.client.clone();
        let lease = self.lease.clone();
        self.runtime.spawn(async move {
            let _ = client
                .close_recording(&lease.vault_id, &lease.id, CancellationToken::new())
                .await;
        });
    }
}

impl RemoteClient {
    pub async fn verify_capture_retirement(
        self: &Arc<Self>,
        profile: Arc<ProfileLease>,
        generation: ProfileGeneration,
        request: CaptureRetirementRequest,
        cancel: CancellationToken,
    ) -> Result<VerifiedCaptureRetirement> {
        self.capture_capability(&cancel)?;
        if !self
            .capabilities
            .features
            .iter()
            .any(|feature| feature == "capture_retirement")
        {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "Upgrade the server before verifying source cleanup",
            ));
        }
        hypr_loofah_protocol::validate_capture_retirement_request(&request).map_err(invalid)?;
        let current = generation.profile();
        if current.server != request.identity.scope.server
            || current.member_id.as_deref() != Some(&request.identity.scope.member_id)
            || current.vault_id.as_deref() != Some(&request.identity.scope.vault_id)
            || self.address() != request.identity.scope.server
        {
            return Err(changed());
        }
        // Refresh first, then bind the generation before any request that could
        // prove ownership. Interactive login invalidates this even for ABA scopes.
        self.access_token().await?;
        let auth_generation = self.tokens.lock().await.generation;
        self.verify_retirement_member(&request, &cancel).await?;
        let response: hypr_loofah_protocol::CaptureIngestStatus = self
            .capture_request(
                self.http
                    .post(self.endpoint(&[
                        "vaults",
                        &request.identity.scope.vault_id,
                        "captures",
                        &request.identity.capture_id,
                        "retirement",
                    ])?)
                    .json(&request),
                &request.cleanup_request_id,
                true,
                cancel.clone(),
            )
            .await?;
        let response = self.capture_status_checked(
            response,
            &request.identity.scope.vault_id,
            &request.identity.capture_id,
        )?;
        if response.identity != request.identity
            || response.format != request.format
            || response.retirement.as_ref().is_none_or(|value| {
                value.request != request
                    || value.state != CaptureRetirementState::Retired
                    || value.remaining_chunks != 0
            })
        {
            return Err(failure(
                ErrorCode::IncompatibleVersion,
                "Server cleanup receipt did not match the original request",
            ));
        }
        let lease = verify_canonical(
            self,
            &request.publication,
            &request.identity.scope.vault_id,
            &cancel,
        )
        .await?;
        let recording = RetainedRecording {
            client: self.clone(),
            lease,
            runtime: tokio::runtime::Handle::current(),
        };
        self.verify_retirement_member(&request, &cancel).await?;
        if self.tokens.lock().await.generation != auth_generation {
            return Err(changed());
        }
        // A later authorized server edit remains a deliberate user action. This
        // lease pins the exact verified generation, not its pathname forever.
        self.head_recording(&recording.lease, cancel.clone())
            .await?;
        Ok(VerifiedCaptureRetirement {
            client: self.clone(),
            profile,
            generation,
            auth_generation,
            request,
            verified_at: Instant::now(),
            cancel,
            recording: Some(recording),
        })
    }

    async fn verify_retirement_member(
        self: &Arc<Self>,
        request: &CaptureRetirementRequest,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let reply = self
            .execute(
                RemoteOperation::Me,
                &uuid::Uuid::new_v4().to_string(),
                cancel.clone(),
            )
            .await?;
        if !matches!(reply, RemoteReply::Me(me) if me.member.id == request.identity.scope.member_id)
        {
            return Err(changed());
        }
        Ok(())
    }
}

impl VerifiedCaptureRetirement {
    /// Host ownership must include its shutdown accounting and actual capture,
    /// export, upload and processing drains. The exclusive admission excludes
    /// all new raw users; these owners survive a cancelled async waiter.
    pub async fn retire_device(
        mut self,
        admission: CaptureRetirementAdmission,
        host: impl CaptureRetirementAuthority,
    ) -> Result<CaptureSnapshot> {
        let snapshot = admission.snapshot();
        if snapshot.identity != self.request.identity
            || snapshot.format != self.request.format
            || snapshot.seal.as_ref() != Some(&self.request.source)
            || snapshot.publication.as_ref() != Some(&self.request.publication)
            || snapshot
                .processing
                .as_ref()
                .map(|declaration| &declaration.request)
                != self.request.processing.as_ref()
        {
            return Err(changed());
        }
        if self.verified_at.elapsed() > Duration::from_secs(30) {
            return Err(failure(
                ErrorCode::Conflict,
                "Canonical verification expired; verify the original request again",
            ));
        }
        let profile = self.profile.guard_generation(&self.generation).await?;
        let tokens = self.client.tokens.clone().lock_owned().await;
        if tokens.generation != self.auth_generation || !tokens.current_for_commit() {
            return Err(changed());
        }
        let request = self.request.clone();
        let authority = DeviceAuthority {
            client: self.client.clone(),
            cancel: self.cancel.clone(),
            host,
            _profile: profile,
            _tokens: tokens,
            _recording: self.recording.take().expect("verified recording owner"),
        };
        authority.check_current()?;
        admission.retire_verified(request, authority).await
    }
}
struct DeviceAuthority<H> {
    client: Arc<RemoteClient>,
    cancel: CancellationToken,
    host: H,
    _profile: ProfileCommitGuard,
    _tokens: tokio::sync::OwnedMutexGuard<crate::auth::TokenState>,
    _recording: RetainedRecording,
}
impl<H: CaptureRetirementAuthority> CaptureRetirementAuthority for DeviceAuthority<H> {
    fn check_current(&self) -> Result<()> {
        if self.cancel.is_cancelled() || self.client.is_closed() {
            return Err(cancelled());
        }
        if !self._tokens.current_for_commit() {
            return Err(changed());
        }
        self.host.check_current()
    }
}
fn changed() -> hypr_loofah_protocol::ApplicationError {
    failure(
        ErrorCode::Conflict,
        "The original cleanup account, profile or source changed; device audio was retained",
    )
}

pub(crate) async fn verify_canonical(
    client: &Arc<RemoteClient>,
    publication: &CapturePublication,
    vault: &str,
    stop: &CancellationToken,
) -> Result<RecordingLease> {
    let id = uuid::Uuid::new_v4().to_string();
    let lease = client
        .open_recording(vault, &publication.session_id, &id, stop.clone())
        .await?
        .ok_or_else(|| {
            failure(
                ErrorCode::Conflict,
                "Published recording is absent; device audio was retained",
            )
        })?;
    let result = async {
        if lease.metadata.sha256 != publication.canonical_audio_sha256
            || lease.metadata.size_bytes != publication.canonical_audio_bytes
        {
            return Err(changed());
        }
        let mut hash = Sha256::new();
        let mut offset = 0u64;
        while offset < publication.canonical_audio_bytes {
            let bytes = client
                .read_recording_chunk(&lease, offset, RECORDING_CHUNK_LIMIT, stop.clone())
                .await?;
            if bytes.is_empty() || bytes.len() as u64 > publication.canonical_audio_bytes - offset {
                return Err(failure(
                    ErrorCode::Conflict,
                    "Canonical recording verification length did not match",
                ));
            }
            offset += bytes.len() as u64;
            hash.update(bytes);
        }
        let actual: String = hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        if offset != publication.canonical_audio_bytes
            || actual != publication.canonical_audio_sha256
        {
            return Err(failure(
                ErrorCode::Conflict,
                "Canonical recording verification checksum did not match",
            ));
        }
        Ok(())
    }
    .await;
    if let Err(cause) = result {
        let _ = client
            .close_recording(vault, &id, CancellationToken::new())
            .await;
        return Err(cause);
    }
    Ok(lease)
}
