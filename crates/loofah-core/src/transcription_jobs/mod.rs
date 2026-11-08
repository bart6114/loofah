//! Durable absent-only batch jobs. Hosts inject model admission and canonical
//! response mapping; this module never resolves a device model, endpoint or key.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, OnceLock},
};

use hypr_loofah_protocol::*;
use hypr_vault_read::{SessionAudio, TranscriptWithData};
use tokio_util::sync::CancellationToken;

use crate::{VaultContext, VaultRuntime, vault::error};

pub(crate) mod ledger;
mod source;
#[cfg(test)]
mod tests;
mod worker;

pub use source::TrustedTranscriptionSource;
pub(crate) use worker::TranscriptionWorker;

pub type TranscriptionFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Clone)]
pub struct TranscriptionWork {
    pub job_id: String,
    pub principal: String,
    pub request: TranscriptionJobRequest,
}

/// Preparation reserves the host's shared native/model capacity before source
/// hashing or copying. No operation or vault-write lock spans inference.
pub trait TranscriptionEngine: Send + Sync + 'static {
    fn prepare<'a>(
        &'a self,
        request: &'a TranscriptionJobRequest,
        cancel: CancellationToken,
    ) -> TranscriptionFuture<'a, Result<Box<dyn PreparedTranscription>, TranscriptionFailure>>;
}

pub trait PreparedTranscription: Send + 'static {
    /// Move source and admission owners into actual native work. Cancellation
    /// requests a stop; the future must await actual completion, including errors.
    fn transcribe(
        self: Box<Self>,
        input: TranscriptionInput,
        cancel: CancellationToken,
    ) -> TranscriptionFuture<'static, Result<TranscriptWithData, TranscriptionFailure>>;
}

pub struct TranscriptionInput {
    pub source: TrustedTranscriptionSource,
    pub metadata: TranscriptionMetadata,
    pub request: TranscriptionJobRequest,
}

#[derive(Clone)]
pub struct TranscriptionMetadata {
    pub session_id: String,
    // Legacy desktop injects the zero UUID. Remote jobs use their authenticated
    // member so the transcript's self channel belongs to the submitting person.
    pub user_id: String,
    pub created_at: String,
    pub memo_md: String,
    pub audio: SessionAudio,
    materialized: Arc<OnceLock<f64>>,
}

impl TranscriptionMetadata {
    /// Call while mapping the first result, never when starting a long model run.
    /// This is the legacy processing clock, not the recording's wall-clock time.
    pub fn started_at_ms(&self) -> f64 {
        *self
            .materialized
            .get_or_init(|| chrono::Utc::now().timestamp_millis() as f64)
    }

    pub fn empty_transcript(&self) -> TranscriptWithData {
        TranscriptWithData {
            id: uuid::Uuid::new_v4().to_string(),
            user_id: self.user_id.clone(),
            created_at: self.created_at.clone(),
            session_id: self.session_id.clone(),
            started_at: self.started_at_ms(),
            ended_at: None,
            memo_md: self.memo_md.clone(),
            words: Vec::new(),
            speaker_hints: Vec::new(),
        }
    }
}

pub struct TranscriptionAccess {
    /// Membership revocation only. An admitted job survives the original HTTP
    /// token's expiry/logout while membership remains valid.
    pub cancelled: CancellationToken,
    pub interrupted: CancellationToken,
    pub owner: Box<dyn Send>,
}

pub trait TranscriptionAuthorizer: Send + Sync + 'static {
    fn observe<'a>(
        &'a self,
        work: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>>;

    /// Acquire fresh current membership BEFORE Store preparation/recheck. Retain
    /// this grant through commit to preserve the host's Catalog -> Store order.
    fn authorize<'a>(
        &'a self,
        work: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>>;
}

#[derive(Debug, Clone, Copy)]
pub struct TranscriptionLimits {
    pub max_jobs: usize,
    pub max_queued: usize,
    pub private_audio_bytes: u64,
}

impl Default for TranscriptionLimits {
    fn default() -> Self {
        Self {
            max_jobs: 256,
            max_queued: 16,
            private_audio_bytes: 8 * 1024 * 1024 * 1024,
        }
    }
}

pub(crate) struct TranscriptionService {
    journal: ledger::Journal,
    stop: CancellationToken,
    finished: CancellationToken,
}

