//! Capture-backed provisional transcription. Delivery never owns native work or
//! authorizes canonical publication.

use std::{
    collections::BTreeMap,
    future::Future,
    sync::{Arc, Mutex},
};

use hypr_capture_store::capture_spool::CaptureWriter;
use hypr_loofah_protocol::*;
use tokio_util::sync::CancellationToken;

use crate::{VaultContext, VaultRuntime, vault::error};

mod engine;
mod ledger;
#[cfg(test)]
mod tests;
mod worker;
pub use engine::*;

pub(crate) struct CaptureLiveService {
    journal: ledger::Journal,
    engine: Arc<dyn CaptureLiveEngine>,
    authorizer: Arc<dyn CaptureLiveAuthorizer>,
    stop: CancellationToken,
    tasks: Mutex<BTreeMap<String, Task>>,
    admissions: Arc<tokio::sync::Semaphore>,
    failed: std::sync::atomic::AtomicBool,
}

struct Task {
    cancel: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl VaultRuntime {
    pub async fn enable_capture_live(
        &mut self,
        engine: Arc<dyn CaptureLiveEngine>,
        authorizer: Arc<dyn CaptureLiveAuthorizer>,
    ) -> Result<(), ApplicationError> {
        if self.context.capture_live.get().is_some() {
            return Err(conflict("Live processing already configured"));
        }
        let root = self
            .context
            .operations
            .as_ref()
            .ok_or_else(|| unavailable("Operation storage required"))?
            .root
            .join("capture-live");
        let journal = ledger::Journal::open(root, self.context.clone()).await?;
        self.context
            .capture_live
            .set(Arc::new(CaptureLiveService {
                journal,
                engine,
                authorizer,
                stop: CancellationToken::new(),
                tasks: Mutex::new(BTreeMap::new()),
                admissions: Arc::new(tokio::sync::Semaphore::new(2)),
                failed: false.into(),
            }))
            .map_err(|_| conflict("Live processing already configured"))
    }
}

impl CaptureLiveService {
    pub(crate) fn request_shutdown(&self) {
        self.stop.cancel();
    }

    pub(crate) async fn shutdown(&self) -> Result<(), ApplicationError> {
        self.stop.cancel();
        let _admissions = self
            .admissions
            .clone()
            .acquire_many_owned(2)
            .await
            .expect("live admission semaphore remains open");
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for (_, task) in tasks {
            task.cancel.cancel();
            if task.handle.await.is_err() {
                tracing::error!(
                    "Live worker panicked; actual source/native completion remains unconfirmed"
                );
                std::future::pending::<()>().await;
            }
        }
        if self.failed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(storage(
                "Live capture shutdown retained failed/uncertain output",
            ));
        }
        Ok(())
    }
}

impl VaultContext {
    pub fn ensure_capture_processing_ready(&self, session: &str) -> Result<(), ApplicationError> {
        if self
            .capture_live
            .get()
            .is_some_and(|service| service.journal.blocks(session))
        {
            return Err(unavailable(
                "Capture transcription is not ready for source processing",
            ));
        }
        Ok(())
    }

    pub async fn start_capture_live_authorized<G, F>(
        self: &Arc<Self>,
        member: String,
        capture: String,
        request_id: String,
        request: CaptureLiveRequest,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CaptureLiveSnapshot, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        if !valid_uuid(&capture) || !valid_uuid(&request_id) {
            return Err(error(
                ErrorCode::Validation,
                "Canonical capture and generation UUIDs required",
            ));
        }
        validate_capture_live_request(&request)
            .map_err(|message| error(ErrorCode::Validation, message))?;
        let service = self
            .capture_live
            .get()
            .ok_or_else(|| unavailable("Live processing is not configured"))?
            .clone();
        if service.failed.load(std::sync::atomic::Ordering::Acquire) {
            return Err(storage("Live capture storage needs review"));
        }
        if service.stop.is_cancelled() {
            return Err(unavailable("Live processing is stopping"));
        }
        let admission = service
            .admissions
            .clone()
            .try_acquire_owned()
            .map_err(|_| unavailable("Live admission busy"))?;
        let context = self.clone();
        tokio::spawn(async move {
            let _admission = admission;
            let _grant = authorize().await?;
            if service.stop.is_cancelled() { return Err(unavailable("Live processing is stopping")); }
            let writer = context.capture_writer(member.clone(), capture).await?;
            let source = writer.snapshot().await?;
            if source.capture_fingerprint != request.capture_fingerprint { return Err(conflict("Capture fingerprint changed")); }
            let session = match &source.identity.target {
                CaptureTarget::ExistingSession {session_id, expected_audio_sha256: None} => session_id.clone(),
                _ => return Err(error(ErrorCode::Validation, "Live processing requires an existing session with absent recording baseline")),
            };
            context.validate(&session)?;
            // Replays do not reread/edit note metadata or restart a native engine.
            match service.journal.get(member.clone(), request_id.clone()).await {
                Ok(old) => {
                    if old.identity != source.identity || old.format != source.format || old.request != request { return Err(conflict("Live generation replay changed")); }
                    return Ok(old.snapshot);
                },
                Err(cause) if cause.code == ErrorCode::NotFound => {},
                Err(cause) => return Err(cause),
            }
            if source.seal.is_some() {
                return Err(conflict("Sealed captures require explicit recorded-audio repair, not a new live generation"));
            }
            let note = context.read_note(&session).await?.markdown.unwrap_or_default();
            let memo = tokio::task::spawn_blocking(move || {
                if note.len() > 8 * 1024 * 1024 { return Err(capacity()); }
                if note.is_empty() { return Ok(String::new()); }
                let json = hypr_tiptap::md_to_tiptap_json(&note).map_err(storage)?;
                serde_json::to_string(&json).map_err(storage)
            }).await.map_err(storage)??;
            let observation = source.first_admitted_audio_observed_at_ms.ok_or_else(|| conflict("Capture timing is unavailable"))?;
            if service.stop.is_cancelled() { return Err(unavailable("Live processing is stopping")); }
            let record = ledger::Record::new(source.identity, source.format, request, request_id.clone(), session, observation, &memo);
            let (record, fresh) = service.journal.admit(record, memo).await?;
            if fresh {
                let cancel = service.stop.child_token();
                let task_cancel = cancel.clone();
                let task_service = service.clone();
                let task_context = context.clone();
                let snapshot = record.snapshot.clone();
                let handle = tokio::spawn(async move {
                    worker::run(task_context, task_service, writer, record, task_cancel).await;
                });
                service.tasks.lock().unwrap().insert(request_id, Task {cancel, handle});
                Ok(snapshot)
            } else { Ok(record.snapshot) }
        }).await.map_err(storage)?
    }

