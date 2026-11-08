use std::{future::Future, sync::Arc};

use hypr_capture_store::capture_spool::{
    CaptureSnapshot, CaptureSpool, CaptureSpoolLimits, CaptureWriter,
};
use hypr_loofah_protocol::{
    ApplicationError, CaptureChunkDescriptor, CaptureIngestRequest, CaptureIngestStatus,
    CapturePrefixReceipt, CaptureScope, CaptureSeal, ErrorCode, MutationOutcome,
};
use sha2::{Digest, Sha256};
use tokio::sync::{OnceCell, Semaphore};

use super::{VaultContext, error};

#[path = "capture_ingest/lifecycle.rs"]
mod lifecycle;
#[path = "capture_ingest/publication/mod.rs"]
pub(crate) mod publication;
use lifecycle::target_session_id;

pub(super) struct CaptureIngestion {
    server: String,
    limits: CaptureSpoolLimits,
    spool: OnceCell<Arc<CaptureSpool>>,
    operations: Arc<Semaphore>,
    changed: Arc<tokio::sync::Notify>,
}

impl VaultContext {
    pub(crate) fn capture_target_session_id(
        identity: &hypr_loofah_protocol::CaptureIdentity,
    ) -> Result<String, ApplicationError> {
        target_session_id(identity)
    }
    pub(crate) fn capture_changes(&self) -> Option<Arc<tokio::sync::Notify>> {
        self.capture_ingestion
            .get()
            .map(|settings| settings.changed.clone())
    }
    pub(crate) async fn capture_writer(
        self: &Arc<Self>,
        member_id: String,
        capture_id: String,
    ) -> Result<CaptureWriter, ApplicationError> {
        self.store
            .ensure_ready()
            .map_err(|cause| crate::vault::storage_error(cause, false))?;
        validate_id(&capture_id)?;
        let settings = self.capture_ingestion.get().ok_or_else(|| {
            rejected(
                ErrorCode::Unavailable,
                "Capture ingestion is not configured",
            )
        })?;
        let root = self
            .operations
            .as_ref()
            .ok_or_else(|| rejected(ErrorCode::Unavailable, "Operation storage required"))?
            .root
            .join("captures");
        let spool = settings
            .spool
            .get_or_try_init(|| CaptureSpool::open(root, settings.limits.clone()))
            .await?
            .clone();
        find(
            &spool,
            CaptureScope {
                server: settings.server.clone(),
                member_id,
                vault_id: self.events.vault_id(),
            },
            &capture_id,
        )
        .await
    }

    pub fn enable_capture_ingestion(
        &self,
        server: String,
        limits: CaptureSpoolLimits,
    ) -> Result<(), ApplicationError> {
        let url = url::Url::parse(&server)
            .map_err(|_| rejected(ErrorCode::Validation, "Invalid capture server origin"))?;
        hypr_loofah_protocol::validate_endpoint(&url, true)
            .map_err(|message| rejected(ErrorCode::Validation, message))?;
        if url.path() != "/" || url.query().is_some() || url.as_str() != server {
            return Err(rejected(
                ErrorCode::Validation,
                "Capture requires a canonical server origin",
            ));
        }
        self.capture_ingestion
            .set(CaptureIngestion {
                server,
                limits,
                spool: OnceCell::new(),
                operations: Arc::new(Semaphore::new(2)),
                changed: Arc::new(tokio::sync::Notify::new()),
            })
            .map_err(|_| {
                rejected(
                    ErrorCode::Conflict,
                    "Capture ingestion is already configured",
                )
            })
    }

    pub async fn begin_capture_authorized<G, F>(
        self: &Arc<Self>,
        member_id: String,
        input: CaptureIngestRequest,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CaptureIngestStatus, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        let context = self.clone();
        self.capture_action(member_id, authorize, move |spool, scope| async move {
            if input.identity.scope != scope {
                return Err(rejected(
                    ErrorCode::Validation,
                    "Capture identity does not match the authenticated vault scope",
                ));
            }
            if matches!(
                input.identity.target,
                hypr_loofah_protocol::CaptureTarget::AppendSession { .. }
            ) {
                return Err(rejected(
                    ErrorCode::Unavailable,
                    "Append publication is not yet configured",
                ));
            }
            let _admission = context
                .admit_session_work(&target_session_id(&input.identity)?)
                .await?;
            let writer = spool
                .begin_with_observation(
                    input.identity,
                    input.format,
                    input.first_admitted_audio_observed_at_ms,
                )
                .await?;
            status(writer.snapshot().await?)
        })
        .await
    }

