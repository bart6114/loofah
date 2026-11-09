use super::*;
use hypr_capture_store::CaptureRetirementAuthority;
use hypr_loofah_protocol::{CaptureRetirementRequest, validate_capture_retirement_request};
use hypr_vault_write::transcript_publication::ProcessingSourceGuard;

struct Authority<G> {
    context: Arc<VaultContext>,
    request: CaptureRetirementRequest,
    source: ProcessingSourceGuard,
    _grant: G,
    _session: tokio::sync::OwnedRwLockWriteGuard<()>,
    _host: Option<Box<dyn Send + Sync>>,
    _capacity: tokio::sync::OwnedSemaphorePermit,
}
impl<G: Send + 'static> CaptureRetirementAuthority for Authority<G> {
    fn check_current(&self) -> Result<(), ApplicationError> {
        self.context
            .store
            .ensure_ready()
            .map_err(|cause| crate::vault::storage_error(cause, false))?;
        self.context
            .ensure_recording_publication_ready(&self.request.publication.session_id)?;
        self.source
            .recheck_recording()
            .map_err(|cause| crate::vault::storage_error(cause, false))
    }
    fn permits_external_processing(&self, request: &CaptureRetirementRequest) -> bool {
        request == &self.request
    }
}

impl VaultContext {
    /// Explicit raw retirement never follows from a publication observation or a
    /// terminal job alone. The caller supplies fresh authorization twice; the
    /// final grant and exact source/session owners survive blocking filesystem I/O.
    pub async fn retire_capture_authorized<G, F>(
        self: &Arc<Self>,
        member: String,
        capture: String,
        request: CaptureRetirementRequest,
        authorize: impl Fn() -> F + Send + 'static,
    ) -> Result<CaptureIngestStatus, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        validate_id(&capture)?;
        validate_capture_retirement_request(&request)
            .map_err(|message| rejected(ErrorCode::Validation, message))?;
        if request.identity.capture_id != capture || request.identity.scope.member_id != member {
            return Err(rejected(
                ErrorCode::Validation,
                "Cleanup identity does not match this authenticated capture",
            ));
        }
        self.store
            .ensure_ready()
            .map_err(|cause| crate::vault::storage_error(cause, false))?;
        let settings = self.capture_ingestion.get().ok_or_else(|| {
            rejected(
                ErrorCode::Unavailable,
                "Capture ingestion is not configured",
            )
        })?;
        if request.identity.scope.server != settings.server
            || request.identity.scope.vault_id != self.events.vault_id()
            || target_session_id(&request.identity)? != request.publication.session_id
        {
            return Err(rejected(
                ErrorCode::Validation,
                "Cleanup does not match this original vault destination",
            ));
        }
        let capacity = settings
            .operations
            .clone()
            .try_acquire_owned()
            .map_err(|_| rejected(ErrorCode::Unavailable, "Capture retirement is busy"))?;
        let root = self
            .operations
            .as_ref()
            .ok_or_else(|| rejected(ErrorCode::Unavailable, "Operation storage required"))?
            .root
            .join("captures");
        let context = self.clone();
        tokio::spawn(async move {
            let initial = authorize().await.map_err(not_committed)?;
            let session = context.try_session_lifecycle_change(&request.publication.session_id)?;
            let settings = context
                .capture_ingestion
                .get()
                .expect("configured capture ingestion");
            let spool = settings
                .spool
                .get_or_try_init(|| CaptureSpool::open(root, settings.limits.clone()))
                .await?
                .clone();
            let admission = spool.admit_retirement(request.identity.clone()).await?;
            context
                .ensure_capture_session_lifecycle_idle(&request.publication.session_id)
                .await?;
            context
                .ensure_session_jobs_idle(&request.publication.session_id)
                .await?;
            context
                .ensure_transcription_retirement_idle(&request.publication.session_id)
                .await?;
            context
                .join_capture_live_retirement(&request.publication.session_id)
                .await?;
            context
                .verify_capture_retirement_declaration(&request)
                .await?;
            let host = match context.local_lifecycle_host.get() {
                Some(host) => Some(
                    host.admit(&context, &request.publication.session_id)
                        .await?,
                ),
                None => None,
            };
            drop(initial);
            let source = context
                .store
                .guard_processing_source(&request.publication.session_id, None)
                .await
                .map_err(|cause| crate::vault::storage_error(cause, false))?
                .ok_or_else(|| {
                    rejected(ErrorCode::Conflict, "Canonical recording is unavailable")
                })?;
            if source.binding().recording_sha256 != request.publication.canonical_audio_sha256
                || source
                    .recording_bytes()
                    .map_err(|cause| crate::vault::storage_error(cause, false))?
                    != request.publication.canonical_audio_bytes
            {
                return Err(rejected(
                    ErrorCode::Conflict,
                    "Canonical recording no longer matches the original publication",
                ));
            }
            context
                .verify_capture_retirement_declaration(&request)
                .await?;
            let grant = authorize().await.map_err(not_committed)?;
            let authority = Authority {
                context: context.clone(),
                request: request.clone(),
                source,
                _grant: grant,
                _session: session,
                _host: host,
                _capacity: capacity,
            };
            let snapshot = admission.retire_verified(request, authority).await?;
            settings.changed.notify_waiters();
            status(snapshot)
        })
        .await
        .map_err(|_| {
            let mut cause = error(
                ErrorCode::Internal,
                "Cleanup outcome requires the original request retry and fresh verification",
            );
            cause.mutation_outcome = Some(MutationOutcome::Unknown);
            cause
        })?
    }
}
