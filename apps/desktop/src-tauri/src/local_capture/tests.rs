use super::*;
use hypr_audio_actual::{
    AudioProvider, CaptureConfig, CaptureDrainControl, CaptureDrainError, CaptureDrainFuture,
    CaptureDrainReceipt, CaptureFrame, CaptureStream,
};
use hypr_loofah_core::{search::SearchService, settings::ConfigState, startup::StartupState};
use hypr_storage::vault::{LeaseAttempt, VaultLease};
use ractor::Actor;
use std::{path::PathBuf, time::Duration};
use tauri_plugin_transcription::{
    DeviceCaptureRoot, DeviceCaptureRootArgs, DeviceCaptureRootMsg as RootMsg,
    DeviceSessionDataEvent as Data, DeviceSessionErrorEvent as Error,
    DeviceSessionLifecycleEvent as Lifecycle, DeviceSessionProgressEvent as Progress,
    ListenerRuntime,
};
use tokio::sync::{Semaphore, watch};

#[derive(Default)]
struct Display {
    events: Mutex<Vec<Lifecycle>>,
    ready: AtomicBool,
}
impl hypr_storage::StorageRuntime for Display {
    fn global_base(&self) -> Result<PathBuf, hypr_storage::Error> {
        panic!("capture must retain original storage")
    }
    fn vault_base(&self) -> Result<PathBuf, hypr_storage::Error> {
        panic!("capture must retain original storage")
    }
}
impl ListenerRuntime for Display {
    fn emit_lifecycle(&self, event: Lifecycle) {
        self.events.lock().unwrap().push(event);
    }
    fn emit_progress(&self, event: Progress) {
        if matches!(event, Progress::AudioReady { .. }) {
            self.ready.store(true, Ordering::Release);
        }
    }
    fn emit_error(&self, _: Error) {}
    fn emit_data(&self, _: Data) {}
}
async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>) {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let vault = root.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    let LeaseAttempt::Acquired(lease) = VaultLease::try_acquire_existing(&vault).unwrap() else {
        panic!("fixture is owned")
    };
    let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
    let meta = serde_json::from_value(serde_json::json!({"id":"capture-note", "title":"Original", "created_at":"2026-11-08T10:00:00Z", "tags":[]})).unwrap();
    store
        .create_note_session(&meta, "Original note")
        .await
        .unwrap();
    let context = Arc::new(
        VaultContext::new(
            "local:capture".into(),
            store,
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.path().join("operations"))
        .with_vault_lease(lease)
        .unwrap(),
    );
    (root, context)
}
fn params() -> CaptureParams {
    CaptureParams {
        session_id: "capture-note".into(),
        languages: vec![],
        onboarding: true,
        model: String::new(),
        base_url: String::new(),
        api_key: String::new(),
        transcription_mode: Some(TranscriptionMode::Batch),
        participant_human_ids: vec![],
        self_human_id: None,
    }
}
fn delta(id: &str, text: &str, replaced: Vec<String>) -> Data {
    Data::TranscriptDelta { session_id:"capture-note".into(), delta:Box::new(serde_json::from_value(serde_json::json!({"new_words":[{"id":id,"text":text,"start_ms":0,"end_ms":20,"channel":0,"state":"final"}],"replaced_ids":replaced,"partials":[]})).unwrap()) }
}

