use std::collections::VecDeque;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::{panic::AssertUnwindSafe, panic::catch_unwind};

use futures_util::{FutureExt, Stream, StreamExt};
use hypr_aec::AEC;
use hypr_audio_sync::{SyncProbe, SyncProbeConfig, SyncProbeEvent, SyncProbeState};
use hypr_resampler::ResampleExtDynamicNew;
use tokio::task::JoinHandle;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::sync::CancellationToken;

use hypr_audio::{
    CaptureDrainControl, CaptureDrainError, CaptureDrainFuture, CaptureDrainReceipt, CaptureFrame,
    CaptureStream, Error,
};

use crate::mic::MicInput;
use crate::speaker::SpeakerInput;

use super::joiner::Joiner;
use super::recovery::CaptureHealth;

#[cfg(test)]
mod ownership_tests;

pub(crate) trait ChunkSource:
    Stream<Item = Result<Vec<f32>, hypr_resampler::Error>> + Send
{
    fn request_stop(self: Pin<&mut Self>) -> bool;
}
pub(crate) type ChunkStream = Pin<Box<dyn ChunkSource>>;

struct ResampledChunks<S: hypr_audio_interface::AsyncSource>(
    hypr_resampler::ResamplerDynamicNew<S>,
);
impl<S: hypr_audio_interface::AsyncSource + Send + Unpin> Stream for ResampledChunks<S> {
    type Item = Result<Vec<f32>, hypr_resampler::Error>;
    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().0).poll_next(cx)
    }
}
impl<S: hypr_audio_interface::AsyncSource + Send + Unpin> ChunkSource for ResampledChunks<S> {
    fn request_stop(self: Pin<&mut Self>) -> bool {
        self.get_mut().0.request_stop_source()
    }
}

#[cfg(test)]
pub(crate) fn boxed_chunks<S>(stream: S) -> ChunkStream
where
    S: Stream<Item = Result<Vec<f32>, hypr_resampler::Error>> + Send + Unpin + 'static,
{
    struct Plain<S>(S);
    impl<S: Stream + Send + Unpin> Stream for Plain<S> {
        type Item = S::Item;
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            Pin::new(&mut self.get_mut().0).poll_next(cx)
        }
    }
    impl<S: Stream<Item = Result<Vec<f32>, hypr_resampler::Error>> + Send + Unpin> ChunkSource
        for Plain<S>
    {
        fn request_stop(self: Pin<&mut Self>) -> bool {
            false
        }
    }
    Box::pin(Plain(stream))
}

#[cfg(test)]
#[derive(Default)]
pub(crate) struct QueuedProbe {
    pub(crate) stopped: std::sync::atomic::AtomicBool,
    pub(crate) chunks: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
pub(crate) fn queued_chunks(
    capacity: usize,
) -> (
    tokio::sync::mpsc::Sender<Result<Vec<f32>, hypr_resampler::Error>>,
    ChunkStream,
    Arc<QueuedProbe>,
) {
    struct Queued {
        rx: tokio::sync::mpsc::Receiver<Result<Vec<f32>, hypr_resampler::Error>>,
        stopped: Arc<QueuedProbe>,
    }
    impl Stream for Queued {
        type Item = Result<Vec<f32>, hypr_resampler::Error>;
        fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            let state = self.get_mut();
            let item = state.rx.poll_recv(cx);
            if matches!(item, Poll::Ready(Some(_))) {
                state
                    .stopped
                    .chunks
                    .fetch_add(1, std::sync::atomic::Ordering::Release);
            }
            item
        }
    }
    impl ChunkSource for Queued {
        fn request_stop(self: Pin<&mut Self>) -> bool {
            let state = self.get_mut();
            state.rx.close();
            state
                .stopped
                .stopped
                .store(true, std::sync::atomic::Ordering::Release);
            true
        }
    }
    let (tx, rx) = tokio::sync::mpsc::channel(capacity);
    let stopped = Arc::new(QueuedProbe::default());
    (
        tx,
        Box::pin(Queued {
            rx,
            stopped: stopped.clone(),
        }),
        stopped,
    )
}

const AUDIO_SYNC_PROBE_ENV: &str = "AUDIO_SYNC_PROBE";
const AEC_MAX_REFERENCE_LAG_MS: u32 = 100;
const AEC_MIN_REFERENCE_RMS: f32 = 1e-4;
const AEC_MIN_MIC_RMS: f32 = 1e-4;
const AEC_MIN_REFERENCE_CORRELATION: f32 = 0.12;
const AEC_MAX_LINEAR_GAIN: f32 = 1.25;
const AEC_LINEAR_GAIN_SMOOTHING: f32 = 0.12;
const AEC_DOUBLE_TALK_RESIDUAL_RATIO: f32 = 0.08;
const AEC_ROBUST_GAIN_SEGMENTS: usize = 8;
const AEC_ROBUST_GAIN_MIN_SEGMENTS: usize = 3;

struct NativeControl {
    stop: CancellationToken,
    receipt: tokio::sync::watch::Receiver<Option<Result<CaptureDrainReceipt, CaptureDrainError>>>,
}
impl CaptureDrainControl for NativeControl {
    fn request_stop(&self) -> Result<(), CaptureDrainError> {
        if !self.stop.is_cancelled() && self.receipt.borrow().is_some() {
            return Err(CaptureDrainError::NotDrained);
        }
        self.stop.cancel();
        Ok(())
    }
    fn completion(&self) -> CaptureDrainFuture<'_> {
        let mut receipt = self.receipt.clone();
        Box::pin(async move {
            loop {
                if let Some(result) = receipt.borrow().clone() {
                    return result;
                }
                receipt.changed().await.map_err(|_| {
                    CaptureDrainError::Failed(
                        "Native capture task ended without resource acknowledgement".into(),
                    )
                })?;
            }
        })
    }
}

