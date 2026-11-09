//! Typed capture PCM input. This is not authenticated admission, durable ingestion,
//! canonical transcript publication or a replacement for the local s16 WebSocket API.

mod ingress;
mod snapshot;
#[cfg(test)]
mod tests;

pub use ingress::{FrameReceipt, LiveIngressError};
pub use snapshot::{LiveCaption, LiveObserver, LiveSnapshot, LiveStatus};

use super::*;
use crate::service::{
    live_pipeline::{EvidenceWindow, Pipeline, evidence, send},
    live_worker::{Job, Output, OwnedWorker},
};
use futures_util::FutureExt;
use ingress::{Frame, Ingress, Phase};
use std::sync::atomic::AtomicU8;
use std::time::Duration;
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

const MAX_FRAME: u64 = (1_u64 << 53) - 1;
const RESULT_CAPACITY: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChannelRole {
    Microphone,
    SystemAudio,
    Mixed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveChannels {
    Microphone,
    SystemAudio,
    MicSystem,
    MixedMono,
}

impl LiveChannels {
    pub fn count(self) -> usize {
        if self == Self::MicSystem { 2 } else { 1 }
    }
    pub fn role(self, physical: usize) -> Option<ChannelRole> {
        match (self, physical) {
            (Self::Microphone | Self::MicSystem, 0) => Some(ChannelRole::Microphone),
            (Self::SystemAudio, 0) | (Self::MicSystem, 1) => Some(ChannelRole::SystemAudio),
            (Self::MixedMono, 0) => Some(ChannelRole::Mixed),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct LiveSpec {
    pub generation: [u8; 16],
    pub start_frame: u64,
    pub channels: LiveChannels,
    pub params: ListenParams,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveFailure {
    Cancelled,
    Initialization(String),
    Worker(String),
    WorkerPanicked,
    OutputLost,
    InvalidOutput,
    InvalidDrain,
    OwnerReleaseFailed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// Native/owner completion only. Core must reconcile received captions with
/// caption_count and prove their persistence separately before canonical promotion.
pub struct LiveDrainReceipt {
    pub generation: [u8; 16],
    pub start_frame: u64,
    pub admitted_end_frame: u64,
    pub input_consumed_end_frame: u64,
    pub caption_count: u64,
    pub failure: Option<LiveFailure>,
    pub native: NativeDisposition,
    pub owners_released: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeDisposition {
    NeverStarted,
    Joined,
    Unconfirmed,
}

#[derive(Clone, Debug)]
pub enum LiveCompletion {
    Pending,
    Finished(Arc<LiveDrainReceipt>),
}

#[derive(Clone)]
pub struct LiveCompletionObserver(watch::Receiver<LiveCompletion>);

impl LiveCompletionObserver {
    pub fn snapshot(&self) -> LiveCompletion {
        self.0.borrow().clone()
    }
    /// Channel closure without a receipt proves neither native nor owner release.
    pub async fn changed(&mut self) -> Option<LiveCompletion> {
        self.0.changed().await.ok()?;
        Some(self.snapshot())
    }
}

pub struct OwnedLive {
    /// Every caption is essential. Core must drain and persist this independently
    /// of UI watches; a Full/Closed queue interrupts processing explicitly.
    pub results: mpsc::Receiver<LiveCaption>,
    frames: mpsc::Sender<Frame>,
    shared: Arc<Shared>,
    completion: LiveCompletionObserver,
}

#[derive(Clone)]
pub struct LiveController {
    frames: mpsc::Sender<Frame>,
    shared: Arc<Shared>,
}

impl LiveController {
    pub fn cancel(&self) {
        self.shared.fail(LiveFailure::Cancelled);
    }
}

struct Shared {
    spec: LiveSpec,
    state: Mutex<Ingress>,
    budgets: Vec<Arc<tokio::sync::Semaphore>>,
    status: watch::Sender<Arc<LiveSnapshot>>,
    native_cancelled: Arc<AtomicBool>,
    native_proof: Arc<AtomicU8>,
    cancelled: CancellationToken,
    control: Notify,
}

impl OwnedLive {
    pub fn controller(&self) -> LiveController {
        LiveController {
            frames: self.frames.clone(),
            shared: self.shared.clone(),
        }
    }
    pub fn observer(&self) -> LiveObserver {
        LiveObserver(self.shared.status.subscribe())
    }
    pub fn completion(&self) -> LiveCompletionObserver {
        self.completion.clone()
    }
    pub fn cancel(&self) {
        self.shared.fail(LiveFailure::Cancelled);
    }
}

impl Drop for OwnedLive {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl Shared {
    fn fail(&self, failure: LiveFailure) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.phase == Phase::Terminal {
            return;
        }
        state.failure.get_or_insert(failure);
        state.phase = Phase::Stopping;
        self.native_cancelled.store(true, Ordering::Release);
        self.cancelled.cancel();
        self.publish(&state);
    }
}

impl WhisperResources {
    pub fn start_live(
        &self,
        admission: LiveAdmission,
        spec: LiveSpec,
        source_owner: impl Send + 'static,
    ) -> Result<OwnedLive, ResourceError> {
        use super::admission::Kind;
        if !Arc::ptr_eq(&admission.0.inner, &self.inner) {
            return Err(ResourceError::Invalid("admission owner"));
        }
        if !matches!(admission.0.kind, Kind::Live(count) if usize::from(count) == spec.channels.count())
            || usize::from(spec.params.channels) != spec.channels.count()
            || spec.params.sample_rate != 16000
            || spec.generation == [0; 16]
            || spec.start_frame > MAX_FRAME
            || usize::try_from(spec.start_frame).is_err()
        {
            return Err(ResourceError::Invalid("live specification"));
        }
        let engine = {
            let state = self.inner.lock();
            if state.stopping {
                return Err(ResourceError::Stopping);
            }
            state.engine.clone().ok_or(ResourceError::Unavailable)?
        };
        let start = spec.start_frame;
        let (status, _) = watch::channel(Arc::new(LiveSnapshot {
            generation: spec.generation,
            start_frame: start,
            admitted_end_frame: start,
            input_consumed_end_frame: start,
            completed_through_frame: None,
            status: LiveStatus::Starting,
            first_sequence: 0,
            next_sequence: 0,
            captions: Vec::new(),
        }));
        let shared = Arc::new(Shared {
            budgets: ingress::budgets(spec.channels.count()),
            spec,
            state: Mutex::new(Ingress::new(start)),
            status,
            native_cancelled: admission.0.cancellation.clone(),
            native_proof: Arc::new(AtomicU8::new(0)),
            cancelled: CancellationToken::new(),
            control: Notify::new(),
        });
        let (frames, receiver) = mpsc::channel(ingress::INPUT_BLOCKS);
        let (results, result_rx) = mpsc::channel(RESULT_CAPACITY);
        let (completed, completion) = watch::channel(LiveCompletion::Pending);
        let owned = shared.clone();
        let runtime = self.inner.runtime.clone();
        runtime.spawn(run(
            owned,
            receiver,
            results,
            admission,
            engine,
            Box::new(source_owner),
            completed,
        ));
        Ok(OwnedLive {
            results: result_rx,
            frames,
            shared,
            completion: LiveCompletionObserver(completion),
        })
    }
}

struct Prepared {
    worker: WorkerGuard,
    pipeline: Pipeline,
    speech: tokio::sync::oneshot::Receiver<()>,
    observations: crate::service::live_pipeline::Evidence,
}

struct WorkerGuard {
    jobs: mpsc::Sender<Job>,
    output: Option<mpsc::Receiver<Output>>,
    initialized: Option<tokio::sync::oneshot::Receiver<Result<(), String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
    cancelled: Arc<AtomicBool>,
    proof: Arc<AtomicU8>,
}

impl WorkerGuard {
    fn new(worker: OwnedWorker, shared: &Shared) -> Self {
        shared.native_proof.store(1, Ordering::Release);
        Self {
            jobs: worker.jobs,
            output: Some(worker.output),
            initialized: Some(worker.initialized),
            thread: Some(worker.thread),
            cancelled: shared.native_cancelled.clone(),
            proof: shared.native_proof.clone(),
        }
    }

    fn join_inner(&mut self) -> bool {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.jobs.try_send(Job::Deadline);
        drop(self.output.take());
        drop(self.initialized.take());
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            self.proof.store(2, Ordering::Release);
            result.is_ok()
        } else {
            true
        }
    }

    fn join(mut self) -> bool {
        self.join_inner()
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        // Startup can unwind before the async supervisor receives the worker.
        // Dropping a raw JoinHandle would detach live native work from its owners.
        let _ = self.join_inner();
    }
}

fn prepare(engine: Arc<dyn Engine>, shared: Arc<Shared>) -> Result<Prepared, LiveFailure> {
    if shared.native_cancelled.load(Ordering::Acquire) {
        return Err(LiveFailure::Cancelled);
    }
    let worker = engine
        .live(shared.spec.params.clone(), shared.native_cancelled.clone())
        .map_err(|error| LiveFailure::Initialization(error.to_string()))?;
    let worker = WorkerGuard::new(worker, &shared);
    let (started, speech) = tokio::sync::oneshot::channel();
    let count = shared.spec.channels.count();
    let observations = Arc::new(Mutex::new(EvidenceWindow {
        channels: vec![Vec::new(); count],
        end: 0,
        cutoff: 0,
    }));
    match Pipeline::new(
        worker.jobs.clone(),
        count,
        crate::service::redemption_time(&shared.spec.params),
        started,
        observations.clone(),
        shared.spec.start_frame as usize,
    ) {
        Ok(pipeline) => Ok(Prepared {
            worker,
            pipeline,
            speech,
            observations,
        }),
        Err(error) => Err(LiveFailure::Initialization(error)),
    }
}

async fn producer(
    shared: Arc<Shared>,
    mut frames: mpsc::Receiver<Frame>,
    mut pipeline: Pipeline,
    speech: tokio::sync::oneshot::Receiver<()>,
    observations: crate::service::live_pipeline::Evidence,
    jobs: mpsc::Sender<Job>,
    startup_deadline: Duration,
) -> Result<(), String> {
    let deadline = async {
        if speech.await.is_ok() {
            tokio::time::sleep(startup_deadline).await;
            evidence(&jobs, &observations).await?;
            send(&jobs, || Job::Deadline).await?;
        }
        Ok::<_, String>(())
    };
    tokio::pin!(deadline);
    let input = async {
        loop {
            let notified = shared.control.notified();
            if shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .phase
                == Phase::Finishing
            {
                frames.close();
            }
            let frame = tokio::select! {
                _ = notified => continue,
                frame = frames.recv() => frame,
            };
            let Some(frame) = frame else {
                break;
            };
            let count = shared.spec.channels.count();
            let end = frame.first + (frame.samples.len() / count) as u64;
            let mut audio = vec![Vec::with_capacity(frame.samples.len() / count); count];
            for physical in frame.samples.chunks_exact(count) {
                for (channel, sample) in physical.iter().enumerate() {
                    audio[channel].push(*sample);
                }
            }
            pipeline.push(audio, frame.permits).await?;
            let consumed = shared.spec.start_frame + pipeline.consumed_frames() as u64;
            if consumed != end {
                return Err("live input coverage diverged".into());
            }
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            state.consumed_end = consumed;
            shared.publish(&state);
        }
        {
            let state = shared
                .state
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if state.phase != Phase::Finishing || state.finish_end != Some(state.consumed_end) {
                return Err("live input ended without a matching finish barrier".into());
            }
        }
        pipeline.finish(true).await
    };
    tokio::pin!(input);
    tokio::select! {
        _ = shared.cancelled.cancelled() => Err("live input cancelled".into()),
        result = &mut input => result,
        result = &mut deadline => {
            result?;
            tokio::select! {
                _ = shared.cancelled.cancelled() => Err("live input cancelled".into()),
                result = input => result,
            }
        },
    }
}

async fn resource_cancel(shared: &Shared, status: &mut watch::Receiver<ResourceSnapshot>) {
    loop {
        if shared.native_cancelled.load(Ordering::Acquire) {
            return;
        }
        if status.changed().await.is_err() {
            return;
        }
    }
}

async fn drive(
    shared: &Arc<Shared>,
    receiver: mpsc::Receiver<Frame>,
    results: &mpsc::Sender<LiveCaption>,
    engine: &Arc<dyn Engine>,
    resource_status: &mut watch::Receiver<ResourceSnapshot>,
    native_worker: &mut Option<WorkerGuard>,
    input: &mut Option<tokio::task::JoinHandle<Result<(), String>>>,
) {
    let init_engine = engine.clone();
    let init_shared = shared.clone();
    let prepared = tokio::task::spawn_blocking(move || prepare(init_engine, init_shared)).await;
    match prepared {
        Ok(Ok(Prepared {
            worker,
            pipeline,
            speech,
            observations,
        })) => {
            *native_worker = Some(worker);
            let worker = native_worker.as_mut().unwrap();
            let initialized = worker.initialized.take().unwrap();
            let initialized = tokio::select! {
                result = initialized => result.unwrap_or(Err("native initialization acknowledgement lost".into())),
                _ = shared.cancelled.cancelled() => Err("live initialization cancelled".into()),
                _ = resource_cancel(shared, resource_status) => Err("live resources stopping".into()),
            };
            if let Err(error) = initialized {
                shared.fail(if shared.native_cancelled.load(Ordering::Acquire) {
                    LiveFailure::Cancelled
                } else {
                    LiveFailure::Initialization(error)
                });
                drop(worker.output.take());
                drop(pipeline);
            } else {
                {
                    let mut state = shared
                        .state
                        .lock()
                        .unwrap_or_else(|error| error.into_inner());
                    if state.phase == Phase::Starting {
                        state.phase = Phase::Open;
                    }
                    shared.publish(&state);
                }
                let metadata = engine.metadata();
                *input = Some(tokio::spawn(producer(
                    shared.clone(),
                    receiver,
                    pipeline,
                    speech,
                    observations,
                    worker.jobs.clone(),
                    Duration::from_secs(15),
                )));
                let mut finished = None;
                loop {
                    tokio::select! {
                        biased;
                        _ = shared.cancelled.cancelled() => break,
                        _ = resource_cancel(shared, resource_status) => { shared.fail(LiveFailure::Cancelled); break; }
                        result = async { input.as_mut().unwrap().await }, if input.is_some() => {
                            *input = None;
                            match result {
                                Ok(Ok(())) => {},
                                Ok(Err(error)) => { shared.fail(LiveFailure::Worker(error)); break; }
                                Err(_) => { shared.fail(LiveFailure::WorkerPanicked); break; }
                            }
                        }
                        message = worker.output.as_mut().unwrap().recv() => match message {
                            Some(Output::Segment(channel, segment, finalized)) => {
                                if let Err(error) = shared.caption(channel, &segment, finalized, &metadata, results) { shared.fail(error); break; }
                                tokio::task::yield_now().await;
                            }
                            Some(Output::Finished { duration, finalized }) => {
                                if !valid_finish(shared, duration, finalized) { shared.fail(LiveFailure::InvalidDrain); }
                                finished = Some((duration, finalized)); break;
                            }
                            Some(Output::Error(error)) => { shared.fail(LiveFailure::Worker(error)); break; }
                            None => { shared.fail(LiveFailure::InvalidDrain); break; }
                        }
                    }
                }
                drop(worker.output.take());
                if finished.is_none() {
                    shared.fail(LiveFailure::InvalidDrain);
                }
            }
        }
        Ok(Err(error)) => shared.fail(error),
        Err(_) => shared.fail(LiveFailure::WorkerPanicked),
    }
}

fn valid_finish(shared: &Shared, duration: f64, finalized: bool) -> bool {
    let state = shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    finalized
        && duration == (state.admitted_end - shared.spec.start_frame) as f64 / 16000.0
        && state.finish_end == Some(state.admitted_end)
        && state.consumed_end == state.admitted_end
        && state.phase == Phase::Finishing
}

async fn run(
    shared: Arc<Shared>,
    receiver: mpsc::Receiver<Frame>,
    results: mpsc::Sender<LiveCaption>,
    admission: LiveAdmission,
    engine: Arc<dyn Engine>,
    source_owner: Box<dyn Send>,
    completed: watch::Sender<LiveCompletion>,
) {
    let mut resource_status = admission.0.inner.status.subscribe();
    let mut native_worker = None;
    let mut input = None;
    let outcome = std::panic::AssertUnwindSafe(drive(
        &shared,
        receiver,
        &results,
        &engine,
        &mut resource_status,
        &mut native_worker,
        &mut input,
    ))
    .catch_unwind()
    .await;
    if outcome.is_err() {
        shared.fail(LiveFailure::WorkerPanicked);
    }
    if let Some(worker) = native_worker.as_mut() {
        drop(worker.output.take());
    }
    if let Some(input) = input {
        match input.await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => shared.fail(LiveFailure::Worker(error)),
            Err(_) => shared.fail(LiveFailure::WorkerPanicked),
        }
    }
    let released = Arc::new(AtomicBool::new(false));
    let released_proof = released.clone();
    let cleanup = tokio::task::spawn_blocking(move || {
        let native_ok = native_worker.is_none_or(WorkerGuard::join);
        drop(source_owner);
        drop(engine);
        drop(admission);
        released_proof.store(true, Ordering::Release);
        native_ok
    })
    .await;
    let mut state = shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match cleanup {
        Ok(true) => {}
        Ok(false) => {
            state.failure.get_or_insert(LiveFailure::WorkerPanicked);
        }
        Err(_) => {
            state.failure.get_or_insert(LiveFailure::OwnerReleaseFailed);
        }
    }
    if state.failure.is_none() {
        state.window.completed = Some(state.admitted_end);
    }
    state.phase = Phase::Terminal;
    shared.publish(&state);
    let receipt = LiveDrainReceipt {
        generation: shared.spec.generation,
        start_frame: shared.spec.start_frame,
        admitted_end_frame: state.admitted_end,
        input_consumed_end_frame: state.consumed_end,
        caption_count: state.window.next_sequence,
        failure: state.failure.clone(),
        native: match shared.native_proof.load(Ordering::Acquire) {
            0 => NativeDisposition::NeverStarted,
            2 => NativeDisposition::Joined,
            _ => NativeDisposition::Unconfirmed,
        },
        owners_released: released.load(Ordering::Acquire),
    };
    completed.send_replace(LiveCompletion::Finished(Arc::new(receipt)));
}
