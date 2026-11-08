//! Optional operator-owned CPU adapter. Core owns source verification, durable jobs,
//! authorization and publication; this host owns native admission and actual drain.

use std::{
    panic::AssertUnwindSafe,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use futures_util::FutureExt;
use hypr_loofah_core::transcription_jobs::{
    PreparedTranscription, TranscriptionEngine, TranscriptionFuture, TranscriptionInput,
};
use hypr_loofah_protocol::{TranscriptionFailure, TranscriptionJobRequest};
use hypr_transcribe_core::{
    recorded_audio::PreparedAudio,
    transcript::{BatchTranscriptMeta, transcript_from_batch_response},
};
use hypr_transcribe_whisper_local::resources::{
    BatchAdmission, BatchObserver, BatchSnapshot, Limits, ModelSpec, OwnedBatch, ResourceError,
    ResourceObserver, TrustedBatchSource, WhisperResources,
};
use hypr_vault_read::TranscriptWithData;
use owhisper_interface::{ListenParams, batch};
use tokio_util::sync::CancellationToken;

use crate::speech_admission::{AdmissionError, SpeechAdmission, SpeechPermit};

mod live;
#[cfg(test)]
mod tests;

type SourceOwner = Arc<Mutex<Box<dyn Send>>>;
type BatchResult = Arc<Result<batch::Response, ResourceError>>;

pub(crate) struct CpuTranscriptionHost {
    model: String,
    resources: Arc<WhisperResources>,
    admission: Arc<SpeechAdmission>,
    failed: Arc<AtomicBool>,
}

impl CpuTranscriptionHost {
    pub(crate) fn start(
        model: String,
        spec: ModelSpec,
        limits: Limits,
        admission: Arc<SpeechAdmission>,
    ) -> Result<Arc<Self>, ResourceError> {
        if admission.total() != limits.workload_groups
            || model.is_empty()
            || model.len() > 512
            || model.trim() != model
            || model.chars().any(char::is_control)
        {
            return Err(ResourceError::Invalid("model binding"));
        }
        Ok(Arc::new(Self {
            model,
            resources: Arc::new(WhisperResources::start_cpu(spec, limits)?),
            admission,
            failed: Arc::new(AtomicBool::new(false)),
        }))
    }

    pub(crate) fn accepting(&self) -> bool {
        !self.failed.load(Ordering::Acquire) && !self.admission.is_stopped()
    }

    pub(crate) fn capacity_exhausted(&self) -> bool {
        self.admission.is_busy()
    }

    pub(crate) fn observer(&self) -> ResourceObserver {
        self.resources.observer()
    }

    pub(crate) fn shutdown(&self) -> ResourceObserver {
        self.stop_admission();
        self.resources.shutdown()
    }

    pub(crate) fn stop_admission(&self) {
        self.failed.store(true, Ordering::Release);
        self.admission.stop();
    }
}

impl TranscriptionEngine for CpuTranscriptionHost {
    fn prepare<'a>(
        &'a self,
        request: &'a TranscriptionJobRequest,
        cancel: CancellationToken,
    ) -> TranscriptionFuture<'a, Result<Box<dyn PreparedTranscription>, TranscriptionFailure>> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(TranscriptionFailure::Interrupted);
            }
            if self.failed.load(Ordering::Acquire) {
                return Err(TranscriptionFailure::EngineUnavailable);
            }
            let params = parameters(&self.model, request)?;
            if self.resources.observer().snapshot().phase
                != hypr_transcribe_whisper_local::resources::ResourcePhase::Ready
            {
                return Err(TranscriptionFailure::EngineUnavailable);
            }
            let capacity = self
                .admission
                .batch(&cancel)
                .await
                .map_err(admission_failure)?;
            let admission = self.resources.try_batch().map_err(resource_failure)?;
            if cancel.is_cancelled() {
                return Err(TranscriptionFailure::Interrupted);
            }
            if self.failed.load(Ordering::Acquire) {
                return Err(TranscriptionFailure::EngineUnavailable);
            }
            Ok(Box::new(PreparedCpu {
                resources: self.resources.clone(),
                admission,
                capacity: Arc::new(capacity),
                params,
                request: request.clone(),
                failed: self.failed.clone(),
            }) as Box<dyn PreparedTranscription>)
        })
    }
}

