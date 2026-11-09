//! Explicit model ownership for trusted hosts. Model paths are operator configuration,
//! never client input. Provision model storage read-only for the owner's entire lifetime:
//! before/after-load verification cannot prevent an operator modifying a mapped model later.
//! This module performs no vault authorization, transcript publication or live transport.

mod admission;
pub mod live;
#[cfg(test)]
mod tests;

pub use admission::{BatchAdmission, LiveAdmission};

use std::{
    fs::File,
    io::Read,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use owhisper_interface::{ListenParams, batch, batch_sse::BatchSseMessage};
use sha2::{Digest, Sha256};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Debug)]
pub struct ModelSpec {
    pub absolute_path: PathBuf,
    pub sha256: String,
    pub size_bytes: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// One group is one batch or one live capture with one/two channels. Increasing
    /// this can multiply native CPU and session memory; it is an operator decision.
    pub workload_groups: usize,
    pub progress_capacity: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            workload_groups: 1,
            progress_capacity: 16,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ResourceError {
    #[error("invalid {0}")]
    Invalid(&'static str),
    #[error("model is loading")]
    Loading,
    #[error("native capacity is occupied")]
    Busy,
    #[error("resources are stopping")]
    Stopping,
    #[error("model is unavailable")]
    Unavailable,
    #[error("model identity changed")]
    ModelChanged,
    #[error("operation cancelled")]
    Cancelled,
    #[error("native worker panicked")]
    WorkerPanicked,
    #[error("{0}")]
    Failed(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourcePhase {
    Loading,
    Ready,
    Failed(ResourceError),
    Stopping,
    Stopped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResourceSnapshot {
    pub phase: ResourcePhase,
    pub batch_groups: usize,
    pub live_groups: usize,
    pub live_channels: usize,
}

#[derive(Clone)]
pub struct ResourceObserver(watch::Receiver<ResourceSnapshot>);

impl ResourceObserver {
    pub fn snapshot(&self) -> ResourceSnapshot {
        self.0.borrow().clone()
    }

    /// Channel closure without Stopped is not resource-release evidence.
    pub async fn changed(&mut self) -> Option<ResourceSnapshot> {
        self.0.changed().await.ok()?;
        Some(self.snapshot())
    }
}

pub struct WhisperResources {
    inner: Arc<Inner>,
}

struct Inner {
    runtime: tokio::runtime::Handle,
    limits: Limits,
    state: Mutex<State>,
    status: watch::Sender<ResourceSnapshot>,
}

struct State {
    phase: ResourcePhase,
    engine: Option<Arc<dyn Engine>>,
    loading: bool,
    stopping: bool,
    releasing: bool,
    batch_groups: usize,
    live_groups: usize,
    live_channels: usize,
    cancellations: Vec<Weak<AtomicBool>>,
}

trait Engine: Send + Sync {
    fn live(
        &self,
        _params: ListenParams,
        _cancelled: Arc<AtomicBool>,
    ) -> Result<crate::service::live_worker::OwnedWorker, ResourceError> {
        Err(ResourceError::Unavailable)
    }

    fn metadata(&self) -> owhisper_interface::stream::Metadata {
        Default::default()
    }

    fn batch(
        &self,
        source: &TrustedBatchSource,
        params: &ListenParams,
        cancelled: Arc<AtomicBool>,
        emit: &mut dyn FnMut(BatchSseMessage) -> Result<(), crate::Error>,
    ) -> Result<batch::Response, crate::Error>;
}

struct CpuEngine {
    model: Arc<hypr_whisper_local::LoadedWhisper>,
    path: PathBuf,
    _file: File,
    _owner: Option<Box<dyn Send + Sync>>,
}

impl Engine for CpuEngine {
    fn live(
        &self,
        params: ListenParams,
        cancelled: Arc<AtomicBool>,
    ) -> Result<crate::service::live_worker::OwnedWorker, ResourceError> {
        crate::service::live_worker::start_owned(self.model.clone(), params, cancelled)
            .map_err(failed)
    }

    fn metadata(&self) -> owhisper_interface::stream::Metadata {
        crate::service::build_metadata(&self.path)
    }

    fn batch(
        &self,
        source: &TrustedBatchSource,
        params: &ListenParams,
        cancelled: Arc<AtomicBool>,
        emit: &mut dyn FnMut(BatchSseMessage) -> Result<(), crate::Error>,
    ) -> Result<batch::Response, crate::Error> {
        crate::transcribe_batch_file(
            &self.model,
            &self.path,
            &source.path,
            params,
            cancelled,
            emit,
        )
    }
}

/// The host supplies the source's real lease/owner. Lifetime retention is not source
/// verification: the host still owns recording hash/revision and authorization checks.
pub struct TrustedBatchSource {
    path: PathBuf,
    _owner: Box<dyn Send>,
}

impl TrustedBatchSource {
    pub fn new(path: PathBuf, owner: impl Send + 'static) -> Result<Self, ResourceError> {
        if !path.is_absolute() {
            return Err(ResourceError::Invalid("source path"));
        }
        Ok(Self {
            path,
            _owner: Box::new(owner),
        })
    }
}

#[derive(Clone, Debug)]
pub enum BatchSnapshot {
    Running,
    Finished(Arc<Result<batch::Response, ResourceError>>),
}

#[derive(Clone)]
pub struct BatchObserver(watch::Receiver<BatchSnapshot>);

impl BatchObserver {
    pub fn snapshot(&self) -> BatchSnapshot {
        self.0.borrow().clone()
    }

    /// Channel closure without Finished is not native-completion evidence.
    pub async fn changed(&mut self) -> Option<BatchSnapshot> {
        self.0.changed().await.ok()?;
        Some(self.snapshot())
    }
}

pub struct OwnedBatch {
    /// Optional, bounded progress. Full/closed queues discard notifications;
    /// obtain the complete authoritative response from the completion observer.
    pub events: mpsc::Receiver<BatchSseMessage>,
    completion: BatchObserver,
    cancellation: Arc<AtomicBool>,
}

impl OwnedBatch {
    pub fn observer(&self) -> BatchObserver {
        self.completion.clone()
    }

    pub fn cancel(&self) {
        self.cancellation.store(true, Ordering::Release);
    }
}

impl Drop for OwnedBatch {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl WhisperResources {
    pub fn start_cpu(spec: ModelSpec, limits: Limits) -> Result<Self, ResourceError> {
        Self::start_verified(spec, limits, Some(false), None)
    }

    /// Native desktop policy: use the compiled Metal/Vulkan accelerator when available.
    pub fn start_device(spec: ModelSpec, limits: Limits) -> Result<Self, ResourceError> {
        Self::start_verified(spec, limits, None, None)
    }

    pub fn start_device_owned(
        spec: ModelSpec,
        limits: Limits,
        owner: Box<dyn Send + Sync>,
    ) -> Result<Self, ResourceError> {
        Self::start_verified(spec, limits, None, Some(owner))
    }

    fn start_verified(
        spec: ModelSpec,
        limits: Limits,
        use_gpu: Option<bool>,
        owner: Option<Box<dyn Send + Sync>>,
    ) -> Result<Self, ResourceError> {
        validate(&spec, limits)?;
        Self::start(limits, move || {
            let (model, file) = load_verified(&spec, || {
                let path = spec
                    .absolute_path
                    .to_str()
                    .ok_or(ResourceError::Invalid("model path"))?;
                let builder = hypr_whisper_local::LoadedWhisper::builder().model_path(path);
                let builder = match use_gpu {
                    Some(gpu) => builder.use_gpu(gpu),
                    None => builder,
                };
                builder.build().map_err(failed)
            })?;
            Ok(Arc::new(CpuEngine {
                model: Arc::new(model),
                path: spec.absolute_path,
                _file: file,
                _owner: owner,
            }))
        })
    }

    fn start(
        limits: Limits,
        loader: impl FnOnce() -> Result<Arc<dyn Engine>, ResourceError> + Send + 'static,
    ) -> Result<Self, ResourceError> {
        let runtime =
            tokio::runtime::Handle::try_current().map_err(|_| ResourceError::Invalid("runtime"))?;
        let initial = ResourceSnapshot {
            phase: ResourcePhase::Loading,
            batch_groups: 0,
            live_groups: 0,
            live_channels: 0,
        };
        let (status, _) = watch::channel(initial);
        let inner = Arc::new(Inner {
            runtime: runtime.clone(),
            limits,
            status,
            state: Mutex::new(State {
                phase: ResourcePhase::Loading,
                engine: None,
                loading: true,
                stopping: false,
                releasing: false,
                batch_groups: 0,
                live_groups: 0,
                live_channels: 0,
                cancellations: Vec::new(),
            }),
        });
        let owned = inner.clone();
        let worker = runtime.spawn_blocking(loader);
        runtime.spawn(async move {
            let result = worker.await.unwrap_or(Err(ResourceError::WorkerPanicked));
            {
                let mut state = owned.lock();
                state.loading = false;
                match result {
                    Ok(engine) => {
                        state.engine = Some(engine);
                        if !state.stopping {
                            state.phase = ResourcePhase::Ready;
                        }
                    }
                    Err(error) if !state.stopping => state.phase = ResourcePhase::Failed(error),
                    Err(_) => {}
                }
                owned.publish(&state);
            }
            owned.maybe_release();
        });
        Ok(Self { inner })
    }

    pub fn observer(&self) -> ResourceObserver {
        ResourceObserver(self.inner.status.subscribe())
    }

    pub fn shutdown(&self) -> ResourceObserver {
        self.inner.stop();
        self.observer()
    }

    pub fn start_batch(
        &self,
        admission: BatchAdmission,
        source: TrustedBatchSource,
        params: ListenParams,
    ) -> Result<OwnedBatch, ResourceError> {
        if !Arc::ptr_eq(&admission.0.inner, &self.inner) {
            return Err(ResourceError::Invalid("admission owner"));
        }
        let engine = {
            let state = self.inner.lock();
            if state.stopping {
                return Err(ResourceError::Stopping);
            }
            state.engine.clone().ok_or(ResourceError::Unavailable)?
        };
        let cancellation = admission.0.cancellation.clone();
        let owned_cancel = cancellation.clone();
        let runtime = self.inner.runtime.clone();
        let (events, event_rx) = mpsc::channel(self.inner.limits.progress_capacity);
        let (completed, completion) = watch::channel(BatchSnapshot::Running);
        let worker = runtime.spawn_blocking(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if owned_cancel.load(Ordering::Acquire) {
                    return Err(ResourceError::Cancelled);
                }
                let mut emit = |event| {
                    if owned_cancel.load(Ordering::Acquire) {
                        return Err(crate::Error::protocol("batch cancelled"));
                    }
                    // The complete terminal response is authoritative; progress must never
                    // retain native capacity just because an observer stopped consuming it.
                    let _ = events.try_send(event);
                    Ok(())
                };
                let result = engine.batch(&source, &params, owned_cancel.clone(), &mut emit);
                if owned_cancel.load(Ordering::Acquire) {
                    Err(ResourceError::Cancelled)
                } else {
                    result.map_err(failed)
                }
            }))
            .unwrap_or(Err(ResourceError::WorkerPanicked));
            drop(source);
            drop(engine);
            drop(admission);
            result
        });
        runtime.spawn(async move {
            let result = worker.await.unwrap_or(Err(ResourceError::WorkerPanicked));
            completed.send_replace(BatchSnapshot::Finished(Arc::new(result)));
        });
        Ok(OwnedBatch {
            events: event_rx,
            completion: BatchObserver(completion),
            cancellation,
        })
    }
}

impl Drop for WhisperResources {
    fn drop(&mut self) {
        self.inner.stop();
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn publish(&self, state: &State) {
        self.status.send_replace(ResourceSnapshot {
            phase: state.phase.clone(),
            batch_groups: state.batch_groups,
            live_groups: state.live_groups,
            live_channels: state.live_channels,
        });
    }

    fn stop(self: &Arc<Self>) {
        {
            let mut state = self.lock();
            if state.stopping {
                return;
            }
            state.stopping = true;
            state.phase = ResourcePhase::Stopping;
            for cancellation in state.cancellations.iter().filter_map(Weak::upgrade) {
                cancellation.store(true, Ordering::Release);
            }
            self.publish(&state);
        }
        self.maybe_release();
    }

    fn maybe_release(self: &Arc<Self>) {
        let engine = {
            let mut state = self.lock();
            if !state.stopping
                || state.loading
                || state.releasing
                || state.batch_groups + state.live_groups != 0
            {
                return;
            }
            state.releasing = true;
            state.engine.take()
        };
        let owned = self.clone();
        let worker = self.runtime.spawn_blocking(move || drop(engine));
        self.runtime.spawn(async move {
            let result = worker.await;
            let mut state = owned.lock();
            state.phase = if result.is_ok() {
                ResourcePhase::Stopped
            } else {
                ResourcePhase::Failed(ResourceError::WorkerPanicked)
            };
            owned.publish(&state);
        });
    }
}

fn validate(spec: &ModelSpec, limits: Limits) -> Result<(), ResourceError> {
    if !(1..=8).contains(&limits.workload_groups) || !(1..=64).contains(&limits.progress_capacity) {
        return Err(ResourceError::Invalid("resource limits"));
    }
    if !spec.absolute_path.is_absolute() || spec.absolute_path.to_str().is_none() {
        return Err(ResourceError::Invalid("model path"));
    }
    if spec.sha256.len() != 64
        || !spec
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ResourceError::Invalid("model digest"));
    }
    if spec.size_bytes == 0 || spec.size_bytes > 16 * 1024 * 1024 * 1024 {
        return Err(ResourceError::Invalid("model size"));
    }
    Ok(())
}

fn verify_model(spec: &ModelSpec) -> Result<File, ResourceError> {
    let metadata = std::fs::symlink_metadata(&spec.absolute_path).map_err(failed)?;
    if !metadata.file_type().is_file() || metadata.len() != spec.size_bytes {
        return Err(ResourceError::ModelChanged);
    }
    let mut file = File::open(&spec.absolute_path).map_err(failed)?;
    let opened = file.metadata().map_err(failed)?;
    if !opened.is_file() || opened.len() != spec.size_bytes {
        return Err(ResourceError::ModelChanged);
    }
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    let mut read = 0_u64;
    loop {
        let length = file.read(&mut buffer).map_err(failed)?;
        if length == 0 {
            break;
        }
        read = read
            .checked_add(length as u64)
            .ok_or(ResourceError::ModelChanged)?;
        if read > spec.size_bytes {
            return Err(ResourceError::ModelChanged);
        }
        hash.update(&buffer[..length]);
    }
    if read != spec.size_bytes || digest_hex(hash.finalize().as_ref()) != spec.sha256 {
        return Err(ResourceError::ModelChanged);
    }
    Ok(file)
}

fn digest_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 15)]));
    }
    output
}

fn load_verified<T>(
    spec: &ModelSpec,
    load: impl FnOnce() -> Result<T, ResourceError>,
) -> Result<(T, File), ResourceError> {
    let file = verify_model(spec)?;
    let model = load()?;
    verify_model(spec)?;
    Ok((model, file))
}

fn failed(error: impl std::fmt::Display) -> ResourceError {
    ResourceError::Failed(error.to_string().chars().take(512).collect())
}
