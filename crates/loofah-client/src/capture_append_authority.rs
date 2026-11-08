use std::sync::Arc;

use hypr_loofah_protocol::{CaptureAppendRequest, ErrorCode};
use tokio_util::sync::CancellationToken;

use crate::{
    ProfileCommitGuard, ProfileGeneration, ProfileLease, RemoteClient, RemoteOperation,
    RemoteReply, Result, cancelled, failure, invalid,
};

/// Process-local authority for the original account and profile, including login ABA changes.
pub struct CaptureAppendAuthority {
    client: Arc<RemoteClient>,
    profile: Arc<ProfileLease>,
    generation: ProfileGeneration,
    auth_generation: u64,
    request: CaptureAppendRequest,
    cancel: CancellationToken,
}

pub struct CaptureAppendDispatchGuard {
    client: Arc<RemoteClient>,
    cancel: CancellationToken,
    _profile: ProfileCommitGuard,
    tokens: tokio::sync::OwnedMutexGuard<crate::auth::TokenState>,
}

impl RemoteClient {
    pub async fn bind_capture_append(
        self: &Arc<Self>,
        profile: Arc<ProfileLease>,
        request: CaptureAppendRequest,
        cancel: CancellationToken,
    ) -> Result<Arc<CaptureAppendAuthority>> {
        self.capture_capability(&cancel)?;
        hypr_loofah_protocol::validate_capture_append_request(&request).map_err(invalid)?;
        let generation = profile.generation().await?;
        let current = generation.profile();
        if current.server != request.scope.server
            || current.member_id.as_deref() != Some(&request.scope.member_id)
            || current.vault_id.as_deref() != Some(&request.scope.vault_id)
            || self.address() != request.scope.server
            || !current.credential_accounts.contains(&self.account)
        {
            return Err(changed());
        }
        self.access_token().await?;
        let auth_generation = self.tokens.lock().await.generation;
        let authority = Arc::new(CaptureAppendAuthority {
            client: self.clone(),
            profile,
            generation,
            auth_generation,
            request,
            cancel,
        });
        authority.check_current().await?;
        Ok(authority)
    }
}

impl CaptureAppendAuthority {
    pub fn request(&self) -> &CaptureAppendRequest {
        &self.request
    }

    pub async fn check_current(&self) -> Result<()> {
        self.client.capture_capability(&self.cancel)?;
        // Refresh may rotate credentials, but it must not replace the original login generation.
        if self.client.tokens.lock().await.generation != self.auth_generation {
            return Err(changed());
        }
        self.client.access_token().await?;
        let reply = self
            .client
            .execute(
                RemoteOperation::Me,
                &uuid::Uuid::new_v4().to_string(),
                self.cancel.clone(),
            )
            .await?;
        if !matches!(reply, RemoteReply::Me(me) if me.member.id == self.request.scope.member_id) {
            return Err(changed());
        }
        self.guard_dispatch().await?.check_current()
    }

    /// Hold only across the producer dispatch itself, never across the producer's acknowledgement.
    pub async fn guard_dispatch(&self) -> Result<CaptureAppendDispatchGuard> {
        let profile = self.profile.guard_generation(&self.generation).await?;
        let tokens = self.client.tokens.clone().lock_owned().await;
        if tokens.generation != self.auth_generation || !tokens.current_for_commit() {
            return Err(changed());
        }
        let guard = CaptureAppendDispatchGuard {
            client: self.client.clone(),
            cancel: self.cancel.clone(),
            _profile: profile,
            tokens,
        };
        guard.check_current()?;
        Ok(guard)
    }
}
impl CaptureAppendDispatchGuard {
    pub fn check_current(&self) -> Result<()> {
        if self.cancel.is_cancelled() || self.client.is_closed() {
            return Err(cancelled());
        }
        if !self.tokens.current_for_commit() {
            return Err(changed());
        }
        Ok(())
    }
}
fn changed() -> hypr_loofah_protocol::ApplicationError {
    failure(
        ErrorCode::Conflict,
        "The original append account or profile changed; retain the original request for review",
    )
}