fn parameters(
    model: &str,
    request: &TranscriptionJobRequest,
) -> Result<ListenParams, TranscriptionFailure> {
    hypr_loofah_protocol::validate_transcription_job_request(request)
        .map_err(|_| TranscriptionFailure::EngineUnavailable)?;
    parameters_for_options(model, &request.model, &request.languages, &request.keywords)
}

fn parameters_for_options(
    model: &str,
    requested_model: &str,
    languages: &[String],
    keywords: &[String],
) -> Result<ListenParams, TranscriptionFailure> {
    if requested_model != model {
        return Err(TranscriptionFailure::EngineUnavailable);
    }
    let mut params = ListenParams {
        model: Some(model.to_owned()),
        keywords: keywords.to_vec(),
        ..Default::default()
    };
    params.languages = languages
        .iter()
        .map(|language| language.parse())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| TranscriptionFailure::EngineUnavailable)?;
    for language in &params.languages {
        let _: hypr_whisper::Language = language
            .clone()
            .try_into()
            .map_err(|_| TranscriptionFailure::EngineUnavailable)?;
    }
    Ok(params)
}

struct PreparedCpu {
    resources: Arc<WhisperResources>,
    admission: BatchAdmission,
    capacity: Arc<SpeechPermit>,
    params: ListenParams,
    request: TranscriptionJobRequest,
    failed: Arc<AtomicBool>,
}

struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl PreparedTranscription for PreparedCpu {
    fn transcribe(
        self: Box<Self>,
        input: TranscriptionInput,
        cancel: CancellationToken,
    ) -> TranscriptionFuture<'static, Result<TranscriptWithData, TranscriptionFailure>> {
        Box::pin(async move {
            let cancel = cancel.child_token();
            let _cancel = CancelOnDrop(cancel.clone());
            // Dropping the caller's waiter requests stop; this task still owns every lease.
            tokio::spawn(run(*self, input, cancel))
                .await
                .unwrap_or(Err(TranscriptionFailure::EngineFailed))
        })
    }
}

struct NativeSource {
    _capacity: Arc<SpeechPermit>,
    _prepared: Arc<PreparedAudio>,
    _source: SourceOwner,
}

async fn run(
    prepared: PreparedCpu,
    input: TranscriptionInput,
    cancel: CancellationToken,
) -> Result<TranscriptWithData, TranscriptionFailure> {
    // Also cover preparation failures: the inner future must drop its native
    // admission before any FIFO waiter can acquire this final scheduler owner.
    retain_capacity(
        prepared.capacity.clone(),
        run_owned(prepared, input, cancel),
    )
    .await
}

async fn retain_capacity<T>(
    capacity: Arc<SpeechPermit>,
    work: impl std::future::Future<Output = T>,
) -> T {
    let result = work.await;
    drop(capacity);
    result
}