fn spawn_native(
    future: impl std::future::Future<Output = Result<CaptureDrainReceipt, CaptureDrainError>>
    + Send
    + 'static,
    done: tokio::sync::watch::Sender<Option<Result<CaptureDrainReceipt, CaptureDrainError>>>,
    health: Arc<CaptureHealth>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let result = AssertUnwindSafe(future)
            .catch_unwind()
            .await
            .unwrap_or_else(|_| {
                Err(CaptureDrainError::Failed(
                    "Native capture task panicked".into(),
                ))
            });
        let result = if health
            .cleanup_failed
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            Err(CaptureDrainError::Failed(
                "An input could not confirm stopping".into(),
            ))
        } else {
            result
        };
        let _ = done.send(Some(result));
    })
}

fn stop_inputs(
    mic: &mut Option<ChunkStream>,
    speaker: &mut Option<ChunkStream>,
) -> Result<(), CaptureDrainError> {
    let mic = mic
        .as_mut()
        .is_none_or(|stream| stream.as_mut().request_stop());
    let speaker = speaker
        .as_mut()
        .is_none_or(|stream| stream.as_mut().request_stop());
    if mic && speaker {
        Ok(())
    } else {
        Err(CaptureDrainError::Unsupported)
    }
}

async fn discard_stopped_input(stream: Option<ChunkStream>) {
    if let Some(mut stream) = stream {
        while let Some(Ok(_)) = stream.next().await {}
    }
}

struct CaptureStreamInner {
    inner: ReceiverStream<Result<CaptureFrame, Error>>,
    cancel_token: CancellationToken,
    task: JoinHandle<()>,
}

