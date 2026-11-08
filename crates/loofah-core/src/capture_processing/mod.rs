//! Explicit capture processing declarations own orchestration, never raw capture
//! or upload. Child journals remain the authority for actual inference/commit.
use crate::{VaultContext, VaultRuntime, vault::error};
use futures_util::FutureExt;
use hypr_loofah_protocol::*;
use std::{future::Future, sync::Arc};
use tokio_util::sync::CancellationToken;

mod append;
mod ledger;
mod origin;
pub(crate) use origin::CaptureChildOrigin;
use origin::child_origin;
#[cfg(test)]
mod tests;
mod worker;

#[derive(Clone, PartialEq, Eq)]
pub struct CaptureProcessingModel {
    pub alias: String,
    pub sha256: String,
}

pub struct CaptureProcessingPolicy {
    pub model: Option<CaptureProcessingModel>,
    pub summary_enabled: bool,
    pub availability: Arc<dyn Fn() -> TranscriptionAvailability + Send + Sync>,
}

pub trait CaptureProcessingAuthorizer: Send + Sync + 'static {
    fn authorize<'a>(
        &'a self,
        member: &'a str,
    ) -> crate::transcription_jobs::TranscriptionFuture<
        'a,
        Result<Box<dyn Send>, TranscriptionFailure>,
    >;
}