async fn run_owned(
    prepared: PreparedCpu,
    input: TranscriptionInput,
    cancel: CancellationToken,
) -> Result<TranscriptWithData, TranscriptionFailure> {
    if input.request != prepared.request || input.metadata.session_id != input.request.session_id {
        return Err(TranscriptionFailure::InvalidOutput);
    }
    let PreparedCpu {
        resources,
        admission,
        capacity,
        mut params,
        failed,
        ..
    } = prepared;
    if failed.load(Ordering::Acquire) {
        return Err(TranscriptionFailure::EngineUnavailable);
    }
    let (path, owner) = input.source.into_parts();
    // Core deliberately grants Send-only opaque ownership, not shared access to its payload.
    let owner = Arc::new(Mutex::new(owner));
    let decode_owner = owner.clone();
    let flag = Arc::new(AtomicBool::new(cancel.is_cancelled()));
    let decode_cancel = flag.clone();
    let bridge_cancel = cancel.clone();
    let decoded = CancellationToken::new();
    let decoded_done = decoded.clone();
    let bridge = tokio::spawn(async move {
        tokio::select! {
            _ = bridge_cancel.cancelled() => flag.store(true, Ordering::Release),
            _ = decoded_done.cancelled() => {},
        }
    });
    let audio = input.metadata.audio;
    let worker = tokio::task::spawn_blocking(move || {
        let _owner = decode_owner;
        let audio = PreparedAudio::from_path(&path, audio, &decode_cancel);
        (audio, admission, capacity)
    })
    .await;
    decoded.cancel();
    let _ = bridge.await;
    let (audio, admission, capacity) = worker.map_err(|_| TranscriptionFailure::EngineFailed)?;
    let audio = Arc::new(audio.map_err(|_| {
        if cancel.is_cancelled() {
            TranscriptionFailure::Interrupted
        } else {
            TranscriptionFailure::SourceUnavailable
        }
    })?);
    if cancel.is_cancelled() {
        return Err(TranscriptionFailure::Interrupted);
    }
    if failed.load(Ordering::Acquire) {
        return Err(TranscriptionFailure::EngineUnavailable);
    }
    params.channels = audio
        .pcm
        .channels
        .try_into()
        .map_err(|_| TranscriptionFailure::InvalidOutput)?;
    params.sample_rate = audio.pcm.sample_rate;
    let source = TrustedBatchSource::new(
        audio.path.clone(),
        NativeSource {
            _capacity: capacity.clone(),
            _prepared: audio.clone(),
            _source: owner.clone(),
        },
    )
    .map_err(resource_failure)?;
    let started = std::panic::catch_unwind(AssertUnwindSafe(|| {
        resources.start_batch(admission, source, params)
    }));
    let batch = match started {
        Ok(batch) => batch.map_err(resource_failure)?,
        Err(_) => retain_unconfirmed(&failed).await,
    };
    let observer = batch.observer();
    complete(
        AdmittedBatch {
            driver: NativeBatch { batch, observer },
            _capacity: capacity,
        },
        owner,
        failed,
        cancel,
        move |response| {
            let mut response = response.clone();
            audio.stamp_response(&mut response);
            let metadata = &input.metadata;
            // Materialize the legacy processing clock at the result, never at model admission.
            let mapped = transcript_from_batch_response(
                &response,
                BatchTranscriptMeta {
                    session_id: &metadata.session_id,
                    user_id: &metadata.user_id,
                    provider: "whispercpp",
                    created_at: metadata.created_at.clone(),
                    started_at_ms: metadata.started_at_ms(),
                    memo_md: metadata.memo_md.clone(),
                },
                &mut || uuid::Uuid::new_v4().to_string(),
            );
            Ok(mapped.unwrap_or_else(|| metadata.empty_transcript()))
        },
    )
    .await
}

trait BatchDriver: Send {
    fn snapshot(&self) -> BatchSnapshot;
    fn changed(&mut self) -> TranscriptionFuture<'_, Option<BatchSnapshot>>;
    fn cancel(&mut self);
}
// Native source destruction precedes release of Whisper's native admission.
// Keep the scheduler permit until Finished confirms that entire worker retired.
struct AdmittedBatch<D> {
    driver: D,
    _capacity: Arc<SpeechPermit>,
}
impl<D: BatchDriver> BatchDriver for AdmittedBatch<D> {
    fn snapshot(&self) -> BatchSnapshot {
        self.driver.snapshot()
    }
    fn changed(&mut self) -> TranscriptionFuture<'_, Option<BatchSnapshot>> {
        self.driver.changed()
    }
    fn cancel(&mut self) {
        self.driver.cancel();
    }
}

struct NativeBatch {
    batch: OwnedBatch,
    observer: BatchObserver,
}
impl BatchDriver for NativeBatch {
    fn snapshot(&self) -> BatchSnapshot {
        self.observer.snapshot()
    }
    fn changed(&mut self) -> TranscriptionFuture<'_, Option<BatchSnapshot>> {
        Box::pin(self.observer.changed())
    }
    fn cancel(&mut self) {
        self.batch.cancel();
    }
}

