use std::sync::Arc;

use hypr_loofah_protocol::{CaptureScope, ErrorCode};
use tokio_util::sync::CancellationToken;

use crate::{
    ProfileCommitGuard, ProfileGeneration, ProfileLease, RemoteClient, RemoteOperation,
    RemoteReply, Result, cancelled, failure,
};

/// Original device account/profile authority for a selected vault. The selected
/// vault need not be the profile's saved default destination.
pub struct AuthenticatedScope {
    client: Arc<RemoteClient>,
    profile: Arc<ProfileLease>,
    generation: ProfileGeneration,
    auth_generation: u64,
    scope: CaptureScope,
    cancel: CancellationToken,
}

pub struct AuthenticatedScopeCommitGuard {
    client: Arc<RemoteClient>,
    cancel: CancellationToken,
    _profile: ProfileCommitGuard,
    tokens: tokio::sync::OwnedMutexGuard<crate::auth::TokenState>,
}

impl RemoteClient {
    pub async fn bind_authenticated_scope(
        self: &Arc<Self>,
        profile: Arc<ProfileLease>,
        scope: CaptureScope,
        cancel: CancellationToken,
    ) -> Result<Arc<AuthenticatedScope>> {
        for id in [&scope.member_id, &scope.vault_id] {
            if id.is_empty()
                || id.len() > 256
                || id.contains(['/', '\\', '\0'])
                || id == "."
                || id == ".."
            {
                return Err(failure(
                    ErrorCode::Validation,
                    "An original member and vault scope is required",
                ));
            }
        }
        let generation = profile.generation().await?;
        let current = generation.profile();
        if current.server != scope.server
            || self.address() != scope.server
            || current.member_id.as_deref() != Some(&scope.member_id)
            || !current.credential_accounts.contains(&self.account)
        {
            return Err(changed());
        }
        // Bind before refresh so an interactive-login ABA during HTTP cannot
        // replace the original account even when /me later has the same ID.
        let auth_generation = self.tokens.lock().await.generation;
        let authority = Arc::new(AuthenticatedScope {
            client: self.clone(),
            profile,
            generation,
            auth_generation,
            scope,
            cancel,
        });
        authority.check_current().await?;
        Ok(authority)
    }
}

impl AuthenticatedScope {
    pub(crate) fn profile_retirement(&self) -> CancellationToken {
        self.generation.retirement()
    }

    pub fn scope(&self) -> &CaptureScope {
        &self.scope
    }

    pub async fn check_current(&self) -> Result<()> {
        if self.cancel.is_cancelled() || self.client.is_closed() {
            return Err(cancelled());
        }
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
        if !matches!(reply,RemoteReply::Me(me) if me.member.id==self.scope.member_id) {
            return Err(changed());
        }
        self.guard_commit().await?.check_current()
    }

    pub async fn authorize_session(&self, session_id: &str) -> Result<()> {
        self.check_current().await?;
        let reply = self
            .client
            .execute(
                RemoteOperation::Session {
                    vault_id: self.scope.vault_id.clone(),
                    session_id: session_id.into(),
                },
                &uuid::Uuid::new_v4().to_string(),
                self.cancel.clone(),
            )
            .await?;
        if !matches!(reply,RemoteReply::Session(session) if session.meta.id==session_id) {
            return Err(changed());
        }
        self.guard_commit().await?.check_current()
    }

    /// Network and rendering must finish before taking this short publication fence.
    pub async fn guard_commit(&self) -> Result<AuthenticatedScopeCommitGuard> {
        let profile = self.profile.guard_generation(&self.generation).await?;
        let tokens = self.client.tokens.clone().lock_owned().await;
        if tokens.generation != self.auth_generation || !tokens.current_for_commit() {
            return Err(changed());
        }
        let guard = AuthenticatedScopeCommitGuard {
            client: self.client.clone(),
            cancel: self.cancel.clone(),
            _profile: profile,
            tokens,
        };
        guard.check_current()?;
        Ok(guard)
    }
}
impl AuthenticatedScopeCommitGuard {
    pub(crate) fn auth_retirement(&self) -> CancellationToken {
        self.tokens.retired.clone()
    }
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
        "The original account or profile changed; restart the export in its original workspace",
    )
}
