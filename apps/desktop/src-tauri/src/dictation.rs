use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use futures_util::StreamExt;
use hypr_audio::AudioProvider;
use hypr_transcription_core::listener2::{
    BatchParams, BatchProvider, BatchRuntime, run_dictation_batch,
};
use serde::Serialize;
use tauri::Manager;
use tauri_plugin_clipboard_manager::ClipboardExt;
use tauri_plugin_local_stt::{LocalModel, LocalSttPluginExt};
use tauri_plugin_settings::SettingsPluginExt;
use tauri_plugin_transcription::ListenerPluginExt;
use tauri_plugin_tray::TrayPluginExt;
use tauri_specta::Event;

const SAMPLE_RATE: u32 = 16_000;
const TEMP_PREFIX: &str = "loofah-dictation-";
const TAIL_CAPTURE_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

pub fn cleanup_stale_audio() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(TEMP_PREFIX)
            && entry.file_type().is_ok_and(|kind| kind.is_dir())
            && entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age > std::time::Duration::from_secs(24 * 60 * 60))
        {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum DictationPhase {
    #[default]
    Idle,
    Starting,
    Recording,
    LoadingModel,
    Transcribing,
    Inserting,
    Done,
    Copied,
    Cancelled,
    Error,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum DictationProblem {
    Disabled,
    MeetingActive,
    ModelRequired,
    Microphone,
    AccessibilityRequired,
    TargetUnavailable,
    TranscriptionFailed,
    NoSpeech,
    InsertionFailed,
    Busy,
    CopyFailed,
}

#[derive(Clone, Default, Debug, Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct DictationStatus {
    pub attempt_id: u32,
    pub revision: u32,
    pub phase: DictationPhase,
    pub amplitude: f64,
    pub problem: Option<DictationProblem>,
    pub can_retry: bool,
    pub can_copy: bool,
    pub busy: bool,
}

#[derive(Default)]
pub struct DictationState(Mutex<Inner>, Arc<AtomicBool>);

impl DictationState {
    pub fn shutdown(&self) {
        self.1.store(true, Ordering::SeqCst);
        // Exit runs on the main thread; insertion may already be dispatched there.
        // Never block that thread waiting for a worker which is waiting for it.
        if let Ok(mut inner) = self.0.try_lock() {
            inner.failed_audio = None;
            inner.text = None;
            if let Some(operation) = &inner.operation {
                operation.cancel.store(true, Ordering::SeqCst);
                operation.wake.notify_one();
            }
        }
    }
}

#[derive(Default)]
struct Inner {
    status: DictationStatus,
    operation: Option<Arc<Operation>>,
    failed_audio: Option<Arc<RecordedAudio>>,
    text: Option<String>,
}

#[derive(Default)]
struct Operation {
    cancel: AtomicBool,
    shutdown: Arc<AtomicBool>,
    stop: AtomicBool,
    wake: tokio::sync::Notify,
}

impl Operation {
    fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst) || self.shutdown.load(Ordering::SeqCst)
    }
}

struct RecordedAudio {
    _directory: tempfile::TempDir,
    path: std::path::PathBuf,
    samples: usize,
}

impl Inner {
    fn changed(&mut self) -> DictationStatus {
        self.status.revision = self.status.revision.wrapping_add(1);
        self.status.busy = self.operation.is_some();
        self.status.can_retry = self.operation.is_none() && self.failed_audio.is_some();
        self.status.can_copy = self.operation.is_none() && self.text.is_some();
        self.status.clone()
    }