    pub async fn capture_status_authorized<G, F>(
        self: &Arc<Self>,
        member_id: String,
        capture_id: String,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CaptureIngestStatus, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        validate_id(&capture_id)?;
        self.capture_action(member_id, authorize, move |spool, scope| async move {
            let writer = find(&spool, scope, &capture_id).await?;
            status(writer.snapshot().await?)
        })
        .await
    }

    pub async fn append_capture_authorized<G, F>(
        self: &Arc<Self>,
        member_id: String,
        capture_id: String,
        descriptor: CaptureChunkDescriptor,
        bytes: Vec<u8>,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CapturePrefixReceipt, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        validate_id(&capture_id)?;
        if bytes.len() > 1024 * 1024
            || bytes.is_empty()
            || descriptor.bytes as usize != bytes.len()
            || descriptor.frames == 0
            || descriptor.sha256 != hash(&bytes)
            || descriptor.digest != hypr_loofah_protocol::capture_chunk_digest(&descriptor)
        {
            return Err(rejected(
                ErrorCode::Validation,
                "Capture chunk does not match its declaration",
            ));
        }
        let context = self.clone();
        self.capture_action(member_id, authorize, move |spool, scope| async move {
            let writer = find(&spool, scope, &capture_id).await?;
            let _admission = context
                .admit_session_work(&target_session_id(writer.identity())?)
                .await?;
            let snapshot = writer.snapshot().await?;
            if descriptor.capture_fingerprint != snapshot.capture_fingerprint {
                return Err(rejected(
                    ErrorCode::Conflict,
                    "Capture fingerprint does not match",
                ));
            }
            if descriptor.sequence > snapshot.chunk_count {
                return Err(rejected(
                    ErrorCode::Conflict,
                    "Capture chunk is beyond the admitted prefix",
                ));
            }
            if descriptor.sequence < snapshot.chunk_count {
                let stored = writer.chunks(descriptor.sequence.checked_sub(1), 1).await?;
                if stored.first() != Some(&descriptor) {
                    return Err(rejected(
                        ErrorCode::Conflict,
                        "Capture chunk conflicts with admitted audio",
                    ));
                }
            } else if descriptor.first_frame != snapshot.frames
                || descriptor.pcm_offset != snapshot.pcm_bytes
                || descriptor.previous_digest != snapshot.chain_digest
            {
                return Err(rejected(
                    ErrorCode::Conflict,
                    "Capture chunk does not extend the admitted prefix",
                ));
            }
            // Validate all declaration fields before the store admits any bytes.
            let accepted = writer
                .append_chunk(descriptor.sequence, descriptor.frames, bytes)
                .await?;
            if accepted != descriptor {
                let mut cause = error(ErrorCode::Internal, "Capture chunk outcome needs review");
                cause.mutation_outcome = Some(MutationOutcome::Unknown);
                return Err(cause);
            }
            Ok(CapturePrefixReceipt {
                capture_fingerprint: descriptor.capture_fingerprint,
                chunk_count: descriptor
                    .sequence
                    .checked_add(1)
                    .ok_or_else(|| rejected(ErrorCode::Validation, "Invalid capture sequence"))?,
                frames: descriptor
                    .first_frame
                    .checked_add(descriptor.frames as u64)
                    .ok_or_else(|| {
                        rejected(ErrorCode::Validation, "Invalid capture frame count")
                    })?,
                pcm_bytes: descriptor
                    .pcm_offset
                    .checked_add(descriptor.bytes as u64)
                    .ok_or_else(|| rejected(ErrorCode::Validation, "Invalid capture byte count"))?,
                chain_digest: descriptor.digest,
            })
        })
        .await
    }

    pub async fn seal_capture_authorized<G, F>(
        self: &Arc<Self>,
        member_id: String,
        capture_id: String,
        seal: CaptureSeal,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CaptureIngestStatus, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        validate_id(&capture_id)?;
        let context = self.clone();
        self.capture_action(member_id, authorize, move |spool, scope| async move {
            let writer = find(&spool, scope, &capture_id).await?;
            let _admission = context
                .admit_session_work(&target_session_id(writer.identity())?)
                .await?;
            writer.seal_if_matches(seal).await?;
            status(writer.snapshot().await?)
        })
        .await
    }

