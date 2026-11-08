use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use hypr_fs_format::TranscriptWord;
use hypr_loofah_core::VaultContext;
use hypr_vault_write::TranscriptDelta;
use tauri_plugin_transcription::{
    DeviceCaptureCompletion, DeviceCaptureCompletionSnapshot, DeviceSessionDataEvent as Data,
    DeviceSessionErrorEvent as Error, DeviceSessionLifecycleEvent as Lifecycle,
    DeviceSessionProgressEvent as Progress, ListenerRuntime, LiveTranscriptDelta,
};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

pub(super) struct Persisted {
    pub transcript_id: Option<String>,
    pub live_active: bool,
    pub needs_repair: bool,
    pub audio_path: Option<String>,
}
pub(super) struct Completed {
    pub physical: DeviceCaptureCompletionSnapshot,
    pub persistence: Result<Persisted, String>,
}
struct End {
    path: Option<String>,
    at: String,
    error: Option<String>,
}
struct Shared {
    failed: AtomicBool,
    live: AtomicBool,
    repair: AtomicBool,
    started: Mutex<Option<String>>,
    ended: Mutex<Option<End>>,
    wake: tokio::sync::Notify,
    closed: AtomicBool,
    stop_requested: AtomicBool,
}
pub(super) struct Runtime {
    context: Arc<VaultContext>,
    session: String,
    request: String,
    display: Arc<dyn ListenerRuntime>,
    shared: Arc<Shared>,
    deltas: mpsc::Sender<LiveTranscriptDelta>,
}
impl Runtime {
    fn fail(&self) {
        self.shared.failed.store(true, Ordering::Release);
        self.shared.wake.notify_one();
        if !self.shared.stop_requested.swap(true, Ordering::AcqRel) {
            let session = self.session.clone();
            let request = self.request.clone();
            tokio::spawn(async move {
                tauri_plugin_transcription::stop_owned_local_capture(session, request).await;
            });
        }
    }
    fn current(&self, session: &str) -> bool {
        session == self.session && !self.shared.closed.load(Ordering::Acquire)
    }
}
impl hypr_storage::StorageRuntime for Runtime {
    fn global_base(&self) -> Result<PathBuf, hypr_storage::Error> {
        self.display.global_base()
    }
    fn vault_base(&self) -> Result<PathBuf, hypr_storage::Error> {
        Ok(self.context.store.vault_base().to_owned())
    }
}
impl ListenerRuntime for Runtime {
    fn emit_lifecycle(&self, event: Lifecycle) {
        let session = match &event {
            Lifecycle::Active { session_id, .. }
            | Lifecycle::Finalizing { session_id }
            | Lifecycle::Inactive { session_id, .. } => session_id,
        };
        if !self.current(session) {
            self.fail();
            return;
        }
        let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        match &event {
            Lifecycle::Active {
                current_transcription_mode,
                ..
            } => {
                self.shared.live.store(
                    matches!(
                        current_transcription_mode,
                        tauri_plugin_transcription::TranscriptionMode::Live
                    ),
                    Ordering::Release,
                );
                if let Ok(mut started) = self.shared.started.lock() {
                    started.get_or_insert(at);
                } else {
                    self.fail();
                }
                self.shared.wake.notify_one();
            }
            Lifecycle::Inactive {
                audio_path, error, ..
            } => {
                if let Ok(mut ended) = self.shared.ended.lock() {
                    if ended.is_some() {
                        self.fail();
                        return;
                    }
                    *ended = Some(End {
                        path: audio_path.clone(),
                        at,
                        error: error.clone(),
                    });
                } else {
                    self.fail();
                }
                self.shared.wake.notify_one();
                return;
            }
            _ => {}
        }
        self.display.emit_lifecycle(event);
    }
    fn emit_progress(&self, event: Progress) {
        self.display.emit_progress(event);
    }
    fn emit_error(&self, event: Error) {
        match &event {
            Error::ConnectionError { session_id, .. } if self.current(session_id) => {
                self.shared.repair.store(true, Ordering::Release);
            }
            Error::AudioError {
                session_id,
                is_fatal: true,
                ..
            } if self.current(session_id) => self.fail(),
            _ => {}
        }
        self.display.emit_error(event);
    }
    fn emit_data(&self, event: Data) {
        if let Data::TranscriptDelta { session_id, delta } = &event {
            if !self.current(session_id) {
                self.fail();
                return;
            }
            if !delta.new_words.is_empty() || !delta.replaced_ids.is_empty() {
                let bytes: usize = delta
                    .new_words
                    .iter()
                    .map(|word| word.id.len().saturating_add(word.text.len()))
                    .chain(delta.replaced_ids.iter().map(String::len))
                    .sum();
                if delta.new_words.len() > 4096
                    || delta.replaced_ids.len() > 4096
                    || bytes > 256 * 1024
                    || self.deltas.try_send((**delta).clone()).is_err()
                {
                    self.fail();
                }
            }
        }
        self.display.emit_data(event);
    }
}