    pub async fn capture_live_status_authorized<G, F>(
        self: &Arc<Self>,
        member: String,
        capture: String,
        generation: String,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CaptureLiveSnapshot, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        let context = self.clone();
        tokio::spawn(async move {
            let _grant = authorize().await?;
            let record = context.live_record(member, capture, generation).await?;
            Ok(record.snapshot)
        })
        .await
        .map_err(storage)?
    }

    pub async fn capture_live_captions_authorized<G, F>(
        self: &Arc<Self>,
        member: String,
        capture: String,
        generation: String,
        after: Option<u64>,
        limit: u32,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CaptureLiveCaptionPage, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        let context = self.clone();
        tokio::spawn(async move {
            let _grant = authorize().await?;
            context
                .live_record(member.clone(), capture, generation.clone())
                .await?;
            context
                .capture_live
                .get()
                .unwrap()
                .journal
                .page(member, generation, after, limit)
                .await
        })
        .await
        .map_err(storage)?
    }

    pub async fn capture_live_result_authorized<G, F>(
        self: &Arc<Self>,
        member: String,
        capture: String,
        generation: String,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<Option<TranscriptionJobResult>, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        let context = self.clone();
        tokio::spawn(async move {
            let _grant = authorize().await?;
            let record = context
                .live_record(member.clone(), capture, generation.clone())
                .await?;
            Ok(context
                .capture_live
                .get()
                .unwrap()
                .journal
                .result(member, generation.clone())
                .await?
                .map(|transcript| TranscriptionJobResult {
                    job_id: generation,
                    session_id: record.snapshot.session_id,
                    transcript,
                }))
        })
        .await
        .map_err(storage)?
    }

    pub async fn cancel_capture_live_authorized<G, F>(
        self: &Arc<Self>,
        member: String,
        capture: String,
        generation: String,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<CaptureLiveSnapshot, ApplicationError>
    where
        G: Send + 'static,
        F: Future<Output = Result<G, ApplicationError>> + Send + 'static,
    {
        let context = self.clone();
        tokio::spawn(async move {
            let _grant = authorize().await?;
            let record = context
                .live_record(member, capture, generation.clone())
                .await?;
            let service = context.capture_live.get().unwrap();
            if let Some(task) = service.tasks.lock().unwrap().get(&generation) {
                task.cancel.cancel();
            }
            if record.snapshot.state.is_terminal() {
                return Ok(record.snapshot);
            }
            Ok(service
                .journal
                .change(generation, |record| {
                    if !record.snapshot.state.is_terminal() {
                        record.snapshot.state = CaptureLiveState::Cancelling;
                    }
                })
                .await?
                .snapshot)
        })
        .await
        .map_err(storage)?
    }

    async fn live_record(
        &self,
        member: String,
        capture: String,
        generation: String,
    ) -> Result<ledger::Record, ApplicationError> {
        self.store.ensure_ready().map_err(storage)?;
        if !valid_uuid(&capture) || !valid_uuid(&generation) {
            return Err(error(
                ErrorCode::Validation,
                "Invalid live generation identity",
            ));
        }
        let record = self
            .capture_live
            .get()
            .ok_or_else(|| unavailable("Live processing is not configured"))?
            .journal
            .get(member, generation)
            .await?;
        if record.identity.capture_id != capture {
            return Err(not_found());
        }
        Ok(record)
    }
}

fn valid_uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}
fn capacity() -> ApplicationError {
    error(
        ErrorCode::ResourceLimit,
        "Live capture resource limit reached",
    )
}
fn storage(cause: impl std::fmt::Display) -> ApplicationError {
    tracing::error!(%cause, "Live capture storage failed");
    let mut error = error(ErrorCode::Internal, "Live capture storage needs review");
    error.mutation_outcome = Some(MutationOutcome::Unknown);
    error
}
fn unavailable(message: &str) -> ApplicationError {
    error(ErrorCode::Unavailable, message)
}
fn conflict(message: &str) -> ApplicationError {
    error(ErrorCode::Conflict, message)
}
fn not_found() -> ApplicationError {
    error(ErrorCode::NotFound, "Live capture generation not found")
}