#[tokio::test]
async fn owned_local_persistence_waits_for_real_source_join_and_preserves_original_vault() {
    let (_directory, context) = fixture().await;
    let vault = context.store.vault_base().to_owned();
    let attachment = vault.join("sessions/capture-note/user.bin");
    std::fs::write(&attachment, b"unknown\0attachment").unwrap();
    context
        .store
        .prepare_recording("capture-note")
        .await
        .unwrap();
    let display = Arc::new(Display::default());
    let (frames, input) = tokio::sync::mpsc::channel(4);
    let gate = Arc::new(Semaphore::new(0));
    let audio = Arc::new(Audio {
        input: Mutex::new(Some(input)),
        resource_gate: gate.clone(),
        fail_open: false.into(),
    });
    let (root, actor) = Actor::spawn(
        None,
        DeviceCaptureRoot,
        DeviceCaptureRootArgs {
            maintenance: Default::default(),
            runtime: Arc::new(Display::default()),
            audio,
        },
    )
    .await
    .unwrap();
    let (reporter, completion) = DeviceCaptureCompletion::channel();
    let (runtime, persist) = persistence::start(
        context.clone(),
        "capture-note".into(),
        uuid::Uuid::new_v4().to_string(),
        display.clone(),
        completion,
        CancellationToken::new(),
    );
    let weak = Arc::downgrade(&context);
    ractor::call!(
        root,
        RootMsg::StartOwnedLocalCapture,
        OwnedLocalCaptureArgs {
            request_id: "original-request".into(),
            params: params().into(),
            runtime: runtime.clone(),
            completion: reporter
        }
    )
    .unwrap()
    .unwrap();
    frames
        .send(CaptureFrame {
            raw_mic: Arc::from(vec![0.1; 1600]),
            raw_speaker: Arc::from(vec![-0.1; 1600]),
            aec_mic: None,
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !display.ready.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    runtime.emit_data(delta("first", "draft", vec![]));
    runtime.emit_data(delta("replacement", "saved", vec!["first".into()]));
    assert!(
        !ractor::call!(root, |reply| RootMsg::StopOwnedLocalCapture {
            session_id: "capture-note".into(),
            request_id: "another-request".into(),
            reply
        })
        .unwrap()
    );
    assert!(
        ractor::call!(root, |reply| RootMsg::StopOwnedLocalCapture {
            session_id: "capture-note".into(),
            request_id: "original-request".into(),
            reply
        })
        .unwrap()
    );
    let mut wait = tokio::spawn(async move { persist.await.unwrap() });
    assert!(
        tokio::time::timeout(Duration::from_millis(50), &mut wait)
            .await
            .is_err()
    );
    assert!(
        !display
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, Lifecycle::Inactive { .. }))
    );
    assert!(context.store.is_recording("capture-note"));
    drop(runtime);
    drop(context);
    assert!(weak.upgrade().is_some());
    assert!(matches!(
        VaultLease::try_acquire_existing(&vault).unwrap(),
        LeaseAttempt::Busy
    ));
    gate.add_permits(1);
    let completed = tokio::time::timeout(Duration::from_secs(10), wait)
        .await
        .unwrap()
        .unwrap();
    assert!(completed.physical.supervisor_ended);
    assert!(completed.physical.resources_released_observed_at.is_some());
    let persisted = completed.persistence.unwrap();
    assert!(persisted.transcript_id.is_some());
    assert!(
        persisted
            .audio_path
            .as_ref()
            .is_some_and(|path| PathBuf::from(path).starts_with(&vault))
    );
    let raw: serde_json::Value = serde_json::from_slice(
        &std::fs::read(vault.join("sessions/capture-note/transcript.json")).unwrap(),
    )
    .unwrap();
    let text = raw.to_string();
    assert!(text.contains("saved"));
    assert!(!text.contains("draft"));
    assert_eq!(std::fs::read(attachment).unwrap(), b"unknown\0attachment");
    assert!(
        display
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, Lifecycle::Inactive { error: None, .. }))
    );
    root.stop(None);
    actor.await.unwrap();
}

#[tokio::test]
async fn missing_physical_completion_never_reports_saved_capture() {
    let (_directory, context) = fixture().await;
    let display = Arc::new(Display::default());
    let (reporter, completion) = DeviceCaptureCompletion::channel();
    let (runtime, join) = persistence::start(
        context,
        "capture-note".into(),
        uuid::Uuid::new_v4().to_string(),
        display.clone(),
        completion,
        CancellationToken::new(),
    );
    runtime.emit_lifecycle(Lifecycle::Inactive {
        session_id: "capture-note".into(),
        audio_path: None,
        error: None,
    });
    drop(reporter);
    let result = tokio::time::timeout(Duration::from_secs(2), join)
        .await
        .unwrap()
        .unwrap();
    assert!(result.persistence.is_err());
    assert!(!result.physical.supervisor_ended);
    assert!(display.events.lock().unwrap().is_empty());
}

#[tokio::test]
async fn shutdown_before_dispatch_never_polls_the_native_start() {
    let state = LocalCaptureState::default();
    state.request_shutdown();
    let entered = AtomicBool::new(false);
    let result = dispatch(&state, async {
        entered.store(true, Ordering::Release);
        Ok(())
    })
    .await;
    assert!(matches!(
        result,
        Err(DeviceCaptureStartError::NotDispatched)
    ));
    assert!(!entered.load(Ordering::Acquire));
}