pub(crate) struct CaptureProcessingService {
    append: append::Workers,
    journal: ledger::Journal,
    policy: CaptureProcessingPolicy,
    authorizer: Arc<dyn CaptureProcessingAuthorizer>,
    stop: CancellationToken,
    ready: CancellationToken,
    wake: tokio::sync::Notify,
    failed: std::sync::atomic::AtomicBool,
}
pub(crate) struct CaptureProcessingWorker {
    task: tokio::task::JoinHandle<Result<(), ApplicationError>>,
}
impl CaptureProcessingWorker {
    pub(crate) async fn shutdown(self) -> Result<(), ApplicationError> {
        self.task.await.map_err(storage)?
    }
}
impl VaultRuntime {
    /// Prepare before starting child services, then start after all are attached.
    /// This makes restart model/cancel checks precede queued paid/native work.
    pub async fn prepare_capture_processing(
        &mut self,
        policy: CaptureProcessingPolicy,
        authorizer: Arc<dyn CaptureProcessingAuthorizer>,
    ) -> Result<(), ApplicationError> {
        if self.context.capture_processing.get().is_some() {
            return Err(conflict("Capture processing already configured"));
        }
        if let Some(model) = &policy.model
            && (!is_transcription_sha256(&model.sha256)
                || model.alias.is_empty()
                || model.alias.len() > 512)
        {
            return Err(error(
                ErrorCode::Validation,
                "Invalid operator model binding",
            ));
        }
        let root = self
            .context
            .operations
            .as_ref()
            .ok_or_else(|| unavailable("Operation storage required"))?
            .root
            .join("capture-processing");
        let journal = ledger::Journal::open(root, self.context.clone()).await?;
        let service = Arc::new(CaptureProcessingService {
            append: Default::default(),
            journal,
            policy,
            authorizer,
            stop: CancellationToken::new(),
            ready: CancellationToken::new(),
            wake: tokio::sync::Notify::new(),
            failed: false.into(),
        });
        self.context
            .capture_processing
            .set(service.clone())
            .map_err(|_| conflict("Capture processing already configured"))?;
        let context = self.context.clone();
        self.capture_processing = Some(CaptureProcessingWorker {
            task: tokio::spawn(async move {
                let result = std::panic::AssertUnwindSafe(worker::run(context, service.clone()))
                    .catch_unwind()
                    .await
                    .unwrap_or_else(|_| Err(storage("Capture processing coordinator panicked")));
                let drained = service.append.shutdown().await;
                let result = result.and(drained);
                if result.is_err() {
                    service
                        .failed
                        .store(true, std::sync::atomic::Ordering::Release);
                }
                result
            }),
        });
        Ok(())
    }
    pub fn start_capture_processing(&self) {
        if let Some(service) = self.context.capture_processing.get() {
            service.ready.cancel();
        }
    }
}
impl CaptureProcessingService {
    pub(crate) fn request_shutdown(&self) {
        self.stop.cancel();
    }
    fn model_matches(&self, record: &ledger::Record) -> bool {
        self.policy.model.as_ref().is_some_and(|model| {
            model.alias == record.snapshot.model && model.sha256 == record.model_sha256
        })
    }
}
impl VaultContext {
    pub async fn submit_capture_processing_authorized<G, F>(
        self: &Arc<Self>,
        member: String,
        capture: String,
        request_id: String,
        input: CaptureProcessingRequest,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CaptureProcessingSnapshot, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        if !valid_uuid(&capture) || !valid_uuid(&request_id) || !valid_uuid(&member) {
            return Err(error(
                ErrorCode::Validation,
                "Canonical capture/member/request UUIDs required",
            ));
        }
        validate_capture_processing_request(&input)
            .map_err(|message| error(ErrorCode::Validation, message))?;
        let context = self.clone();
        tokio::spawn(async move {
            let _grant = authorize().await?;
            let service = context.capture_processing_service()?;
            let writer = context
                .capture_writer(member.clone(), capture.clone())
                .await?;
            let source = writer.snapshot().await?;
            if matches!(
                source.identity.target,
                CaptureTarget::ExistingSession {
                    expected_audio_sha256: Some(_),
                    ..
                }
            ) {
                return Err(error(
                    ErrorCode::Validation,
                    "Capture processing append is not supported",
                ));
            }
            if let CaptureTarget::AppendSession { admission, .. } = &source.identity.target
                && admission.request.processing.as_ref().is_none_or(|options| {
                    options.request(source.capture_fingerprint.clone()) != input
                })
            {
                return Err(conflict(
                    "Append processing differs from its pre-capture admission",
                ));
            }
            if source.identity.request_id != request_id
                || source.capture_fingerprint != input.capture_fingerprint
            {
                return Err(conflict("Capture processing identity changed"));
            }
            match service.journal.get(&member, &capture) {
                Ok(record) => {
                    return if record.identity == source.identity
                        && record.format == source.format
                        && record.snapshot.request == input
                    {
                        Ok(record.snapshot)
                    } else {
                        Err(conflict("Capture processing replay changed"))
                    };
                }
                Err(cause) if cause.code == ErrorCode::NotFound => {}
                Err(cause) => return Err(cause),
            }
            let model = service
                .policy
                .model
                .clone()
                .filter(|model| {
                    input
                        .model
                        .as_ref()
                        .is_none_or(|alias| alias == &model.alias)
                })
                .ok_or_else(|| unavailable("Capture processing model is not configured"))?;
            if input.summary.is_some() && !service.policy.summary_enabled {
                return Err(unavailable("Capture summaries are not configured"));
            }
            let session = Self::capture_target_session_id(&source.identity)?;
            let _admission = context.admit_session_work(&session).await?;
            let summary_revision = if let Some(summary) = &input.summary {
                if source.session_id.is_some() {
                    let doc = context.read_document(&session, &summary.target).await?;
                    if matches!(summary.target, DocumentTarget::TemplateOutput { .. })
                        && doc.document.is_none()
                    {
                        return Err(not_found());
                    }
                    Some(doc.revision)
                } else {
                    if !matches!(summary.target, DocumentTarget::Summary) {
                        return Err(error(
                            ErrorCode::Validation,
                            "Pending captures cannot target a missing template output",
                        ));
                    }
                    None
                }
            } else {
                None
            };
            let record = ledger::Record::new(source, model, input, session, summary_revision);
            let record = service.journal.admit(record).await?;
            service.wake.notify_one();
            Ok(record.snapshot)
        })
        .await
        .map_err(storage)?
    }
    pub async fn capture_processing_status_authorized<G, F>(
        self: &Arc<Self>,
        member: String,
        capture: String,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CaptureProcessingSnapshot, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        let context = self.clone();
        tokio::spawn(async move {
            let _grant = authorize().await?;
            Ok(context
                .capture_processing_service()?
                .journal
                .get(&member, &capture)?
                .snapshot)
        })
        .await
        .map_err(storage)?
    }
    pub async fn cancel_capture_processing_authorized<G, F>(
        self: &Arc<Self>,
        member: String,
        capture: String,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CaptureProcessingSnapshot, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        let context = self.clone();
        tokio::spawn(async move {
            let _grant = authorize().await?;
            let service = context.capture_processing_service()?;
            let previous = service.journal.get(&member, &capture)?;
            if previous.snapshot.state.is_terminal() {
                return Ok(previous.snapshot);
            }
            let mut next = previous.clone();
            next.cancel_requested = true;
            next.snapshot.state = CaptureProcessingState::Cancelled;
            let record = service.journal.update(previous, next).await?;
            service.wake.notify_one();
            Ok(record.snapshot)
        })
        .await
        .map_err(storage)?
    }
    fn capture_processing_service(
        &self,
    ) -> Result<Arc<CaptureProcessingService>, ApplicationError> {
        self.store.ensure_ready().map_err(storage)?;
        self.capture_processing
            .get()
            .filter(|service| {
                !service.stop.is_cancelled()
                    && !service.failed.load(std::sync::atomic::Ordering::Acquire)
            })
            .cloned()
            .ok_or_else(|| unavailable("Capture processing is not configured or requires review"))
    }
    pub(crate) async fn verify_capture_retirement_declaration(
        &self,
        request: &CaptureRetirementRequest,
    ) -> Result<(), ApplicationError> {
        self.ensure_capture_intent_idle(&request.publication.session_id, false)
            .await?;
        let operations = self
            .operations
            .as_ref()
            .ok_or_else(|| unavailable("Operation storage required"))?;
        ledger::verify_retirement_declaration(
            operations.root.join("capture-processing"),
            self.events.vault_id(),
            request.clone(),
            self.lease_pin(),
        )
        .await
    }