impl VaultRuntime {
    pub async fn enable_transcription(
        &mut self,
        engine: Arc<dyn TranscriptionEngine>,
        authorizer: Arc<dyn TranscriptionAuthorizer>,
        limits: TranscriptionLimits,
    ) -> Result<(), ApplicationError> {
        if self.transcription.is_some() || self.context.transcription.get().is_some() {
            return Err(error(
                ErrorCode::Conflict,
                "Transcription is already configured",
            ));
        }
        self.context.store.ensure_ready().map_err(storage)?;
        if self.context.lease.is_none() {
            return Err(unavailable());
        }
        let root = self
            .context
            .operations
            .as_ref()
            .ok_or_else(unavailable)?
            .root
            .join("transcription-jobs");
        let journal = ledger::Journal::open(
            root,
            self.context.events.clone(),
            limits,
            Some(self.context.clone()),
        )
        .await?;
        let worker =
            TranscriptionWorker::start(self.context.clone(), journal.clone(), engine, authorizer)
                .await?;
        let service = TranscriptionService {
            journal,
            stop: worker.stop.clone(),
            finished: worker.finished.clone(),
        };
        if self.context.transcription.set(service).is_err() {
            worker.shutdown().await?;
            return Err(error(
                ErrorCode::Conflict,
                "Transcription is already configured",
            ));
        }
        self.transcription = Some(worker);
        Ok(())
    }
}

impl VaultContext {
    pub(crate) async fn retry_unstarted_capture_repair(
        &self,
        member: &str,
        id: &str,
    ) -> Result<(), ApplicationError> {
        self.transcription_service()?
            .journal
            .retry_unstarted(member.to_owned(), id.to_owned())
            .await
    }
    pub(crate) async fn cancel_capture_repair_request(
        &self,
        member: String,
        request_id: String,
        request: TranscriptionJobRequest,
    ) -> Result<bool, ApplicationError> {
        let service = self.transcription_service()?;
        if let Some(snapshot) = service
            .journal
            .replay_submission(member.clone(), request_id, request)
            .await?
        {
            let snapshot = service.journal.cancel(member, snapshot.id).await?;
            return Ok(
                snapshot.state.is_terminal() && snapshot.state != TranscriptionJobState::Uncertain
            );
        }
        Ok(true)
    }
    fn transcription_service(&self) -> Result<&TranscriptionService, ApplicationError> {
        self.store.ensure_ready().map_err(storage)?;
        self.transcription.get().ok_or_else(unavailable)
    }

    async fn transcription_read(
        self: &Arc<Self>,
        operation: impl FnOnce(
            ledger::Journal,
        ) -> TranscriptionFuture<
            'static,
            Result<TranscriptionJobSnapshot, ApplicationError>,
        > + Send
        + 'static,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        let context = self.clone();
        tokio::spawn(async move {
            let service = context.transcription_service()?;
            if service.finished.is_cancelled() {
                service.journal.interrupt().await?;
            }
            operation(service.journal.clone()).await
        })
        .await
        .map_err(|_| unavailable())?
    }

    pub async fn submit_transcription_job(
        self: &Arc<Self>,
        member: String,
        request_id: String,
        input: TranscriptionJobRequest,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        self.submit_transcription_inner(member, request_id, input, None)
            .await
    }
    pub(crate) async fn submit_capture_repair_job(
        self: &Arc<Self>,
        member: String,
        request_id: String,
        input: TranscriptionJobRequest,
        origin: crate::capture_processing::CaptureChildOrigin,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        self.submit_transcription_inner(member, request_id, input, Some(origin))
            .await
    }
    async fn submit_transcription_inner(
        self: &Arc<Self>,
        member: String,
        request_id: String,
        input: TranscriptionJobRequest,
        origin: Option<crate::capture_processing::CaptureChildOrigin>,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        validate_transcription_job_request(&input)
            .map_err(|message| error(ErrorCode::Validation, message))?;
        let context = self.clone();
        tokio::spawn(async move {
            let service = context.transcription_service()?;
            if service.stop.is_cancelled() {
                return Err(unavailable());
            }
            if let Some(snapshot) = service
                .journal
                .replay_with_origin(
                    member.clone(),
                    request_id.clone(),
                    input.clone(),
                    origin.clone(),
                )
                .await?
            {
                return Ok(snapshot);
            }
            let _admission = context.admit_session_work(&input.session_id).await?;
            let exists = context
                .store
                .read_meta(&input.session_id)
                .await
                .map_err(|cause| crate::vault::storage_error(cause, false))?
                .is_some_and(|meta| meta.id == input.session_id);
            if !exists {
                return Err(error(ErrorCode::NotFound, "Session not found"));
            }
            service
                .journal
                .submit_with_origin(member, request_id, input, origin)
                .await
        })
        .await
        .map_err(|_| unavailable())?
    }