#[tokio::test]
async fn shutdown_after_dispatch_preserves_unknown_outcome_until_original_acknowledgement() {
    let state = Arc::new(LocalCaptureState::default());
    let (sent, observed) = tokio::sync::oneshot::channel();
    let (finish, ack) = tokio::sync::oneshot::channel();
    let owner = state.clone();
    let task = tokio::spawn(async move {
        dispatch(&owner, async {
            sent.send(()).unwrap();
            ack.await.unwrap()
        })
        .await
    });
    observed.await.unwrap();
    state.request_shutdown();
    finish
        .send(Err(DeviceCaptureStartError::Unconfirmed))
        .unwrap();
    assert!(matches!(
        task.await.unwrap(),
        Err(DeviceCaptureStartError::Unconfirmed)
    ));
}
struct Audio {
    input: Mutex<Option<tokio::sync::mpsc::Receiver<CaptureFrame>>>,
    resource_gate: Arc<Semaphore>,
    fail_open: AtomicBool,
}
struct Drain {
    stop: CancellationToken,
    done: watch::Receiver<Option<u64>>,
}
impl CaptureDrainControl for Drain {
    fn request_stop(&self) -> Result<(), CaptureDrainError> {
        self.stop.cancel();
        Ok(())
    }
    fn completion(&self) -> CaptureDrainFuture<'_> {
        let mut done = self.done.clone();
        Box::pin(async move {
            loop {
                let count = *done.borrow_and_update();
                if let Some(frames) = count {
                    return Ok(CaptureDrainReceipt { frames });
                }
                done.changed().await.map_err(|_| {
                    CaptureDrainError::Failed("fixture resource ownership disappeared".into())
                })?;
            }
        })
    }
}
impl Audio {
    fn stream(&self) -> Result<CaptureStream, hypr_audio_actual::Error> {
        if self.fail_open.load(Ordering::Acquire) {
            return Err(hypr_audio_actual::Error::MicOpenFailed);
        }
        let input = self.input.lock().unwrap().take().unwrap();
        let stop = CancellationToken::new();
        let (done, rx) = watch::channel(None);
        let control = Arc::new(Drain {
            stop: stop.clone(),
            done: rx,
        });
        let gate = self.resource_gate.clone();
        let stream = futures_util::stream::unfold(
            (input, stop, done, gate, 0u64, false),
            |(mut input, stop, done, gate, mut frames, mut stopping)| async move {
                let frame = if stopping {
                    input.recv().await
                } else {
                    tokio::select! { _ = stop.cancelled() => { stopping = true; input.close(); input.recv().await }, frame = input.recv() => frame }
                };
                if let Some(frame) = frame {
                    frames += frame.raw_mic.len().max(frame.raw_speaker.len()) as u64;
                    Some((Ok(frame), (input, stop, done, gate, frames, stopping)))
                } else {
                    let _permit = gate.acquire().await.unwrap();
                    done.send_replace(Some(frames));
                    None
                }
            },
        );
        Ok(CaptureStream::new(stream).with_drain_control(control))
    }
}
impl AudioProvider for Audio {
    fn open_capture(&self, _: CaptureConfig) -> Result<CaptureStream, hypr_audio_actual::Error> {
        self.stream()
    }
    fn open_speaker_capture(
        &self,
        _: u32,
        _: usize,
    ) -> Result<CaptureStream, hypr_audio_actual::Error> {
        self.stream()
    }
    fn open_mic_capture(
        &self,
        _: Option<String>,
        _: u32,
        _: usize,
    ) -> Result<CaptureStream, hypr_audio_actual::Error> {
        self.stream()
    }
    fn default_device_name(&self) -> String {
        "fixture".into()
    }
    fn list_mic_devices(&self) -> Vec<String> {
        vec![]
    }
    fn play_silence(&self) -> std::sync::mpsc::Sender<()> {
        std::sync::mpsc::channel().0
    }
    fn play_bytes(&self, _: &'static [u8]) -> std::sync::mpsc::Sender<()> {
        std::sync::mpsc::channel().0
    }
    fn probe_mic(&self, _: Option<String>) -> Result<(), hypr_audio_actual::Error> {
        Ok(())
    }
    fn probe_speaker(&self) -> Result<(), hypr_audio_actual::Error> {
        Ok(())
    }
}