    async fn capture_action<G, F, T, A>(
        self: &Arc<Self>,
        member_id: String,
        authorize: impl FnOnce() -> F + Send + 'static,
        action: impl FnOnce(Arc<CaptureSpool>, CaptureScope) -> A + Send + 'static,
    ) -> Result<T, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
        T: Send + 'static,
        A: Future<Output = Result<T, ApplicationError>> + Send + 'static,
    {
        self.store
            .ensure_ready()
            .map_err(|cause| super::storage_error(cause, false))?;
        if member_id.is_empty() || member_id.len() > 512 || member_id.chars().any(char::is_control)
        {
            return Err(rejected(ErrorCode::Validation, "Invalid capture member"));
        }
        let settings = self.capture_ingestion.get().ok_or_else(|| {
            rejected(
                ErrorCode::Unavailable,
                "Capture ingestion is not configured",
            )
        })?;
        let permit = settings
            .operations
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                rejected(
                    ErrorCode::Unavailable,
                    "Capture storage is busy; retry shortly",
                )
            })?;
        let root = self
            .operations
            .as_ref()
            .map(|operations| operations.root.join("captures"))
            .ok_or_else(|| {
                rejected(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                )
            })?;
        let context = self.clone();
        tokio::spawn(async move {
            let _permit = permit;
            // This task owns the vault lease and fresh access grant through real filesystem completion.
            let settings = context
                .capture_ingestion
                .get()
                .expect("configured capture ingestion");
            let scope = CaptureScope {
                server: settings.server.clone(),
                member_id,
                vault_id: context.events.vault_id(),
            };
            let spool = settings
                .spool
                .get_or_try_init(|| CaptureSpool::open(root, settings.limits.clone()))
                .await?
                .clone();
            let _grant = authorize().await.map_err(not_committed)?;
            let result = action(spool, scope).await;
            if result.is_ok() {
                settings.changed.notify_waiters();
            }
            result
        })
        .await
        .map_err(|cause| {
            tracing::error!(%cause, "capture ingestion worker stopped");
            let mut cause = error(
                ErrorCode::Internal,
                "Capture outcome could not be confirmed",
            );
            cause.mutation_outcome = Some(MutationOutcome::Unknown);
            cause
        })?
    }
}

async fn find(
    spool: &Arc<CaptureSpool>,
    scope: CaptureScope,
    capture_id: &str,
) -> Result<CaptureWriter, ApplicationError> {
    let identity = spool
        .inventory(scope)
        .await?
        .into_iter()
        .find(|snapshot| snapshot.identity.capture_id == capture_id)
        .map(|snapshot| snapshot.identity)
        .ok_or_else(|| rejected(ErrorCode::NotFound, "Capture not found"))?;
    spool.resume(identity).await
}

fn status(snapshot: CaptureSnapshot) -> Result<CaptureIngestStatus, ApplicationError> {
    let observation = snapshot
        .first_admitted_audio_observed_at_ms
        .ok_or_else(|| {
            rejected(
                ErrorCode::Conflict,
                "Capture timing declaration needs review",
            )
        })?;
    let result = CaptureIngestStatus {
        identity: snapshot.identity,
        format: snapshot.format,
        prefix: CapturePrefixReceipt {
            capture_fingerprint: snapshot.capture_fingerprint,
            chunk_count: snapshot.chunk_count,
            frames: snapshot.frames,
            pcm_bytes: snapshot.pcm_bytes,
            chain_digest: snapshot.chain_digest,
        },
        seal: snapshot.seal,
        first_admitted_audio_observed_at_ms: observation,
        session_id: snapshot.session_id,
        publication: snapshot.publication,
    };
    if serde_json::to_vec(&result)
        .map_err(|_| error(ErrorCode::Internal, "Capture status could not be encoded"))?
        .len()
        > 64 * 1024
    {
        return Err(error(
            ErrorCode::ResourceLimit,
            "Capture status exceeds its size limit",
        ));
    }
    Ok(result)
}

fn validate_id(value: &str) -> Result<(), ApplicationError> {
    if uuid::Uuid::parse_str(value)
        .ok()
        .is_none_or(|id| id.to_string() != value)
    {
        return Err(rejected(
            ErrorCode::Validation,
            "Invalid capture identifier",
        ));
    }
    Ok(())
}
fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
fn rejected(code: ErrorCode, message: &str) -> ApplicationError {
    not_committed(error(code, message))
}
fn not_committed(mut cause: ApplicationError) -> ApplicationError {
    cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
    cause
}

#[cfg(test)]
#[path = "capture_ingest/tests.rs"]
mod tests;

#[path = "capture_ingest/wav.rs"]
mod wav;
