use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use futures_util::StreamExt;
use hypr_audio::AudioProvider;
use hypr_transcription_core::listener2::{BatchParams, BatchProvider, BatchRuntime, run_batch};
use serde::Serialize;
use tauri::Manager;
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

#[derive(Default)]
pub struct DictationState {
    capture: tokio::sync::Mutex<Option<Capture>>,
    processing: AtomicBool,
}

struct Capture {
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<Result<(tempfile::TempDir, std::path::PathBuf, usize), String>>,
}

#[derive(Clone, Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct DictationStatus {
    pub phase: String,
    pub amplitude: f64,
    pub message: Option<String>,
}

fn status(app: &tauri::AppHandle, phase: &str, amplitude: f64, message: Option<String>) {
    let _ = (DictationStatus {
        phase: phase.into(),
        amplitude,
        message,
    })
    .emit(app);
}

async fn capture_tail_samples(
    stream: &mut hypr_audio::CaptureStream,
    deadline: tokio::time::Instant,
) -> Result<Vec<f32>, String> {
    let mut samples = Vec::new();
    while let Ok(Some(frame)) = tokio::time::timeout_at(deadline, stream.next()).await {
        samples.extend_from_slice(&frame.map_err(|error| error.to_string())?.raw_mic);
    }
    Ok(samples)
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_start(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<DictationState>();
    let mut capture = state.capture.lock().await;
    if capture.is_some() || state.processing.load(Ordering::SeqCst) {
        return Err("Dictation is already active".into());
    }
    let snapshot = app.listener().get_capture_snapshot().await;
    if snapshot.active_session_id.is_some() || !snapshot.finalizing_session_ids.is_empty() {
        return Err("A meeting recording is in progress".into());
    }
    let config = app.settings().config();
    if config.current_stt_provider.as_deref() != Some("fmtr") || config.current_stt_model.is_none()
    {
        return Err("Select an on-device transcription model in Settings".into());
    }
    tauri_plugin_windows::dictation_capture_target()?;
    let audio = app.state::<Arc<dyn AudioProvider>>().inner().clone();
    let (stop, mut stopped) = tokio::sync::oneshot::channel();
    let (ready, started) = tokio::sync::oneshot::channel();
    status(&app, "starting", 0.0, None);
    let task_app = app.clone();
    let task = tokio::spawn(async move {
        let dir = tempfile::Builder::new()
            .prefix(TEMP_PREFIX)
            .tempdir()
            .map_err(|error| error.to_string())?;
        let path = dir.path().join("dictation.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer =
            hound::WavWriter::create(&path, spec).map_err(|error| error.to_string())?;
        let chunk_size = hypr_audio_utils::chunk_size_for_stt(SAMPLE_RATE);
        let mut stream = audio
            .open_mic_capture(None, SAMPLE_RATE, chunk_size)
            .map_err(|error| error.to_string())?;
        let mut ready = Some(ready);
        let mut sample_count = 0;
        let mut last_level = std::time::Instant::now();
        loop {
            tokio::select! {
                biased;
                _ = &mut stopped => break,
                frame = stream.next() => {
                    let frame = frame.ok_or("Microphone stream ended")?
                        .map_err(|error| error.to_string())?;
                    let samples = frame.raw_mic;
                    if samples.is_empty() {
                        continue;
                    }
                    let sum: f64 = samples.iter().map(|sample| f64::from(*sample).powi(2)).sum();
                    let amplitude = (sum / samples.len() as f64).sqrt().min(1.0);
                    for sample in samples.iter() {
                        writer.write_sample(*sample).map_err(|error| error.to_string())?;
                    }
                    sample_count += samples.len();
                    if let Some(ready) = ready.take() {
                        status(&task_app, "recording", amplitude, None);
                        let _ = ready.send(());
                    } else if last_level.elapsed() >= std::time::Duration::from_millis(100) {
                        status(&task_app, "recording", amplitude, None);
                        last_level = std::time::Instant::now();
                    }
                }
            }
        }
        // A final word can still be in the microphone buffer when the shortcut is released.
        let tail_samples = capture_tail_samples(
            &mut stream,
            tokio::time::Instant::now() + TAIL_CAPTURE_GRACE,
        )
        .await?;
        for sample in &tail_samples {
            writer
                .write_sample(*sample)
                .map_err(|error| error.to_string())?;
        }
        sample_count += tail_samples.len();
        writer.finalize().map_err(|error| error.to_string())?;
        tracing::info!(
            sample_count,
            tail_sample_count = tail_samples.len(),
            "dictation_capture_completed"
        );
        Ok((dir, path, sample_count))
    });
    match tokio::time::timeout(std::time::Duration::from_secs(5), started).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            let error = task
                .await
                .map_err(|error| error.to_string())?
                .err()
                .unwrap_or_else(|| "Microphone stopped before dictation started".to_string());
            status(&app, "error", 0.0, Some(error.clone()));
            return Err(error);
        }
        Err(_) => {
            task.abort();
            let error = "Microphone did not start in time".to_string();
            status(&app, "error", 0.0, Some(error.clone()));
            return Err(error);
        }
    }
    *capture = Some(Capture { stop, task });
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_stop(app: tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<DictationState>();
    if state.processing.swap(true, Ordering::SeqCst) {
        return Err("Dictation is already transcribing".into());
    }
    let result = stop_inner(&app).await;
    state.processing.store(false, Ordering::SeqCst);
    if let Err(error) = &result {
        status(&app, "error", 0.0, Some(error.clone()));
    }
    result
}

#[tauri::command]
#[specta::specta]
pub async fn dictation_is_recording(app: tauri::AppHandle) -> bool {
    let state = app.state::<DictationState>();
    state.processing.load(Ordering::SeqCst) || state.capture.lock().await.is_some()
}

async fn stop_inner(app: &tauri::AppHandle) -> Result<(), String> {
    let state = app.state::<DictationState>();
    let capture = state
        .capture
        .lock()
        .await
        .take()
        .ok_or("Dictation is not recording")?;
    let _ = capture.stop.send(());
    let (_temp, path, sample_count) = capture.task.await.map_err(|error| error.to_string())??;
    status(&app, "transcribing", 0.0, None);
    if sample_count < SAMPLE_RATE as usize / 4 {
        status(
            app,
            "error",
            0.0,
            Some("Hold the shortcut while speaking, then release it.".into()),
        );
        return Ok(());
    }
    let config = app.settings().config();
    let model_name = config
        .current_stt_model
        .ok_or("No transcription model selected")?;
    let model: LocalModel = serde_json::from_value(serde_json::Value::String(model_name.clone()))
        .map_err(|error| error.to_string())?;
    let url = if let Some(server) = app
        .local_stt()
        .get_server_for_model(&model)
        .await
        .map_err(|error| error.to_string())?
        && let Some(url) = server.url
    {
        url
    } else {
        app.local_stt()
            .start_server(model)
            .await
            .map_err(|error| error.to_string())?
    };
    let provider = if model_name.starts_with("soniqo-") || model_name.starts_with("onnx-") {
        BatchProvider::Soniqo
    } else {
        BatchProvider::WhisperLocal
    };
    let languages = config
        .meeting_languages
        .unwrap_or_else(|| vec!["en".into()]);
    let languages = languages
        .into_iter()
        .map(|language| language.parse().map_err(|error| format!("{error}")))
        .collect::<Result<Vec<_>, String>>()?;
    let output = run_batch(
        Arc::new(SilentRuntime),
        BatchParams {
            audio: Default::default(),
            session_id: "dictation".into(),
            provider,
            file_path: path.to_string_lossy().into_owned(),
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
    .map_err(|error| error.to_string())?;
    let text = output
        .response
        .results
        .channels
        .first()
        .and_then(|channel| channel.alternatives.first())
        .map(|alternative| alternative.transcript.trim())
        .unwrap_or_default();
    if text.is_empty() {
        status(&app, "error", 0.0, Some("No speech detected".into()));
        return Ok(());
    }
    if let Err(error) = app.tray().set_last_dictation(text.to_string()) {
        tracing::warn!(%error, "dictation_tray_menu_update_failed");
    }
    status(&app, "inserting", 0.0, None);
    match tauri_plugin_windows::dictation_insert(text) {
        Ok(()) => status(&app, "done", 0.0, None),
        Err(error) => status(&app, "error", 0.0, Some(error)),
    }
    Ok(())
}

struct SilentRuntime;
impl BatchRuntime for SilentRuntime {
    fn emit(&self, _event: hypr_transcription_core::listener2::BatchEvent) {}
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::stream;
    use hypr_audio::{CaptureFrame, CaptureStream};

    use super::capture_tail_samples;

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

        let tail = capture_tail_samples(&mut stream, deadline).await.unwrap();

        assert_eq!(tail, vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn finishes_tail_capture_when_microphone_has_no_more_frames() {
        let mut stream = CaptureStream::new(stream::pending());
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(20);

        let tail = capture_tail_samples(&mut stream, deadline).await.unwrap();

        assert!(tail.is_empty());
    }
}