    pub(crate) async fn ensure_transcription_session_idle(
        &self,
        id: &str,
    ) -> Result<(), ApplicationError> {
        if let Some(service) = self.transcription.get() {
            service.journal.ensure_session_idle(id.to_owned()).await
        } else {
            let root = self
                .operations
                .as_ref()
                .ok_or_else(unavailable)?
                .root
                .join("transcription-jobs");
            ledger::ensure_stored_session_idle(root, self.events.vault_id(), id.to_owned()).await
        }
    }

    pub async fn transcription_job(
        self: &Arc<Self>,
        member: String,
        job: String,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        self.transcription_read(move |journal| {
            Box::pin(async move { journal.owned(member, job).await })
        })
        .await
    }

    pub async fn cancel_transcription_job(
        self: &Arc<Self>,
        member: String,
        job: String,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        self.transcription_read(move |journal| {
            Box::pin(async move { journal.cancel(member, job).await })
        })
        .await
    }

    /// Explicitly retries only the retained result's checked publication. It never
    /// returns an interrupted inference attempt to the generation queue.
    pub async fn resume_transcription_result(
        self: &Arc<Self>,
        member: String,
        job: String,
    ) -> Result<TranscriptionJobSnapshot, ApplicationError> {
        let context = self.clone();
        tokio::spawn(async move {
            let service = context.transcription_service()?;
            if service.stop.is_cancelled() {
                return Err(unavailable());
            }
            let record = service.journal.record(member.clone(), job.clone()).await?;
            let _admission = context
                .admit_session_work(&record.request.session_id)
                .await?;
            if context
                .store
                .read_meta(&record.request.session_id)
                .await
                .map_err(|cause| crate::vault::storage_error(cause, false))?
                .is_none_or(|meta| meta.id != record.request.session_id)
            {
                return Err(error(ErrorCode::NotFound, "Session not found"));
            }
            service.journal.resume(member, job).await
        })
        .await
        .map_err(|_| unavailable())?
    }

    pub async fn transcription_job_result(
        self: &Arc<Self>,
        member: String,
        job: String,
    ) -> Result<Option<TranscriptionJobResult>, ApplicationError> {
        let context = self.clone();
        tokio::spawn(async move {
            let service = context.transcription_service()?;
            if service.finished.is_cancelled() {
                service.journal.interrupt().await?;
            }
            let record = service.journal.record(member, job).await?;
            Ok(service
                .journal
                .result(&record)
                .await?
                .map(|transcript| TranscriptionJobResult {
                    job_id: record.snapshot.id,
                    session_id: record.request.session_id,
                    transcript,
                }))
        })
        .await
        .map_err(|_| unavailable())?
    }

    pub async fn list_transcription_jobs(
        self: &Arc<Self>,
        member: String,
        input: ListTranscriptionJobsInput,
    ) -> Result<TranscriptionJobPage, ApplicationError> {
        let context = self.clone();
        tokio::spawn(async move {
            let service = context.transcription_service()?;
            if service.finished.is_cancelled() {
                service.journal.interrupt().await?;
            }
            service.journal.list(member, input).await
        })
        .await
        .map_err(|_| unavailable())?
    }
}

fn unavailable() -> ApplicationError {
    error(
        ErrorCode::Unavailable,
        "Transcription is not available for this vault",
    )
}
fn storage(cause: impl std::fmt::Display) -> ApplicationError {
    tracing::error!(%cause, "transcription storage operation stopped");
    error(
        ErrorCode::Internal,
        "Transcription storage operation failed",
    )
}
fn capacity() -> ApplicationError {
    error(
        ErrorCode::ResourceLimit,
        "Transcription storage or queue capacity reached",
    )
}
fn uuid(value: &str) -> Result<String, ApplicationError> {
    let parsed = uuid::Uuid::parse_str(value)
        .map_err(|_| error(ErrorCode::Validation, "A UUID is required"))?;
    if parsed.is_nil() {
        return Err(error(ErrorCode::Validation, "A nonempty UUID is required"));
    }
    Ok(parsed.to_string())
}