pub(super) fn start(
    context: Arc<VaultContext>,
    session: String,
    request: String,
    display: Arc<dyn ListenerRuntime>,
    mut completion: DeviceCaptureCompletion,
    not_dispatched: CancellationToken,
) -> (Arc<Runtime>, tokio::task::JoinHandle<Completed>) {
    let (send, mut receive) = mpsc::channel(128);
    let shared = Arc::new(Shared {
        failed: false.into(),
        live: false.into(),
        repair: false.into(),
        started: Mutex::new(None),
        ended: Mutex::new(None),
        wake: Default::default(),
        closed: false.into(),
        stop_requested: false.into(),
    });
    let runtime = Arc::new(Runtime {
        context: context.clone(),
        session: session.clone(),
        request,
        display: display.clone(),
        shared: shared.clone(),
        deltas: send,
    });
    let join = tokio::spawn(async move {
        let transcript_id = uuid::Uuid::new_v4().to_string();
        let started_at = chrono::Utc::now().timestamp_millis() as f64;
        let mut wrote_transcript = false;
        let mut stamped_start = false;
        let mut failure = None;
        let mut completion_open = true;
        let mut deltas_open = true;
        loop {
            let at = shared.started.lock().ok().and_then(|value| value.clone());
            if !stamped_start && let Some(at) = at {
                stamped_start = true;
                if context
                    .store
                    .mark_recording_started(&session, &at)
                    .await
                    .is_err()
                {
                    failure
                        .get_or_insert_with(|| "Recording metadata could not be saved".to_owned());
                }
            }
            let physical = completion.current();
            let joined =
                physical.supervisor_ended && physical.resources_released_observed_at.is_some();
            let ended = shared
                .ended
                .lock()
                .map(|value| value.is_some())
                .unwrap_or(false);
            if joined && ended {
                while let Ok(delta) = receive.try_recv() {
                    persist_delta(
                        &context,
                        &session,
                        &transcript_id,
                        started_at,
                        delta,
                        &mut wrote_transcript,
                        &mut failure,
                    )
                    .await;
                }
                break;
            }
            tokio::select! {
                _ = not_dispatched.cancelled() => {
                    shared.closed.store(true, Ordering::Release);
                    return Completed { physical, persistence: Err("Capture was not dispatched".into()) };
                }
                delta = receive.recv(), if deltas_open => {
                    if let Some(delta) = delta { persist_delta(&context, &session, &transcript_id, started_at, delta, &mut wrote_transcript, &mut failure).await; }
                    else { deltas_open = false; failure.get_or_insert_with(|| "Capture persistence stream ended unexpectedly".into()); }
                }
                changed = completion.changed(), if completion_open => {
                    if changed.is_err() {
                        completion_open = false;
                        if !joined || !ended {
                        shared.closed.store(true, Ordering::Release);
                        return Completed { physical: completion.current(), persistence: Err("Original capture completion could not be confirmed".into()) };
                        }
                    }
                }
                _ = shared.wake.notified() => {}
            }
        }
        shared.closed.store(true, Ordering::Release);
        if shared.failed.load(Ordering::Acquire) {
            failure.get_or_insert_with(|| "Capture persistence requires recovery".into());
        }
        let end = shared
            .ended
            .lock()
            .ok()
            .and_then(|mut value| value.take())
            .expect("observed exact capture end");
        if wrote_transcript
            && context
                .store
                .finish_transcript(&session, &transcript_id)
                .await
                .is_err()
        {
            failure.get_or_insert_with(|| "Transcript could not be saved".into());
        }
        let mut audio_path = end.path;
        if let Some(path) = &audio_path {
            match catalog(&context, &session, path).await {
                Ok(canonical) => audio_path = Some(canonical),
                Err(_) => {
                    failure.get_or_insert_with(|| "Recording could not be cataloged".into());
                }
            }
        }
        if context
            .store
            .mark_recording_ended(&session, &end.at)
            .await
            .is_err()
        {
            failure.get_or_insert_with(|| "Recording metadata could not be saved".into());
        }
        let error = failure.clone().or(end.error);
        display.emit_lifecycle(Lifecycle::Inactive {
            session_id: session.clone(),
            audio_path: audio_path.clone(),
            error,
        });
        Completed {
            physical: completion.current(),
            persistence: match failure {
                Some(cause) => Err(cause),
                None => Ok(Persisted {
                    transcript_id: wrote_transcript.then_some(transcript_id),
                    live_active: shared.live.load(Ordering::Acquire),
                    needs_repair: shared.repair.load(Ordering::Acquire),
                    audio_path,
                }),
            },
        }
    });
    (runtime, join)
}
async fn persist_delta(
    context: &VaultContext,
    session: &str,
    transcript_id: &str,
    started_at: f64,
    delta: LiveTranscriptDelta,
    wrote: &mut bool,
    failure: &mut Option<String>,
) {
    if delta.new_words.is_empty() && delta.replaced_ids.is_empty() {
        return;
    }
    let words = delta
        .new_words
        .into_iter()
        .map(|word| TranscriptWord {
            id: Some(word.id),
            text: word.text,
            start_ms: word.start_ms as f64,
            end_ms: word.end_ms as f64,
            channel: word.channel as f64,
            speaker: None,
            metadata: Some(serde_json::Map::from_iter([(
                "capture_source".into(),
                serde_json::json!("recording"),
            )])),
        })
        .collect();
    let delta = TranscriptDelta {
        transcript_id: transcript_id.into(),
        new_words: words,
        replaced_ids: delta.replaced_ids,
        new_hints: vec![],
        started_at_ms: started_at,
    };
    match context.store.append_transcript(session, delta).await {
        Ok(()) => *wrote = true,
        Err(_) => {
            failure.get_or_insert_with(|| "Live transcript could not be saved".into());
        }
    }
}

async fn catalog(
    context: &Arc<VaultContext>,
    session: &str,
    source: &str,
) -> Result<String, String> {
    let directory = context.store.vault_base().join(
        context
            .store
            .session_dir(session)
            .await
            .map_err(|_| "Recording directory is unavailable")?,
    );
    let path = PathBuf::from(source);
    let Some(relative) = path
        .strip_prefix(&directory)
        .ok()
        .and_then(|value| value.to_str())
    else {
        return Err("Capture recording belongs to another session".into());
    };
    if !matches!(
        relative,
        "audio.mp3"
            | "audio.wav"
            | "audio.ogg"
            | "audio/audio.mp3"
            | "audio/audio.wav"
            | "audio/audio.ogg"
    ) {
        return Err("Capture recording is not an owned audio file".into());
    }
    let pin = context.clone();
    tokio::task::spawn_blocking(move || {
        let _pin = pin;
        let metadata = path
            .symlink_metadata()
            .map_err(|_| "Capture recording is unavailable")?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err("Capture recording is not a regular file");
        }
        Ok(())
    })
    .await
    .map_err(|_| "Capture recording inspection failed")??;
    context
        .store
        .store_audio(session, source)
        .await
        .map_err(|_| "Capture recording could not be saved".into())
}