    pub(crate) async fn ensure_capture_intent_idle(
        &self,
        session: &str,
        source: bool,
    ) -> Result<(), ApplicationError> {
        if let Some(service) = self.capture_processing.get() {
            if service.failed.load(std::sync::atomic::Ordering::Acquire) {
                return Err(storage("Capture processing worker requires review"));
            }
            if service.journal.records()?.iter().any(|record| {
                record.target_session == session
                    && if source {
                        !record.transcript_ready
                            && record.snapshot.state != CaptureProcessingState::Cancelled
                    } else {
                        record.blocks()
                    }
            }) {
                return Err(unavailable("Capture processing must finish or be reviewed"));
            }
            return Ok(());
        }
        let Some(operations) = &self.operations else {
            return Ok(());
        };
        ledger::ensure_stored_idle(
            operations.root.join("capture-processing"),
            self.events.vault_id(),
            session.to_owned(),
            self.lease_pin(),
            source,
        )
        .await
    }
    pub(crate) async fn capture_child_admission(
        &self,
        member: &str,
        request_id: &str,
        origin: Option<&CaptureChildOrigin>,
    ) -> Result<(), ApplicationError> {
        let Some(service) = self.capture_processing.get() else {
            if origin.is_some() {
                return Err(unavailable("Capture processing child has no coordinator"));
            }
            return self
                .reject_unattached_capture_child(member, request_id, None)
                .await;
        };
        let found = if let Some(origin) = origin {
            Some(
                service
                    .journal
                    .get(member, origin.capture_id())
                    .map_err(|_| unavailable("Capture processing child has no valid parent"))?,
            )
        } else {
            service.journal.records()?.into_iter().find(|record| {
                record.member() == member
                    && [
                        child_request_id(record, "repair"),
                        child_request_id(record, "summary"),
                    ]
                    .iter()
                    .any(|id| id == request_id)
            })
        };
        let Some(record) = found else {
            return Ok(());
        };
        // Child workers may already be running as host initialization attaches
        // services, but cannot enter inference before intent reconciliation.
        tokio::select! {_=service.ready.cancelled()=>{},_=service.stop.cancelled()=>return Err(unavailable("Capture processing is stopping"))};
        let record = service
            .journal
            .get(record.member(), &record.identity.capture_id)?;
        if record.cancel_requested
            || record.snapshot.state.is_terminal()
            || !service.model_matches(&record)
            || origin.is_none_or(|origin| !origin.matches(&record, request_id))
            || (child_request_id(&record, "repair") == request_id
                && record.repair_request.is_none())
            || (child_request_id(&record, "summary") == request_id
                && record.summary_request.is_none())
        {
            return Err(conflict("Capture processing child requires review"));
        }
        Ok(())
    }
    pub(crate) async fn capture_live_intent_admission(
        &self,
        member: &str,
        capture: &str,
        generation: &str,
    ) -> Result<(), ApplicationError> {
        let Some(service) = self.capture_processing.get() else {
            return self
                .reject_unattached_capture_child(member, generation, Some(capture.to_owned()))
                .await;
        };
        let record = match service.journal.get(member, capture) {
            Ok(record) => record,
            Err(cause) if cause.code == ErrorCode::NotFound => return Ok(()),
            Err(cause) => return Err(cause),
        };
        if record.identity.request_id != generation {
            return Ok(());
        }
        tokio::select! {_=service.ready.cancelled()=>{},_=service.stop.cancelled()=>return Err(unavailable("Capture processing is stopping"))};
        let record = service.journal.get(member, capture)?;
        if record.cancel_requested
            || record.snapshot.state.is_terminal()
            || !service.model_matches(&record)
            || record.snapshot.live_generation_id.as_deref() != Some(generation)
        {
            return Err(conflict("Live capture processing requires review"));
        }
        Ok(())
    }
    pub(crate) async fn guard_capture_summary_source(
        &self,
        member: &str,
        request_id: &str,
        origin: Option<&CaptureChildOrigin>,
    ) -> Result<
        Option<hypr_vault_write::transcript_publication::ProcessingSourceGuard>,
        ApplicationError,
    > {
        self.capture_child_admission(member, request_id, origin)
            .await?;
        let Some(service) = self.capture_processing.get() else {
            self.reject_unattached_capture_child(member, request_id, None)
                .await?;
            return Ok(None);
        };
        let record = service.journal.records()?.into_iter().find(|record| {
            record.member() == member && child_request_id(record, "summary") == request_id
        });
        let Some(record) = record else {
            return Ok(None);
        };
        if record.cancel_requested || !record.transcript_ready || !service.model_matches(&record) {
            return Err(conflict("Capture summary source requires review"));
        }
        let expected = record
            .summary_source
            .ok_or_else(|| conflict("Capture summary has no source binding"))?;
        self.store
            .guard_processing_source(&record.target_session, Some(expected))
            .await
            .map_err(|cause| crate::vault::storage_error(cause, false))?
            .map(Some)
            .ok_or_else(|| conflict("Capture summary source disappeared"))
    }
    async fn reject_unattached_capture_child(
        &self,
        member: &str,
        request_id: &str,
        live_capture: Option<String>,
    ) -> Result<(), ApplicationError> {
        let Some(operations) = &self.operations else {
            return Ok(());
        };
        ledger::reject_stored_child(
            operations.root.join("capture-processing"),
            self.events.vault_id(),
            member.to_owned(),
            request_id.to_owned(),
            live_capture,
            self.lease_pin(),
        )
        .await
    }
}
fn child_request_id(record: &ledger::Record, stage: &str) -> String {
    use sha2::{Digest, Sha256};
    let data = serde_json::to_vec(&("loofah/capture-processing/v1", &record.identity, stage))
        .expect("capture identity serializes");
    let digest = Sha256::digest(data);
    let mut bytes = [0; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}
fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn unavailable(message: &str) -> ApplicationError {
    error(ErrorCode::Unavailable, message)
}
fn conflict(message: &str) -> ApplicationError {
    error(ErrorCode::Conflict, message)
}
fn not_found() -> ApplicationError {
    error(
        ErrorCode::NotFound,
        "Capture processing intent or target not found",
    )
}
fn capacity() -> ApplicationError {
    error(
        ErrorCode::ResourceLimit,
        "Capture processing capacity reached",
    )
}
fn storage(cause: impl std::fmt::Display) -> ApplicationError {
    tracing::error!(%cause,"Capture processing storage failed");
    let mut cause = error(ErrorCode::Internal, "Capture processing requires review");
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}
