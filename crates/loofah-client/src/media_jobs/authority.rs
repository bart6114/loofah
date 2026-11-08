use super::*;
use crate::{ProfileCommitGuard, ProfileGeneration, ProfileLease, RemoteOperation, RemoteReply};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct MediaScope {
    pub profile_id: String,
    pub server: String,
    pub member_id: String,
    pub vault_id: String,
}

pub struct MediaAuthority {
    pub(super) client: Arc<RemoteClient>,
    profile: Arc<ProfileLease>,
    generation: Arc<ProfileGeneration>,
    auth_generation: u64,
    scope: MediaScope,
    pub(super) cancel: CancellationToken,
    pub(super) lifetime: CancellationToken,
}

pub(super) struct DispatchGuard {
    client: Arc<RemoteClient>,
    cancel: CancellationToken,
    lifetime: CancellationToken,
    _profile: ProfileCommitGuard,
    tokens: tokio::sync::OwnedMutexGuard<crate::auth::TokenState>,
    pub access: String,
}
impl RemoteClient {
    pub async fn bind_media(
        self: &Arc<Self>,
        profile: Arc<ProfileLease>,
        scope: MediaScope,
        cancel: CancellationToken,
    ) -> Result<Arc<MediaAuthority>> {
        self.media_jobs_capability(&cancel)?;
        let generation = profile.generation().await?;
        let original = generation.profile();
        if original.id != scope.profile_id
            || original.server != scope.server
            || original.member_id.as_deref() != Some(&scope.member_id)
            || original.vault_id.as_deref() != Some(&scope.vault_id)
            || self.address() != scope.server
            || !original.credential_accounts.contains(&self.account)
        {
            return Err(changed());
        }
        let auth_generation = self.tokens.lock().await.generation;
        self.access_token().await?;
        if self.tokens.lock().await.generation != auth_generation {
            return Err(changed());
        }
        let authority = Arc::new(MediaAuthority {
            client: self.clone(),
            profile,
            generation: Arc::new(generation),
            auth_generation,
            scope,
            lifetime: cancel.clone(),
            cancel,
        });
        authority.check_current().await?;
        Ok(authority)
    }
}
impl MediaAuthority {
    pub fn for_request(self: &Arc<Self>, cancel: CancellationToken) -> Self {
        Self {
            client: self.client.clone(),
            profile: self.profile.clone(),
            generation: self.generation.clone(),
            auth_generation: self.auth_generation,
            scope: self.scope.clone(),
            cancel,
            lifetime: self.lifetime.clone(),
        }
    }
    pub fn scope(&self) -> &MediaScope {
        &self.scope
    }
    pub async fn check_current(&self) -> Result<()> {
        self.client.media_jobs_capability(&self.cancel)?;
        if self.lifetime.is_cancelled() {
            return Err(crate::cancelled());
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
        if !matches!(reply, RemoteReply::Me(me) if me.member.id == self.scope.member_id) {
            return Err(changed());
        }
        self.guard().await?.check_current()
    }
    pub(super) async fn guard(&self) -> Result<Arc<DispatchGuard>> {
        let profile = self.profile.guard_generation(&self.generation).await?;
        // Capture access before taking the token mutex: refresh uses this same mutex.
        self.client.access_token().await?;
        let tokens = self.client.tokens.clone().lock_owned().await;
        if tokens.generation != self.auth_generation || !tokens.current_for_commit() {
            return Err(changed());
        }
        let access = tokens.access_for_commit().ok_or_else(changed)?.to_owned();
        let guard = Arc::new(DispatchGuard {
            client: self.client.clone(),
            cancel: self.cancel.clone(),
            lifetime: self.lifetime.clone(),
            _profile: profile,
            tokens,
            access,
        });
        guard.check_current()?;
        Ok(guard)
    }
}
impl DispatchGuard {
    pub fn check_current(&self) -> Result<()> {
        if self.cancel.is_cancelled() || self.lifetime.is_cancelled() || self.client.is_closed() {
            return Err(crate::cancelled());
        }
        if !self.tokens.current_for_commit() {
            return Err(changed());
        }
        Ok(())
    }
}
fn changed() -> ApplicationError {
    failure(
        ErrorCode::Conflict,
        "The original media account or profile changed; retain its original request for review",
    )
}

pub(super) struct SourcePin {
    _profile: Arc<ProfileLease>,
    client: Arc<RemoteClient>,
    cancel: CancellationToken,
    lifetime: CancellationToken,
}
impl MediaAuthority {
    pub(super) fn pin_source(&self) -> Arc<SourcePin> {
        Arc::new(SourcePin {
            _profile: self.profile.clone(),
            client: self.client.clone(),
            cancel: self.cancel.clone(),
            lifetime: self.lifetime.clone(),
        })
    }
}
impl SourcePin {
    pub fn check_current(&self) -> Result<()> {
        if self.cancel.is_cancelled() || self.lifetime.is_cancelled() || self.client.is_closed() {
            return Err(crate::cancelled());
        }
        Ok(())
    }
}

/// Holds only the final original profile/login fence, after the private copy is complete.
pub struct MediaSourceExportGuard(Arc<DispatchGuard>);
impl MediaSourceExportGuard {
    pub fn check_current(&self) -> Result<()> {
        self.0.check_current()
    }
}
impl MediaAuthority {
    pub async fn guard_source_export(
        &self,
        submission: &MediaSubmission,
    ) -> Result<MediaSourceExportGuard> {
        self.client.media_source_export_capability(&self.cancel)?;
        self.validate_submission(submission)?;
        // Status authorizes the original vault's current editor role without
        // rehashing the source or dispatching any work.
        let status = self.status(&submission.declaration.request_id).await?;
        if status.declaration != submission.declaration
            || submission.admission_revision.as_deref() != Some(status.revision.as_str())
        {
            return Err(failure(
                ErrorCode::Conflict,
                "Original media source admission changed before saving",
            ));
        }
        self.check_current().await?;
        Ok(MediaSourceExportGuard(self.guard().await?))
    }
}
