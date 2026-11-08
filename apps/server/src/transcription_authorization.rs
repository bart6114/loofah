use hypr_loofah_core::transcription_jobs::{
    TranscriptionAccess, TranscriptionAuthorizer, TranscriptionFuture, TranscriptionWork,
};
use hypr_loofah_protocol::{ErrorCode, TranscriptionFailure};
use tokio_util::sync::CancellationToken;

use crate::{catalog::Catalog, error::ApiError};

pub(crate) struct CatalogTranscriptionAuthorizer {
    catalog: Catalog,
    vault_id: String,
    stop: CancellationToken,
}

impl CatalogTranscriptionAuthorizer {
    pub(crate) fn new(catalog: Catalog, vault_id: String, stop: CancellationToken) -> Self {
        Self {
            catalog,
            vault_id,
            stop,
        }
    }
}

struct MembershipWatch {
    stop: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for MembershipWatch {
    fn drop(&mut self) {
        self.stop.cancel();
        // This task only waits on in-memory catalogue gates and notifications.
        // It never owns a Store/native operation that could outlive an abort.
        self.task.abort();
    }
}

impl CatalogTranscriptionAuthorizer {
    fn observe_member<'a>(
        &'a self,
        member: &'a str,
    ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>> {
        Box::pin(async move {
            let mut changes = self.catalog.subscribe();
            drop(self.authorize_member(member).await?);
            let cancelled = CancellationToken::new();
            let revoked = cancelled.clone();
            let interrupted = CancellationToken::new();
            let host_interrupted = interrupted.clone();
            let owner_stop = CancellationToken::new();
            let end = owner_stop.clone();
            let host_stop = self.stop.clone();
            let catalog = self.catalog.clone();
            let vault = self.vault_id.clone();
            let member = member.to_owned();
            let task = tokio::spawn(async move {
                loop {
                    tokio::select! {
                        biased;
                        _ = end.cancelled() => return,
                        _ = host_stop.cancelled() => { host_interrupted.cancel(); return; },
                        changed = changes.changed() => {
                            if changed.is_err() { host_interrupted.cancel(); return; }
                        },
                    }
                    let access = tokio::select! {
                        biased;
                        _ = end.cancelled() => return,
                        _ = host_stop.cancelled() => { host_interrupted.cancel(); return; },
                        access = catalog.access_member(&member, &vault, true) => access,
                    };
                    match access {
                        Ok(grant) => drop(grant),
                        Err(error) => {
                            if member_failure(error) == TranscriptionFailure::Interrupted {
                                host_interrupted.cancel();
                            } else {
                                revoked.cancel();
                            }
                            return;
                        }
                    }
                }
            });
            Ok(TranscriptionAccess {
                interrupted,
                cancelled,
                owner: Box::new(MembershipWatch {
                    stop: owner_stop,
                    task,
                }),
            })
        })
    }

    fn authorize_member<'a>(
        &'a self,
        member: &'a str,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        Box::pin(async move {
            if self.stop.is_cancelled() {
                return Err(TranscriptionFailure::Interrupted);
            }
            let grant = self
                .catalog
                .access_member(member, &self.vault_id, true)
                .await
                .map_err(member_failure)?;
            if self.stop.is_cancelled() {
                return Err(TranscriptionFailure::Interrupted);
            }
            Ok(Box::new(grant) as Box<dyn Send>)
        })
    }
}

impl TranscriptionAuthorizer for CatalogTranscriptionAuthorizer {
    fn observe<'a>(
        &'a self,
        work: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>> {
        self.observe_member(&work.principal)
    }
    fn authorize<'a>(
        &'a self,
        work: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        self.authorize_member(&work.principal)
    }
}
impl hypr_loofah_core::capture_processing::CaptureProcessingAuthorizer
    for CatalogTranscriptionAuthorizer
{
    fn authorize<'a>(
        &'a self,
        member: &'a str,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        self.authorize_member(member)
    }
}
impl hypr_loofah_core::capture_live::CaptureLiveAuthorizer for CatalogTranscriptionAuthorizer {
    fn observe<'a>(
        &'a self,
        work: &'a hypr_loofah_core::capture_live::CaptureLiveWork,
    ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>> {
        self.observe_member(&work.principal)
    }
    fn authorize<'a>(
        &'a self,
        work: &'a hypr_loofah_core::capture_live::CaptureLiveWork,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        self.authorize_member(&work.principal)
    }
}

fn member_failure(error: ApiError) -> TranscriptionFailure {
    if error.0.code == ErrorCode::Unavailable {
        TranscriptionFailure::Interrupted
    } else {
        TranscriptionFailure::AccessDenied
    }
}