impl Stream for CaptureStreamInner {
    type Item = Result<CaptureFrame, Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl Drop for CaptureStreamInner {
    fn drop(&mut self) {
        self.cancel_token.cancel();
        self.task.abort();
    }
}

pub(crate) fn setup_mic_stream_owned(
    sample_rate: u32,
    chunk_size: usize,
    mic_device: Option<String>,
    integrity: Arc<hypr_audio::CaptureIntegrity>,
    owner: Option<Arc<dyn Send + Sync>>,
) -> Result<ChunkStream, Error> {
    let mic = MicInput::new(mic_device).map_err(|_| Error::MicOpenFailed)?;
    mic.stream_owned(Some(integrity), owner)
        .resampled_chunks(sample_rate, chunk_size)
        .map(|stream| Box::pin(ResampledChunks(stream)) as ChunkStream)
        .map_err(|_| Error::MicStreamSetupFailed)
}

pub(crate) fn setup_speaker_stream_owned(
    sample_rate: u32,
    chunk_size: usize,
    integrity: Arc<hypr_audio::CaptureIntegrity>,
    owner: Option<Arc<dyn Send + Sync>>,
) -> Result<ChunkStream, Error> {
    ensure_system_audio_permission()?;

    let speaker = SpeakerInput::new().map_err(crate::speaker::setup_error)?;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    let speaker_stream = speaker.stream_owned(Some(integrity), owner);
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let speaker_stream = {
        let _ = (integrity, owner);
        speaker.stream()
    };
    speaker_stream
        .map_err(crate::speaker::setup_error)?
        .resampled_chunks(sample_rate, chunk_size)
        .map(|stream| Box::pin(ResampledChunks(stream)) as ChunkStream)
        .map_err(|err| Error::SpeakerStreamSetupFailed(format!("resample: {err}")))
}

// NEVER_ASKED must pass through so first tap use can trigger the OS prompt;
// a stale DENIED grant makes the tap deliver silence, so fail fast instead.
#[cfg(target_os = "macos")]
fn ensure_system_audio_permission() -> Result<(), Error> {
    if hypr_tcc::audio_capture_permission_status() == hypr_tcc::DENIED {
        return Err(Error::SystemAudioPermissionDenied);
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn ensure_system_audio_permission() -> Result<(), Error> {
    Ok(())
}

pub(crate) fn open_dual(
    sample_rate: u32,
    mic_stream: ChunkStream,
    speaker_stream: ChunkStream,
    enable_aec: bool,
    health: Option<Arc<CaptureHealth>>,
) -> CaptureStream {
    let health = health.unwrap_or_else(|| Arc::new(CaptureHealth::default()));
    let integrity = health.integrity.clone();
    let cancel_token = CancellationToken::new();
    let stop = CancellationToken::new();
    let (done, receipt) = tokio::sync::watch::channel(None);
    let control = Arc::new(NativeControl {
        stop: stop.clone(),
        receipt,
    });
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let task = spawn_native(
        run_dual_loop(
            tx,
            (cancel_token.clone(), stop),
            sample_rate,
            enable_aec,
            mic_stream,
            speaker_stream,
            health.clone(),
        ),
        done,
        health,
    );
    CaptureStream::new(CaptureStreamInner {
        inner: ReceiverStream::new(rx),
        cancel_token,
        task,
    })
    .with_integrity(integrity)
    .with_drain_control(control)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaptureSide {
    Mic,
    Speaker,
}

pub(crate) fn open_single(
    chunk_stream: ChunkStream,
    side: CaptureSide,
    health: Option<Arc<CaptureHealth>>,
) -> CaptureStream {
    let health = health.unwrap_or_else(|| Arc::new(CaptureHealth::default()));
    let integrity = health.integrity.clone();
    let cancel_token = CancellationToken::new();
    let stop = CancellationToken::new();
    let (done, receipt) = tokio::sync::watch::channel(None);
    let control = Arc::new(NativeControl {
        stop: stop.clone(),
        receipt,
    });
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let task = spawn_native(
        run_single_loop(
            tx,
            (cancel_token.clone(), stop),
            chunk_stream,
            side,
            health.clone(),
        ),
        done,
        health,
    );
    CaptureStream::new(CaptureStreamInner {
        inner: ReceiverStream::new(rx),
        cancel_token,
        task,
    })
    .with_integrity(integrity)
    .with_drain_control(control)
}

enum StreamResult {
    Continue,
    Failed(Error),
}

async fn run_dual_loop(
    tx: tokio::sync::mpsc::Sender<Result<CaptureFrame, Error>>,
    controls: (CancellationToken, CancellationToken),
    sample_rate: u32,
    enable_aec: bool,
    mic_stream: ChunkStream,
    speaker_stream: ChunkStream,
    health: Arc<CaptureHealth>,
) -> Result<CaptureDrainReceipt, CaptureDrainError> {
    let (cancel, stop) = controls;
    let mut mic = Some(mic_stream);
    let mut speaker = Some(speaker_stream);
    let mut stopping = false;
    let mut frames = 0u64;
    let mut joiner = Joiner::with_integrity(Some(health.integrity.clone()));
    let mut aec = if enable_aec { build_aec() } else { None };
    let mut linear_echo_gain = None;
    let mut last_health = (0, 0);
    let mut aec_reference = if aec.is_some() {
        Some(AecReferenceAligner::new(sample_rate))
    } else {
        None
    };
    loop {
        let mut mic_ended = false;
        let mut speaker_ended = false;
        let result = tokio::select! {
            _ = cancel.cancelled() => return Err(CaptureDrainError::Failed("Native capture was aborted".into())),
            _ = stop.cancelled(), if !stopping => { stopping = true; stop_inputs(&mut mic, &mut speaker)?; StreamResult::Continue }
            item = async { mic.as_mut().expect("active microphone").next().await }, if mic.is_some() => {
                mic_ended = item.is_none();
                if item.is_none() && stopping { mic = None; StreamResult::Continue }
                else { handle_stream_item(item, CaptureSide::Mic, &mut joiner) }
            }
            item = async { speaker.as_mut().expect("active system audio").next().await }, if speaker.is_some() => {
                speaker_ended = item.is_none();
                if item.is_none() && stopping { speaker = None; StreamResult::Continue }
                else { handle_stream_item(item, CaptureSide::Speaker, &mut joiner) }
            }
        };
        match result {
            StreamResult::Failed(error) => {
                health.integrity.mark_discontinuity(true, true);
                let stopped = stop_inputs(&mut mic, &mut speaker);
                let _ = tx.send(Err(error)).await;
                stopped?;
                if mic_ended {
                    drop(mic.take());
                }
                if speaker_ended {
                    drop(speaker.take());
                }
                // A recovering source owns another task; dropping its receiver cannot prove
                // that task has stopped and joined the underlying native input.
                futures_util::future::join(
                    discard_stopped_input(mic.take()),
                    discard_stopped_input(speaker.take()),
                )
                .await;
                return Ok(CaptureDrainReceipt { frames });
            }
            StreamResult::Continue => {
                if report_health(&tx, &Some(health.clone()), &mut last_health).await {
                    aec = if enable_aec { build_aec() } else { None };
                    linear_echo_gain = None;
                    aec_reference = aec.as_ref().map(|_| AecReferenceAligner::new(sample_rate));
                }
                while let Some((raw_mic, raw_speaker)) = if stopping {
                    joiner.drain_when_closed(mic.is_none(), speaker.is_none())
                } else {
                    joiner.pop_pair()
                } {
                    let raw_mic = Arc::<[f32]>::from(raw_mic);
                    let raw_speaker = Arc::<[f32]>::from(raw_speaker);
                    let aligned = aec_reference
                        .as_mut()
                        .map(|aligner| aligner.align(&raw_speaker, &raw_mic))
                        .unwrap_or_else(|| AecAlignedPair {
                            mic: Arc::clone(&raw_mic),
                            speaker: Arc::clone(&raw_speaker),
                            alignment_changed: false,
                        });
                    if aligned.alignment_changed {
                        if let Some(processor) = aec.as_mut() {
                            processor.reset();
                        }
                        linear_echo_gain = None;
                    }
                    let aec_mic = process_aec(
                        &mut aec,
                        &mut linear_echo_gain,
                        &aligned.mic,
                        &aligned.speaker,
                    );

                    let frame_count = raw_mic.len().max(raw_speaker.len()) as u64;
                    let frame = CaptureFrame {
                        raw_mic,
                        raw_speaker,
                        aec_mic,
                    };
                    let permit = if stopping { tx.reserve().await } else {
                        tokio::select! {
                            _ = cancel.cancelled() => return Err(CaptureDrainError::Failed("Native capture was aborted".into())),
                            _ = stop.cancelled() => { stopping = true; stop_inputs(&mut mic, &mut speaker)?; tx.reserve().await }
                            permit = tx.reserve() => permit,
                        }
                    }.map_err(|_| CaptureDrainError::Failed("Capture output closed before draining".into()))?;
                    permit.send(Ok(frame));
                    frames += frame_count;
                }
                if mic.is_none() && speaker.is_none() {
                    return Ok(CaptureDrainReceipt { frames });
                }
            }
        }
    }
}

struct AecReferenceAligner {
    probe: SyncProbe,
    mic_delay_line: SampleDelayLine,
    speaker_delay_line: SampleDelayLine,
    last_alignment: AecAlignment,
    last_logged_state: Option<SyncProbeState>,
    last_logged_stable_lag_samples: Option<isize>,
    log_probe_events: bool,
}

struct AecAlignedPair {
    mic: Arc<[f32]>,
    speaker: Arc<[f32]>,
    alignment_changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AecAlignment {
    None,
    DelayMic(usize),
    DelaySpeaker(usize),
}

impl AecAlignment {
    fn from_lag_samples(lag_samples: Option<isize>) -> Self {
        match lag_samples {
            Some(lag) if lag > 0 => Self::DelaySpeaker(lag as usize),
            Some(lag) if lag < 0 => Self::DelayMic(lag.unsigned_abs()),
            _ => Self::None,
        }
    }

    fn mic_delay_samples(self) -> usize {
        match self {
            Self::DelayMic(samples) => samples,
            Self::None | Self::DelaySpeaker(_) => 0,
        }
    }

    fn speaker_delay_samples(self) -> usize {
        match self {
            Self::DelaySpeaker(samples) => samples,
            Self::None | Self::DelayMic(_) => 0,
        }
    }
}

impl AecReferenceAligner {
    fn new(sample_rate: u32) -> Self {
        let max_lag_samples = ((sample_rate as usize) * (AEC_MAX_REFERENCE_LAG_MS as usize)) / 1000;
        let mut config = SyncProbeConfig::new(sample_rate);
        config.max_lag_samples = max_lag_samples.max(config.max_lag_samples);
        let max_delay_samples = config.max_lag_samples;

        Self {
            probe: SyncProbe::new(config),
            mic_delay_line: SampleDelayLine::new(max_delay_samples),
            speaker_delay_line: SampleDelayLine::new(max_delay_samples),
            last_alignment: AecAlignment::None,
            last_logged_state: None,
            last_logged_stable_lag_samples: None,
            log_probe_events: std::env::var(AUDIO_SYNC_PROBE_ENV).ok().as_deref() == Some("1"),
        }
    }

    fn align(&mut self, raw_speaker: &[f32], raw_mic: &[f32]) -> AecAlignedPair {
        let observed = catch_unwind(AssertUnwindSafe(|| {
            self.probe.observe(raw_speaker, raw_mic)
        }));
        let event = match observed {
            Ok(event) => event,
            Err(_) => {
                tracing::error!("audio_sync_probe_panicked");
                None
            }
        };

        let alignment_changed = if let Some(event) = event {
            let alignment_changed = self.update_alignment(&event);
            if self.log_probe_events {
                self.log_probe_event(event);
            }
            alignment_changed
        } else {
            false
        };

        let aligned_mic = self
            .mic_delay_line
            .process(raw_mic, self.last_alignment.mic_delay_samples());
        let aligned_speaker = self
            .speaker_delay_line
            .process(raw_speaker, self.last_alignment.speaker_delay_samples());

        AecAlignedPair {
            mic: Arc::from(aligned_mic),
            speaker: Arc::from(aligned_speaker),
            alignment_changed,
        }
    }

    fn update_alignment(&mut self, event: &SyncProbeEvent) -> bool {
        let snapshot = event.snapshot();
        let next_alignment = if matches!(
            snapshot.state,
            SyncProbeState::Locked | SyncProbeState::Holdover
        ) {
            AecAlignment::from_lag_samples(snapshot.stable_lag_samples)
        } else {
            AecAlignment::None
        };

        if next_alignment != self.last_alignment {
            tracing::info!(
                previous_alignment = ?self.last_alignment,
                alignment = ?next_alignment,
                mic_delay_samples = next_alignment.mic_delay_samples(),
                speaker_delay_samples = next_alignment.speaker_delay_samples(),
                mic_delay_ms = next_alignment.mic_delay_samples() as f32
                    / self.probe.config().sample_rate as f32
                    * 1000.0,
                speaker_delay_ms = next_alignment.speaker_delay_samples() as f32
                    / self.probe.config().sample_rate as f32
                    * 1000.0,
                stable_lag_samples = snapshot.stable_lag_samples,
                state = ?snapshot.state,
                "aec_reference_alignment_changed"
            );
            self.last_alignment = next_alignment;
            return true;
        }

        false
    }

    fn log_probe_event(&mut self, event: SyncProbeEvent) {
        let snapshot = event.snapshot();
        let should_log = self.last_logged_state != Some(snapshot.state)
            || self.last_logged_stable_lag_samples != snapshot.stable_lag_samples;

        if !should_log {
            return;
        }

        match event {
            SyncProbeEvent::Measured(measurement) => {
                tracing::info!(
                    capture_time_sec = measurement.capture_time_sec,
                    state = ?measurement.snapshot.state,
                    stable_lag_samples = measurement.snapshot.stable_lag_samples,
                    candidate_lag_samples = measurement.snapshot.candidate_lag_samples,
                    accepted_window_count = measurement.snapshot.accepted_window_count,
                    confidence = measurement.snapshot.confidence,
                    peak_ratio = measurement.estimate.peak_ratio,
                    distinctiveness = measurement.estimate.distinctiveness,
                    drift_ppm = measurement.trend.drift_ppm,
                    "audio_sync_probe"
                );
            }
            SyncProbeEvent::SkippedLowConfidence(skip) => {
                tracing::info!(
                    capture_time_sec = skip.capture_time_sec,
                    state = ?skip.snapshot.state,
                    stable_lag_samples = skip.snapshot.stable_lag_samples,
                    candidate_lag_samples = skip.snapshot.candidate_lag_samples,
                    accepted_window_count = skip.snapshot.accepted_window_count,
                    confidence = skip.snapshot.confidence,
                    reason = ?skip.reason,
                    peak_ratio = skip.estimate.peak_ratio,
                    distinctiveness = skip.estimate.distinctiveness,
                    "audio_sync_probe"
                );
            }
            SyncProbeEvent::SkippedLowEnergy(skip) => {
                tracing::info!(
                    capture_time_sec = skip.capture_time_sec,
                    state = ?skip.snapshot.state,
                    stable_lag_samples = skip.snapshot.stable_lag_samples,
                    accepted_window_count = skip.snapshot.accepted_window_count,
                    reference_rms = skip.reference_rms,
                    observed_rms = skip.observed_rms,
                    "audio_sync_probe"
                );
            }
        }

        self.last_logged_state = Some(snapshot.state);
        self.last_logged_stable_lag_samples = snapshot.stable_lag_samples;
    }
}

struct SampleDelayLine {
    history: VecDeque<f32>,
    max_delay_samples: usize,
}

impl SampleDelayLine {
    fn new(max_delay_samples: usize) -> Self {
        Self {
            history: VecDeque::with_capacity(max_delay_samples + 1),
            max_delay_samples,
        }
    }

    fn process(&mut self, input: &[f32], delay_samples: usize) -> Vec<f32> {
        let delay_samples = delay_samples.min(self.max_delay_samples);
        let mut output = Vec::with_capacity(input.len());

        for &sample in input {
            self.history.push_back(sample);
            let delayed = self
                .history
                .len()
                .checked_sub(delay_samples + 1)
                .and_then(|idx| self.history.get(idx))
                .copied()
                .unwrap_or(0.0);
            output.push(delayed);

            while self.history.len() > self.max_delay_samples + 1 {
                self.history.pop_front();
            }
        }

        output
    }
}

async fn run_single_loop(
    tx: tokio::sync::mpsc::Sender<Result<CaptureFrame, Error>>,
    controls: (CancellationToken, CancellationToken),
    mut chunks: ChunkStream,
    side: CaptureSide,
    health: Arc<CaptureHealth>,
) -> Result<CaptureDrainReceipt, CaptureDrainError> {
    let (cancel, stop) = controls;
    let mut stopping = false;
    let mut frames = 0u64;
    let mut last_health = (0, 0);
    loop {
        let item = tokio::select! {
            _ = cancel.cancelled() => return Err(CaptureDrainError::Failed("Native capture was aborted".into())),
            _ = stop.cancelled(), if !stopping => { stopping = true; if !chunks.as_mut().request_stop() { return Err(CaptureDrainError::Unsupported); } continue; }
            item = chunks.next() => item,
        };
        match item {
            Some(Ok(data)) => {
                report_health(&tx, &Some(health.clone()), &mut last_health).await;
                let count = data.len() as u64;
                let data = Arc::<[f32]>::from(data);
                let silence = Arc::<[f32]>::from(vec![0.0; data.len()]);
                let frame = match side {
                    CaptureSide::Mic => CaptureFrame {
                        raw_mic: data,
                        raw_speaker: silence,
                        aec_mic: None,
                    },
                    CaptureSide::Speaker => CaptureFrame {
                        raw_mic: silence,
                        raw_speaker: data,
                        aec_mic: None,
                    },
                };
                let permit = if stopping { tx.reserve().await } else {
                    tokio::select! {
                        _ = cancel.cancelled() => return Err(CaptureDrainError::Failed("Native capture was aborted".into())),
                        _ = stop.cancelled() => { stopping = true; if !chunks.as_mut().request_stop() { return Err(CaptureDrainError::Unsupported); } tx.reserve().await }
                        permit = tx.reserve() => permit,
                    }
                }.map_err(|_| CaptureDrainError::Failed("Capture output closed before draining".into()))?;
                permit.send(Ok(frame));
                frames += count;
            }
            None if stopping => {
                drop(chunks);
                return Ok(CaptureDrainReceipt { frames });
            }
            error => {
                let ended = error.is_none();
                health
                    .integrity
                    .mark_discontinuity(side == CaptureSide::Mic, side == CaptureSide::Speaker);
                let stopped = chunks.as_mut().request_stop();
                let error = match (side, error.is_none()) {
                    (CaptureSide::Mic, true) => Error::MicStreamEnded,
                    (CaptureSide::Speaker, true) => Error::SpeakerStreamEnded,
                    (CaptureSide::Mic, false) => Error::MicResampleFailed,
                    (CaptureSide::Speaker, false) => Error::SpeakerResampleFailed,
                };
                let _ = tx.send(Err(error)).await;
                if !stopped {
                    return Err(CaptureDrainError::Unsupported);
                }
                if !ended {
                    discard_stopped_input(Some(chunks)).await;
                }
                return Ok(CaptureDrainReceipt { frames });
            }
        }
    }
}

async fn report_health(
    tx: &tokio::sync::mpsc::Sender<Result<CaptureFrame, Error>>,
    health: &Option<Arc<CaptureHealth>>,
    previous: &mut (u8, usize),
) -> bool {
    use std::sync::atomic::Ordering;
    let Some(health) = health else {
        return false;
    };
    let current = (
        health.recovering.load(Ordering::SeqCst),
        health.generation.load(Ordering::SeqCst),
    );
    if previous.0 != current.0 {
        let _ = tx
            .send(Err(Error::CaptureRecovering {
                microphone: current.0 & 1 != 0,
                speaker: current.0 & 2 != 0,
            }))
            .await;
    }
    let reset = previous.1 != current.1;
    if reset {
        // A whole reconnect can happen between polls, leaving the current recovery mask clear.
        health.integrity.mark_discontinuity(
            current.0 & 1 != 0 || current.0 == 0,
            current.0 & 2 != 0 || current.0 == 0,
        );
    }
    *previous = current;
    reset
}

fn handle_stream_item(
    item: Option<Result<Vec<f32>, hypr_resampler::Error>>,
    side: CaptureSide,
    joiner: &mut Joiner,
) -> StreamResult {
    match item {
        Some(Ok(data)) => {
            match side {
                CaptureSide::Mic => joiner.push_mic(data),
                CaptureSide::Speaker => joiner.push_speaker(data),
            }
            StreamResult::Continue
        }
        Some(Err(_)) => StreamResult::Failed(match side {
            CaptureSide::Mic => Error::MicResampleFailed,
            CaptureSide::Speaker => Error::SpeakerResampleFailed,
        }),
        None => StreamResult::Failed(match side {
            CaptureSide::Mic => Error::MicStreamEnded,
            CaptureSide::Speaker => Error::SpeakerStreamEnded,
        }),
    }
}

fn build_aec() -> Option<AEC> {
    AEC::new()
        .map_err(|error| tracing::warn!(error.message = ?error, "aec_init_failed"))
        .ok()
}

fn process_aec(
    aec: &mut Option<AEC>,
    linear_echo_gain: &mut Option<f32>,
    mic: &[f32],
    speaker: &[f32],
) -> Option<Arc<[f32]>> {
    let processor = aec.as_mut()?;
    match processor.process_streaming(mic, speaker) {
        Ok(processed) => Some(Arc::<[f32]>::from(cancel_linear_echo(
            speaker,
            processed,
            linear_echo_gain,
        ))),
        Err(error) => {
            tracing::warn!(error.message = ?error, "aec_failed");
            None
        }
    }
}

fn cancel_linear_echo(
    speaker: &[f32],
    processed: Vec<f32>,
    linear_echo_gain: &mut Option<f32>,
) -> Vec<f32> {
    let len = speaker.len().min(processed.len());
    if len == 0 {
        return processed;
    }

    let mut processed_energy = 0.0;
    let mut speaker_energy = 0.0;
    let mut cross_energy = 0.0;
    for idx in 0..len {
        let processed_sample = processed[idx];
        let speaker_sample = speaker[idx];
        processed_energy += processed_sample * processed_sample;
        speaker_energy += speaker_sample * speaker_sample;
        cross_energy += processed_sample * speaker_sample;
    }

    let len_f32 = len as f32;
    let processed_rms = (processed_energy / len_f32).sqrt();
    let speaker_rms = (speaker_energy / len_f32).sqrt();
    if processed_rms < AEC_MIN_MIC_RMS || speaker_rms < AEC_MIN_REFERENCE_RMS {
        return processed;
    }

    let correlation = cross_energy.abs() / (processed_energy * speaker_energy).sqrt().max(1e-6);
    if correlation < AEC_MIN_REFERENCE_CORRELATION {
        return processed;
    }

    let instantaneous_gain =
        (cross_energy / speaker_energy.max(1e-6)).clamp(-AEC_MAX_LINEAR_GAIN, AEC_MAX_LINEAR_GAIN);
    let residual_energy =
        (processed_energy - (cross_energy * cross_energy / speaker_energy.max(1e-6))).max(0.0);
    let residual_ratio = (residual_energy / len_f32).sqrt() / processed_rms.max(1e-6);
    let measured_gain = if residual_ratio > AEC_DOUBLE_TALK_RESIDUAL_RATIO {
        segmented_trimmed_gain(&processed, speaker, len, instantaneous_gain)
    } else {
        instantaneous_gain
    };
    let smoothed_gain = linear_echo_gain
        .map(|gain| gain + (measured_gain - gain) * AEC_LINEAR_GAIN_SMOOTHING)
        .unwrap_or(measured_gain);
    *linear_echo_gain = Some(smoothed_gain);

    let gain = if residual_ratio > AEC_DOUBLE_TALK_RESIDUAL_RATIO {
        smoothed_gain
    } else {
        measured_gain
    };

    let mut output = Vec::with_capacity(processed.len());
    output.extend(processed.iter().zip(speaker).take(len).map(
        |(processed_sample, speaker_sample)| {
            (processed_sample - gain * speaker_sample).clamp(-1.0, 1.0)
        },
    ));
    output.extend_from_slice(&processed[len..]);
    output
}

fn segmented_trimmed_gain(
    processed: &[f32],
    speaker: &[f32],
    len: usize,
    initial_gain: f32,
) -> f32 {
    let segment_len = (len / AEC_ROBUST_GAIN_SEGMENTS).max(1);
    let mut gains = Vec::with_capacity(AEC_ROBUST_GAIN_SEGMENTS);
    let mut start = 0;
    while start < len {
        let end = (start + segment_len).min(len);
        let mut processed_energy = 0.0;
        let mut speaker_energy = 0.0;
        let mut cross_energy = 0.0;
        for idx in start..end {
            let processed_sample = processed[idx];
            let speaker_sample = speaker[idx];
            processed_energy += processed_sample * processed_sample;
            speaker_energy += speaker_sample * speaker_sample;
            cross_energy += processed_sample * speaker_sample;
        }

        if processed_energy > 1e-8 && speaker_energy > 1e-8 {
            let correlation =
                cross_energy.abs() / (processed_energy * speaker_energy).sqrt().max(1e-6);
            if correlation >= AEC_MIN_REFERENCE_CORRELATION {
                let gain = (cross_energy / speaker_energy.max(1e-6))
                    .clamp(-AEC_MAX_LINEAR_GAIN, AEC_MAX_LINEAR_GAIN);
                gains.push((gain, speaker_energy));
            }
        }

        start = end;
    }

    if gains.len() < AEC_ROBUST_GAIN_MIN_SEGMENTS {
        return initial_gain;
    }

    gains.sort_by(|left, right| left.0.total_cmp(&right.0));
    let trim = if gains.len() >= 6 { gains.len() / 6 } else { 0 };
    let kept = &gains[trim..gains.len() - trim];
    let weighted_gain = kept.iter().map(|(gain, energy)| gain * energy).sum::<f32>();
    let kept_energy = kept.iter().map(|(_, energy)| energy).sum::<f32>();
    if kept_energy <= 1e-8 {
        return initial_gain;
    }

    (weighted_gain / kept_energy).clamp(-AEC_MAX_LINEAR_GAIN, AEC_MAX_LINEAR_GAIN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_aec_returns_instance() {
        let aec = build_aec();
        assert!(aec.is_some());
    }

    #[test]
    fn process_aec_returns_output_when_enabled() {
        let mut aec = build_aec();
        let mut linear_echo_gain = None;
        let mic = Arc::<[f32]>::from(vec![0.1_f32; 160]);
        let speaker = Arc::<[f32]>::from(vec![0.2_f32; 160]);

        let processed = process_aec(&mut aec, &mut linear_echo_gain, &mic, &speaker);
        assert_eq!(processed.as_ref().map(|data| data.len()), Some(160));
    }

    #[test]
    fn sample_delay_line_outputs_current_samples_with_zero_delay() {
        let mut delay = SampleDelayLine::new(4);

        let output = delay.process(&[1.0, 2.0, 3.0], 0);

        assert_eq!(output, vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn sample_delay_line_outputs_delayed_samples() {
        let mut delay = SampleDelayLine::new(4);

        let first = delay.process(&[1.0, 2.0, 3.0], 2);
        let second = delay.process(&[4.0, 5.0], 2);

        assert_eq!(first, vec![0.0, 0.0, 1.0]);
        assert_eq!(second, vec![2.0, 3.0]);
    }

    #[test]
    fn sample_delay_line_clamps_to_max_delay() {
        let mut delay = SampleDelayLine::new(2);

        let first = delay.process(&[1.0, 2.0, 3.0], 10);
        let second = delay.process(&[4.0], 10);

        assert_eq!(first, vec![0.0, 0.0, 1.0]);
        assert_eq!(second, vec![2.0]);
    }

    #[test]
    fn aec_alignment_delays_speaker_for_positive_lag() {
        let alignment = AecAlignment::from_lag_samples(Some(320));

        assert_eq!(alignment, AecAlignment::DelaySpeaker(320));
        assert_eq!(alignment.mic_delay_samples(), 0);
        assert_eq!(alignment.speaker_delay_samples(), 320);
    }

    #[test]
    fn aec_alignment_delays_mic_for_negative_lag() {
        let alignment = AecAlignment::from_lag_samples(Some(-320));

        assert_eq!(alignment, AecAlignment::DelayMic(320));
        assert_eq!(alignment.mic_delay_samples(), 320);
        assert_eq!(alignment.speaker_delay_samples(), 0);
    }
    #[tokio::test]
    async fn complete_recovery_between_polls_still_latches_integrity_without_local_warning_changes()
    {
        use std::sync::atomic::Ordering;
        let health = Arc::new(CaptureHealth::default());
        health.generation.store(2, Ordering::SeqCst);
        let (tx, mut rx) = tokio::sync::mpsc::channel(2);
        let mut previous = (0, 0);
        assert!(report_health(&tx, &Some(health.clone()), &mut previous).await);
        assert_eq!(previous, (0, 2));
        assert_eq!(health.integrity.channels(), (true, true));
        assert!(rx.try_recv().is_err());
        assert!(!report_health(&tx, &Some(health.clone()), &mut previous).await);
        assert!(health.integrity.discontinuous());
    }
    #[tokio::test]
    async fn graceful_single_stop_preserves_full_output_queue_current_frame_and_short_tail() {
        let (input, chunks, stopped) = queued_chunks(64);
        let mut expected = Vec::new();
        for chunk in 0..40 {
            let data = vec![chunk as f32 / 40.0; if chunk == 39 { 17 } else { 160 }];
            expected.extend_from_slice(&data);
            input.send(Ok(data)).await.unwrap();
        }
        let mut stream = open_single(chunks, CaptureSide::Speaker, None);
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while stopped.chunks.load(std::sync::atomic::Ordering::Acquire) < 33 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        stream.request_stop().unwrap();
        let mut actual = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while let Some(frame) = stream.next().await {
                actual.extend_from_slice(&frame.unwrap().raw_speaker);
            }
        })
        .await
        .unwrap();
        assert_eq!(actual, expected);
        assert!(stopped.stopped.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(
            stream.drain_receipt().await.unwrap().frames as usize,
            expected.len()
        );
        assert_eq!(
            stream
                .drain_control()
                .unwrap()
                .completion()
                .await
                .unwrap()
                .frames as usize,
            expected.len()
        );
        assert!(!stream.integrity().discontinuous());
    }
    #[tokio::test]
    async fn graceful_dual_stop_drains_unpaired_tails_without_erasing_real_samples() {
        let (mic, mic_chunks, mic_stopped) = queued_chunks(8);
        let (speaker, speaker_chunks, speaker_stopped) = queued_chunks(8);
        mic.send(Ok(vec![0.25; 1_920])).await.unwrap();
        mic.send(Ok(vec![-0.5; 777])).await.unwrap();
        speaker.send(Ok(vec![0.75; 1_920])).await.unwrap();
        let mut stream = open_dual(16_000, mic_chunks, speaker_chunks, false, None);
        stream.request_stop().unwrap();
        let mut actual_mic = Vec::new();
        let mut actual_speaker = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while let Some(frame) = stream.next().await {
                let frame = frame.unwrap();
                actual_mic.extend_from_slice(&frame.raw_mic);
                actual_speaker.extend_from_slice(&frame.raw_speaker);
            }
        })
        .await
        .unwrap();
        assert_eq!(actual_mic, [vec![0.25; 1_920], vec![-0.5; 777]].concat());
        assert_eq!(actual_speaker, [vec![0.75; 1_920], vec![0.0; 777]].concat());
        assert!(
            mic_stopped
                .stopped
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert!(
            speaker_stopped
                .stopped
                .load(std::sync::atomic::Ordering::Acquire)
        );
        assert_eq!(stream.drain_receipt().await.unwrap().frames, 2_697);
        assert!(!stream.integrity().discontinuous());
    }

    #[tokio::test]
    async fn interrupted_dual_input_waits_for_nested_recovery_resource_cleanup() {
        use std::sync::atomic::{AtomicBool, Ordering};

        struct DelayedClose {
            close: tokio::sync::oneshot::Receiver<()>,
            stopped: Arc<AtomicBool>,
            dropped: Arc<AtomicBool>,
        }
        impl Stream for DelayedClose {
            type Item = Result<Vec<f32>, hypr_resampler::Error>;
            fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                let state = self.get_mut();
                if !state.stopped.load(Ordering::Acquire) {
                    return Poll::Pending;
                }
                std::future::Future::poll(Pin::new(&mut state.close), cx).map(|_| None)
            }
        }
        impl ChunkSource for DelayedClose {
            fn request_stop(self: Pin<&mut Self>) -> bool {
                self.stopped.store(true, Ordering::Release);
                true
            }
        }
        impl Drop for DelayedClose {
            fn drop(&mut self) {
                self.dropped.store(true, Ordering::Release);
            }
        }
        let (release, close) = tokio::sync::oneshot::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let health = Arc::new(CaptureHealth::default());
        let (_changes, refresh) = tokio::sync::watch::channel(0);
        let speaker = super::super::recovery::recovering_chunks(
            Box::pin(DelayedClose {
                close,
                stopped: stopped.clone(),
                dropped: dropped.clone(),
            }),
            Arc::new(|| panic!("interrupted stop must not reopen input")),
            refresh,
            health.clone(),
            2,
            16_000,
            160,
        );
        let (mic, mic_chunks, _) = queued_chunks(1);
        drop(mic);
        let mut stream = open_dual(16_000, mic_chunks, speaker, false, Some(health));
        let control = stream.drain_control().unwrap();
        assert!(stream.next().await.unwrap().is_err());
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while !stopped.load(Ordering::Acquire) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(!dropped.load(Ordering::Acquire));
        assert!(control.completion().now_or_never().is_none());
        release.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while stream.next().await.is_some() {}
        })
        .await
        .unwrap();
        assert!(dropped.load(Ordering::Acquire));
        assert_eq!(control.completion().await.unwrap().frames, 0);
        assert_eq!(
            stream.drain_receipt().await,
            Err(CaptureDrainError::NotDrained)
        );
        assert!(stream.integrity().discontinuous());
    }

    #[tokio::test]
    async fn interrupted_resampler_error_stops_without_repolling_ready_errors_forever() {
        use std::sync::atomic::{AtomicBool, Ordering};
        struct Failed {
            polls: usize,
            stopped: bool,
            dropped_after_stop: Arc<AtomicBool>,
        }
        impl Stream for Failed {
            type Item = Result<Vec<f32>, hypr_resampler::Error>;
            fn poll_next(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Option<Self::Item>> {
                let state = self.get_mut();
                state.polls += 1;
                assert!(
                    state.polls <= 2,
                    "failed resampler must not be polled in a ready-error loop"
                );
                Poll::Ready(Some(Err(hypr_resampler::Async::<f32>::new_poly(
                    0.0,
                    2.0,
                    hypr_resampler::PolynomialDegree::Cubic,
                    160,
                    1,
                    hypr_resampler::FixedAsync::Input,
                )
                .err()
                .expect("fixture uses an invalid resampling ratio")
                .into())))
            }
        }
        impl ChunkSource for Failed {
            fn request_stop(self: Pin<&mut Self>) -> bool {
                self.get_mut().stopped = true;
                true
            }
        }
        impl Drop for Failed {
            fn drop(&mut self) {
                self.dropped_after_stop
                    .store(self.stopped, Ordering::Release);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let mut stream = open_single(
            Box::pin(Failed {
                polls: 0,
                stopped: false,
                dropped_after_stop: dropped.clone(),
            }),
            CaptureSide::Mic,
            None,
        );
        let control = stream.drain_control().unwrap();
        assert!(stream.next().await.unwrap().is_err());
        assert!(stream.next().await.is_none());
        assert_eq!(control.completion().await.unwrap().frames, 0);
        assert!(dropped.load(Ordering::Acquire));
        assert!(stream.integrity().discontinuous());
    }
}