// Drop queued native state and the mapping closure's scratch owners before Core's
// final receipt. Early cancellation/panic returns must preserve the same order.
struct Completion<D, F> {
    driver: D,
    map: Option<F>,
    _source: SourceOwner,
}

async fn complete(
    driver: impl BatchDriver,
    source: SourceOwner,
    failed: Arc<AtomicBool>,
    cancel: CancellationToken,
    map: impl FnOnce(&batch::Response) -> Result<TranscriptWithData, TranscriptionFailure>
    + Send
    + 'static,
) -> Result<TranscriptWithData, TranscriptionFailure> {
    let mut owned = Completion {
        driver,
        map: Some(map),
        _source: source,
    };
    let mut panicked = false;
    let response = loop {
        match AssertUnwindSafe(drain(&mut owned.driver, &cancel, &failed))
            .catch_unwind()
            .await
        {
            Ok(response) => break response,
            Err(_) if !panicked => {
                panicked = true;
                if std::panic::catch_unwind(AssertUnwindSafe(|| owned.driver.cancel())).is_err() {
                    retain_unconfirmed(&failed).await;
                }
            }
            Err(_) => retain_unconfirmed(&failed).await,
        }
    };
    if panicked {
        return Err(TranscriptionFailure::EngineFailed);
    }
    if cancel.is_cancelled() {
        return Err(TranscriptionFailure::Interrupted);
    }
    let map = owned.map.take().ok_or(TranscriptionFailure::EngineFailed)?;
    let mapped = tokio::task::spawn_blocking(move || {
        let response = response
            .as_ref()
            .as_ref()
            .map_err(|cause| resource_failure(cause.clone()))?;
        std::panic::catch_unwind(AssertUnwindSafe(|| map(response)))
            .unwrap_or(Err(TranscriptionFailure::EngineFailed))
    })
    .await
    .unwrap_or(Err(TranscriptionFailure::EngineFailed));
    if cancel.is_cancelled() {
        Err(TranscriptionFailure::Interrupted)
    } else {
        mapped
    }
}

async fn drain(
    driver: &mut impl BatchDriver,
    cancel: &CancellationToken,
    failed: &AtomicBool,
) -> BatchResult {
    let mut cancellation_sent = false;
    loop {
        if let BatchSnapshot::Finished(result) = driver.snapshot() {
            return result;
        }
        let changed = tokio::select! {
            changed = driver.changed() => changed,
            _ = cancel.cancelled(), if !cancellation_sent => {
                driver.cancel(); cancellation_sent = true; continue;
            }
        };
        if let Some(BatchSnapshot::Finished(result)) = changed {
            return result;
        }
        if changed.is_none() {
            // A sender may close immediately after publishing its final persistent snapshot.
            if let BatchSnapshot::Finished(result) = driver.snapshot() {
                return result;
            }
            retain_unconfirmed(failed).await;
        }
    }
}

async fn retain_unconfirmed(failed: &AtomicBool) -> ! {
    failed.store(true, Ordering::Release);
    tracing::error!(
        "CPU transcription completion could not be confirmed; native ownership retained and further admission disabled; operator restart required"
    );
    match std::future::pending::<std::convert::Infallible>().await {}
}

fn resource_failure(error: ResourceError) -> TranscriptionFailure {
    match error {
        ResourceError::Busy => TranscriptionFailure::ResourceLimit,
        ResourceError::Cancelled => TranscriptionFailure::Interrupted,
        ResourceError::Invalid(_)
        | ResourceError::Loading
        | ResourceError::Stopping
        | ResourceError::Unavailable
        | ResourceError::ModelChanged => TranscriptionFailure::EngineUnavailable,
        ResourceError::WorkerPanicked | ResourceError::Failed(_) => {
            TranscriptionFailure::EngineFailed
        }
    }
}

fn admission_failure(cause: AdmissionError) -> TranscriptionFailure {
    match cause {
        AdmissionError::Busy => TranscriptionFailure::ResourceLimit,
        AdmissionError::Cancelled => TranscriptionFailure::Interrupted,
        AdmissionError::Stopped => TranscriptionFailure::EngineUnavailable,
    }
}