    fn finish_delivery(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => self.status.phase = DictationPhase::Done,
            Err(error) => {
                tracing::warn!(%error, "dictation_insert_failed");
                self.status.phase = DictationPhase::Error;
                self.status.problem = Some(match error.as_str() {
                    "accessibility_permission_required" => DictationProblem::AccessibilityRequired,
                    "target_unavailable" => DictationProblem::TargetUnavailable,
                    _ => DictationProblem::InsertionFailed,
                });
            }
        }
    }

    fn begin(&mut self, shutdown: Arc<AtomicBool>) -> (u32, Arc<Operation>) {
        self.status.attempt_id = self.status.attempt_id.wrapping_add(1);
        self.status.phase = DictationPhase::Starting;
        self.status.problem = None;
        self.status.amplitude = 0.0;
        self.text = None;
        let operation = Arc::new(Operation {
            shutdown,
            ..Default::default()
        });
        self.operation = Some(operation.clone());
        (self.status.attempt_id, operation)
    }

    fn finish_cancelled(&mut self, retry: bool, shutdown: bool) {
        if retry || shutdown {
            self.failed_audio = None;
        }
        self.text = None;
        if self.failed_audio.is_some() {
            self.status.phase = DictationPhase::Error;
            self.status.problem = Some(DictationProblem::TranscriptionFailed);
        } else {
            self.status.phase = DictationPhase::Cancelled;
            self.status.problem = None;
        }
    }

    fn finish_failure(
        &mut self,
        problem: DictationProblem,
        audio: Option<Arc<RecordedAudio>>,
        retry: bool,
    ) {
        if problem == DictationProblem::NoSpeech {
            if retry || audio.as_ref().is_some_and(|audio| audio.samples > 0) {
                self.failed_audio = None;
            }
        } else if audio.is_some() {
            self.failed_audio = audio;
        }
        self.status.phase = DictationPhase::Error;
        self.status.problem = Some(
            if problem == DictationProblem::NoSpeech && self.failed_audio.is_some() {
                DictationProblem::TranscriptionFailed
            } else {
                problem
            },
        );
    }

    fn accepts(&self, id: u32) -> bool {
        self.status.attempt_id == id
    }

    fn may_deliver(&self, id: u32) -> bool {
        self.accepts(id)
            && self
                .operation
                .as_ref()
                .is_some_and(|operation| !operation.is_cancelled())
    }
}

fn emit(app: &tauri::AppHandle, snapshot: DictationStatus) -> DictationStatus {
    let _ = snapshot.emit(app);
    snapshot
}

fn update(app: &tauri::AppHandle, id: u32, phase: DictationPhase, amplitude: f64) {
    let state = app.state::<DictationState>();
    let mut inner = state.0.lock().unwrap();
    if !inner.may_deliver(id) {
        return;
    }
    inner.status.phase = phase;
    inner.status.amplitude = amplitude;
    inner.status.problem = None;
    emit(app, inner.changed());
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_set_copy_label(app: tauri::AppHandle, label: String) -> Result<(), String> {
    app.tray()
        .set_dictation_copy_label(label)
        .map_err(|error| error.to_string())
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_get_status(app: tauri::AppHandle) -> DictationStatus {
    app.state::<DictationState>()
        .0
        .lock()
        .unwrap()
        .status
        .clone()
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_is_recording(app: tauri::AppHandle) -> bool {
    app.state::<DictationState>()
        .0
        .lock()
        .unwrap()
        .operation
        .is_some()
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_start(app: tauri::AppHandle) -> Result<DictationStatus, String> {
    let state = app.state::<DictationState>();
    let mut inner = state.0.lock().unwrap();
    if state.1.load(Ordering::SeqCst) {
        return Ok(inner.status.clone());
    }
    if inner.operation.is_some() {
        inner.status.problem = Some(DictationProblem::Busy);
        return Ok(emit(&app, inner.changed()));
    }
    let (id, operation) = inner.begin(state.1.clone());
    tauri_plugin_windows::dictation_set_active(true);
    let snapshot = emit(&app, inner.changed());
    drop(inner);
    drop(state);
    tauri::async_runtime::spawn(async move {
        run_attempt(app, id, operation, None).await;
    });
    Ok(snapshot)
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_stop(
    app: tauri::AppHandle,
    attempt_id: u32,
) -> Result<DictationStatus, String> {
    let state = app.state::<DictationState>();
    let inner = state.0.lock().unwrap();
    if inner.accepts(attempt_id)
        && matches!(
            inner.status.phase,
            DictationPhase::Starting | DictationPhase::Recording
        )
    {
        if let Some(operation) = &inner.operation {
            operation.stop.store(true, Ordering::SeqCst);
            operation.wake.notify_one();
        }
    }
    Ok(inner.status.clone())
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_cancel(
    app: tauri::AppHandle,
    attempt_id: u32,
) -> Result<DictationStatus, String> {
    let state = app.state::<DictationState>();
    let mut inner = state.0.lock().unwrap();
    if inner.accepts(attempt_id)
        && let Some(operation) = &inner.operation
    {
        operation.cancel.store(true, Ordering::SeqCst);
        operation.wake.notify_one();
        inner.status.phase = DictationPhase::Cancelled;
        inner.status.problem = None;
        tauri_plugin_windows::dictation_set_active(false);
        return Ok(emit(&app, inner.changed()));
    }
    Ok(inner.status.clone())
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_retry(
    app: tauri::AppHandle,
    attempt_id: u32,
) -> Result<DictationStatus, String> {
    let state = app.state::<DictationState>();
    let mut inner = state.0.lock().unwrap();
    if !inner.accepts(attempt_id) || inner.operation.is_some() {
        return Ok(inner.status.clone());
    }
    let Some(audio) = inner.failed_audio.clone() else {
        return Ok(inner.status.clone());
    };
    let (id, operation) = inner.begin(state.1.clone());
    tauri_plugin_windows::dictation_set_active(true);
    let snapshot = emit(&app, inner.changed());
    drop(inner);
    drop(state);
    tauri::async_runtime::spawn(async move {
        run_attempt(app, id, operation, Some(audio)).await;
    });
    Ok(snapshot)
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_copy(
    app: tauri::AppHandle,
    attempt_id: u32,
) -> Result<DictationStatus, String> {
    let state = app.state::<DictationState>();
    let mut inner = state.0.lock().unwrap();
    if !inner.accepts(attempt_id) || inner.operation.is_some() {
        return Ok(inner.status.clone());
    }
    let Some(text) = &inner.text else {
        return Ok(inner.status.clone());
    };
    if let Err(error) = app.clipboard().write_text(text) {
        tracing::warn!(%error, "dictation_copy_failed");
        inner.status.phase = DictationPhase::Error;
        inner.status.problem = Some(DictationProblem::CopyFailed);
    } else {
        inner.status.phase = DictationPhase::Copied;
        inner.status.problem = None;
    }
    Ok(emit(&app, inner.changed()))
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_discard(
    app: tauri::AppHandle,
    attempt_id: u32,
) -> Result<DictationStatus, String> {
    let state = app.state::<DictationState>();
    let mut inner = state.0.lock().unwrap();
    if inner.accepts(attempt_id) && inner.operation.is_none() {
        if let Err(error) = app.tray().clear_last_dictation(attempt_id) {
            tracing::warn!(%error, "dictation_tray_clear_failed");
        }
        inner.failed_audio = None;
        inner.text = None;
        inner.status.phase = DictationPhase::Idle;
        inner.status.problem = None;
        return Ok(emit(&app, inner.changed()));
    }
    Ok(inner.status.clone())
}

async fn run_attempt(
    app: tauri::AppHandle,
    id: u32,
    operation: Arc<Operation>,
    retry: Option<Arc<RecordedAudio>>,
) {
    let is_retry = retry.is_some();
    let mut audio = retry;
    let mut admission_lease = None;
    let result = async {
        if !app.settings().config().dictation_enabled {
            return Err(DictationProblem::Disabled);
        }
        let capture_admission = app.state::<tauri_plugin_transcription::CaptureAdmission>();
        let admission = capture_admission.lock().await;
        app.listener()
            .ensure_capture_idle()
            .await
            .map_err(|error| {
                if matches!(
                    error,
                    tauri_plugin_transcription::Error::SessionAlreadyRunning
                ) {
                    DictationProblem::MeetingActive
                } else {
                    tracing::warn!(%error, "dictation_capture_state_unavailable");
                    DictationProblem::Busy
                }
            })?;
        admission_lease = Some(Arc::new(
            admission
                .reserve_dictation()
                .ok_or(DictationProblem::Busy)?,
        ));
        drop(admission);
        let config = app.settings().config();
        if config.current_stt_provider.as_deref() != Some("fmtr") {
            return Err(DictationProblem::ModelRequired);
        }
        let model_name = config
            .current_stt_model
            .ok_or(DictationProblem::ModelRequired)?;
        let model: LocalModel =
            serde_json::from_value(serde_json::Value::String(model_name.clone()))
                .map_err(|_| DictationProblem::ModelRequired)?;
        if !app
            .local_stt()
            .is_model_downloaded(&model)
            .await
            .unwrap_or(false)
        {
            return Err(DictationProblem::ModelRequired);
        }
        if operation.is_cancelled() {
            return Ok(None);
        }
        let warm_app = app.clone();
        // Whisper startup can replace the active server. Keep meeting capture out
        // until startup finishes even if this recording is cancelled mid-load.
        let server_lease = if matches!(model, LocalModel::Whisper(_)) {
            admission_lease.clone()
        } else {
            None
        };
        let warming = tokio::spawn(async move {
            let _lease = server_lease;
            warm_app.local_stt().prepare_dictation_model(model).await
        });
        if !is_retry {
            audio = Some(Arc::new(record(&app, id, &operation).await.map_err(
                |error| {
                    tracing::warn!(%error, "dictation_capture_failed");
                    DictationProblem::Microphone
                },
            )?));
        }
        if operation.is_cancelled() {
            return Ok(None);
        }
        let recorded = audio.as_ref().unwrap();
        if recorded.samples < SAMPLE_RATE as usize / 4 {
            return Err(DictationProblem::NoSpeech);
        }
        if !warming.is_finished() {
            update(&app, id, DictationPhase::LoadingModel, 0.0);
        }
        let cancelled = async {
            while !operation.is_cancelled() {
                operation.wake.notified().await;
            }
        };
        let url = tokio::select! {
            _ = cancelled => return Ok(None),
            result = warming => result.map_err(|_| DictationProblem::TranscriptionFailed)?
                .map_err(|error| {
                    tracing::warn!(%error, "dictation_model_failed");
                    DictationProblem::TranscriptionFailed
                })?,
        };
        if operation.is_cancelled() {
            return Ok(None);
        }
        update(&app, id, DictationPhase::Transcribing, 0.0);
        let languages = config
            .meeting_languages
            .unwrap_or_else(|| vec!["en".into()])
            .into_iter()
            .map(|language| language.parse())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| DictationProblem::ModelRequired)?;
        let output = run_dictation_batch(
            Arc::new(DictationRuntime {
                operation: operation.clone(),
                _audio: recorded.clone(),
            }),
            BatchParams {
                audio: Default::default(),
                session_id: format!("dictation-{id}"),
                provider: if model_name.starts_with("soniqo-") || model_name.starts_with("onnx-") {
                    BatchProvider::Soniqo
                } else {
                    BatchProvider::WhisperLocal
                },
                file_path: recorded.path.to_string_lossy().into_owned(),
                model: Some(model_name),
                base_url: url,
                api_key: String::new(),
                languages,
                keywords: Vec::new(),
                num_speakers: Some(1),
                min_speakers: None,
                max_speakers: None,
            },
        )
        .await
        .map_err(|error| {
            tracing::warn!(%error, "dictation_transcription_failed");
            DictationProblem::TranscriptionFailed
        })?;
        let text = output
            .response
            .results
            .channels
            .first()
            .and_then(|channel| channel.alternatives.first())
            .map(|alternative| alternative.transcript.trim().to_owned())
            .unwrap_or_default();
        if text.is_empty() {
            return Err(DictationProblem::NoSpeech);
        }
        Ok(Some(text))
    }
    .await;
    let state = app.state::<DictationState>();
    let mut inner = state.0.lock().unwrap();
    if !inner.accepts(id) {
        return;
    }
    if operation.is_cancelled() {
        inner.finish_cancelled(is_retry, operation.shutdown.load(Ordering::SeqCst));
    } else {
        match result {
            Ok(Some(text)) => {
                inner.failed_audio = None;
                if let Err(error) = app.tray().set_last_dictation(id, text.clone()) {
                    tracing::warn!(%error, "dictation_tray_update_failed");
                }
                inner.status.problem = None;
                if is_retry {
                    inner.status.phase = DictationPhase::Ready;
                } else {
                    inner.status.phase = DictationPhase::Inserting;
                    emit(&app, inner.changed());
                    // Serialize cancellation with the final insertion decision.
                    let delivery = tauri_plugin_windows::dictation_insert(&text);
                    inner.finish_delivery(delivery);
                }
                inner.text = Some(text);
            }
            Err(problem) => inner.finish_failure(problem, audio, is_retry),
            Ok(None) => {}
        }
    }
    drop(admission_lease);
    inner.operation = None;
    tauri_plugin_windows::dictation_set_active(false);
    emit(&app, inner.changed());
}

async fn capture_tail_samples(
    stream: &mut hypr_audio::CaptureStream,
    deadline: tokio::time::Instant,
    operation: &Operation,
) -> Result<Vec<f32>, String> {
    let mut samples = Vec::new();
    loop {
        if operation.is_cancelled() {
            break;
        }
        tokio::select! {
            biased;
            _ = operation.wake.notified() => continue,
            frame = tokio::time::timeout_at(deadline, stream.next()) => {
                let Ok(Some(frame)) = frame else { break; };
                samples.extend_from_slice(&frame.map_err(|error| error.to_string())?.raw_mic);
            }
        }
    }
    Ok(samples)
}

async fn record(
    app: &tauri::AppHandle,
    id: u32,
    operation: &Operation,
) -> Result<RecordedAudio, String> {
    let directory = tempfile::Builder::new()
        .prefix(TEMP_PREFIX)
        .tempdir()
        .map_err(|error| error.to_string())?;
    let path = directory.path().join("dictation.wav");
    let mut writer = hound::WavWriter::create(
        &path,
        hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        },
    )
    .map_err(|error| error.to_string())?;
    let audio = app.state::<Arc<dyn AudioProvider>>().inner().clone();
    let mut stream = audio
        .open_mic_capture(
            None,
            SAMPLE_RATE,
            hypr_audio_utils::chunk_size_for_stt(SAMPLE_RATE),
        )
        .map_err(|error| error.to_string())?;
    let mut samples = 0;
    let mut last_level = std::time::Instant::now();
    let mut amplitude = 0.0;
    loop {
        if operation.is_cancelled() || operation.stop.load(Ordering::SeqCst) {
            break;
        }
        tokio::select! {
            biased;
            _ = operation.wake.notified() => continue,
            frame = tokio::time::timeout(std::time::Duration::from_secs(5), stream.next()) => {
                let frame = frame.map_err(|_| "Microphone timed out")?.ok_or("Microphone stream ended")?.map_err(|error| error.to_string())?;
                if frame.raw_mic.is_empty() { continue; }
                amplitude = hypr_audio_utils::smoothed_audio_level(amplitude, &frame.raw_mic);
                for sample in frame.raw_mic.iter() { writer.write_sample(*sample).map_err(|error| error.to_string())?; }
                if samples == 0 {
                    let state = app.state::<DictationState>();
                    let mut inner = state.0.lock().unwrap();
                    if inner.accepts(id) { inner.failed_audio = None; }
                }
                samples += frame.raw_mic.len();
                if samples == frame.raw_mic.len() || last_level.elapsed() >= std::time::Duration::from_millis(100) {
                    update(app, id, DictationPhase::Recording, f64::from(amplitude));
                    last_level = std::time::Instant::now();
                }
            }
        }
    }
    if !operation.is_cancelled() && samples > 0 {
        let tail = capture_tail_samples(
            &mut stream,
            tokio::time::Instant::now() + TAIL_CAPTURE_GRACE,
            operation,
        )
        .await?;
        for sample in &tail {
            writer
                .write_sample(*sample)
                .map_err(|error| error.to_string())?;
        }
        samples += tail.len();
    }
    writer.finalize().map_err(|error| error.to_string())?;
    Ok(RecordedAudio {
        _directory: directory,
        path,
        samples,
    })
}

struct DictationRuntime {
    operation: Arc<Operation>,
    _audio: Arc<RecordedAudio>,
}
impl BatchRuntime for DictationRuntime {
    fn emit(&self, _event: hypr_transcription_core::listener2::BatchEvent) {}
    fn is_cancelled(&self) -> bool {
        self.operation.is_cancelled()
    }
}
#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::stream;
    use hypr_audio::{CaptureFrame, CaptureStream};

    use super::capture_tail_samples;

    #[test]
    fn delivery_failure_preserves_copyable_transcript_without_retrying_audio() {
        for error in [
            "accessibility_permission_required",
            "target_unavailable",
            "Input rejected",
        ] {
            let mut inner = super::Inner::default();
            inner.begin(Arc::new(std::sync::atomic::AtomicBool::new(false)));
            inner.text = Some("Keep these words".into());
            inner.finish_delivery(Err(error.into()));
            inner.operation = None;
            let status = inner.changed();
            assert!(status.can_copy);
            assert!(!status.can_retry);
            assert!(!status.busy);
            assert_eq!(status.phase, super::DictationPhase::Error);
            assert_eq!(
                status.problem,
                Some(match error {
                    "accessibility_permission_required" =>
                        super::DictationProblem::AccessibilityRequired,
                    "target_unavailable" => super::DictationProblem::TargetUnavailable,
                    _ => super::DictationProblem::InsertionFailed,
                })
            );
            assert_eq!(inner.text.as_deref(), Some("Keep these words"));
        }
    }

    #[tokio::test]
    async fn captures_microphone_frame_arriving_after_shortcut_release() {
        let mut stream = CaptureStream::new(stream::once(async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            Ok(CaptureFrame {
                raw_mic: Arc::from([0.1, 0.2, 0.3]),
                raw_speaker: Arc::from([]),
                aec_mic: None,
            })
        }));
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(100);

        let tail = capture_tail_samples(&mut stream, deadline, &super::Operation::default())
            .await
            .unwrap();

        assert_eq!(tail, vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn finishes_tail_capture_when_microphone_has_no_more_frames() {
        let mut stream = CaptureStream::new(stream::pending());
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(20);

        let tail = capture_tail_samples(&mut stream, deadline, &super::Operation::default())
            .await
            .unwrap();

        assert!(tail.is_empty());
    }
    #[test]
    fn cancellation_revokes_delivery_but_keeps_busy_until_worker_drains() {
        let mut inner = super::Inner::default();
        let (id, operation) = inner.begin(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        assert!(inner.may_deliver(id));
        operation
            .cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!inner.may_deliver(id));
        assert!(inner.changed().busy);
        inner.operation = None;
        assert!(!inner.changed().busy);
    }

    #[test]
    fn earlier_attempt_cannot_deliver_into_new_attempt() {
        let mut inner = super::Inner::default();
        let (first, _) = inner.begin(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        inner.operation = None;
        let (second, _) = inner.begin(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        assert_ne!(first, second);
        assert!(!inner.may_deliver(first));
        assert!(inner.may_deliver(second));
    }

    #[test]
    fn failed_audio_survives_startup_and_is_unavailable_while_busy() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dictation.wav");
        std::fs::write(&path, b"recorded audio").unwrap();
        let mut inner = super::Inner::default();
        inner.failed_audio = Some(Arc::new(super::RecordedAudio {
            _directory: directory,
            path: path.clone(),
            samples: 100,
        }));
        inner.text = Some("previous result".into());
        assert!(inner.changed().can_retry);
        inner.begin(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let snapshot = inner.changed();
        assert!(!snapshot.can_retry);
        assert!(!snapshot.can_copy);
        assert!(path.exists());
        inner.operation = None;
        assert!(inner.changed().can_retry);
        inner.failed_audio = None;
        assert!(!path.exists());
    }

    #[test]
    fn runtime_keeps_audio_alive_until_blocking_consumer_releases_it() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dictation.wav");
        std::fs::write(&path, b"recorded audio").unwrap();
        let audio = Arc::new(super::RecordedAudio {
            _directory: directory,
            path: path.clone(),
            samples: 100,
        });
        let operation = Arc::new(super::Operation::default());
        let runtime: Arc<dyn hypr_transcription_core::listener2::BatchRuntime> =
            Arc::new(super::DictationRuntime {
                operation: operation.clone(),
                _audio: audio.clone(),
            });
        drop(audio);
        operation
            .cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(runtime.is_cancelled());
        assert!(path.exists());
        drop(runtime);
        assert!(!path.exists());
    }
    #[test]
    fn shutdown_removes_failed_take_and_result() {
        let state = super::DictationState::default();
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dictation.wav");
        std::fs::write(&path, b"failed take").unwrap();
        {
            let mut inner = state.0.lock().unwrap();
            inner.failed_audio = Some(Arc::new(super::RecordedAudio {
                _directory: directory,
                path: path.clone(),
                samples: 100,
            }));
            inner.text = Some("result".into());
        }
        state.shutdown();
        assert!(!path.exists());
        assert!(state.0.lock().unwrap().text.is_none());
    }

    #[test]
    fn shutdown_cancels_without_waiting_for_main_thread_insertion_lock() {
        let state = super::DictationState::default();
        let mut guard = state.0.lock().unwrap();
        let (_, operation) = guard.begin(state.1.clone());
        state.shutdown();
        assert!(operation.is_cancelled());
        assert!(guard.operation.is_some());
    }
    #[tokio::test]
    async fn cancel_releases_microphone_tail_without_waiting_for_capture_deadline() {
        let operation = Arc::new(super::Operation::default());
        let capture_operation = operation.clone();
        let (started, ready) = tokio::sync::oneshot::channel();
        let capture = tokio::spawn(async move {
            let mut stream = CaptureStream::new(stream::pending());
            started.send(()).unwrap();
            capture_tail_samples(
                &mut stream,
                tokio::time::Instant::now() + std::time::Duration::from_secs(60),
                &capture_operation,
            )
            .await
            .unwrap()
        });
        ready.await.unwrap();
        operation
            .cancel
            .store(true, std::sync::atomic::Ordering::SeqCst);
        operation.wake.notify_one();
        let samples = tokio::time::timeout(std::time::Duration::from_secs(1), capture)
            .await
            .unwrap()
            .unwrap();
        assert!(samples.is_empty());
    }
    #[test]
    fn early_release_and_cancel_preserve_previous_take_until_new_audio_or_retry_discard() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("dictation.wav");
        std::fs::write(&path, b"previous failed recording").unwrap();
        let mut inner = super::Inner::default();
        inner.failed_audio = Some(Arc::new(super::RecordedAudio {
            _directory: directory,
            path: path.clone(),
            samples: 100,
        }));
        inner.begin(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        let empty_directory = tempfile::tempdir().unwrap();
        let empty_path = empty_directory.path().join("dictation.wav");
        std::fs::write(&empty_path, b"").unwrap();
        inner.finish_failure(
            super::DictationProblem::NoSpeech,
            Some(Arc::new(super::RecordedAudio {
                _directory: empty_directory,
                path: empty_path.clone(),
                samples: 0,
            })),
            false,
        );
        inner.operation = None;
        let released_early = inner.changed();
        assert!(released_early.can_retry);
        assert_eq!(
            released_early.problem,
            Some(super::DictationProblem::TranscriptionFailed)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"previous failed recording");
        assert!(!empty_path.exists());

        inner.begin(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        inner.finish_cancelled(false, false);
        inner.operation = None;
        let restored = inner.changed();
        assert!(restored.can_retry);
        assert_eq!(restored.phase, super::DictationPhase::Error);
        assert_eq!(
            restored.problem,
            Some(super::DictationProblem::TranscriptionFailed)
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"previous failed recording");

        inner.begin(Arc::new(std::sync::atomic::AtomicBool::new(false)));
        inner.finish_cancelled(true, false);
        inner.operation = None;
        let discarded = inner.changed();
        assert!(!discarded.can_retry);
        assert_eq!(discarded.phase, super::DictationPhase::Cancelled);
        assert!(!path.exists());
    }
}
