use super::*;
use hypr_loofah_client::capture_spool::{
    CaptureFormat, CaptureIdentity, CaptureScope, CaptureSpool, CaptureSpoolLimits, CaptureTarget,
};
use hypr_loofah_protocol::{ErrorCode, MutationOutcome};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tauri_plugin_transcription::{
    DeviceListenerRuntime, DeviceRecordingEnd, DeviceRecordingSink, DeviceSessionLifecycleEvent,
};

#[derive(Default)]
struct Observer(Mutex<Vec<ScopedDeviceCaptureEvent>>);
impl DeviceCaptureObserver for Observer {
    fn emit(&self, event: ScopedDeviceCaptureEvent) {
        self.0.lock().unwrap().push(event);
    }
}
fn identity() -> CaptureIdentity {
    CaptureIdentity {
        scope: CaptureScope {
            server: "https://capture.example/".into(),
            member_id: "member".into(),
            vault_id: "remote".into(),
        },
        capture_id: uuid::Uuid::new_v4().to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        target: CaptureTarget::PendingSession {
            create_request_id: uuid::Uuid::new_v4().to_string(),
        },
    }
}
async fn spool() -> (tempfile::TempDir, Arc<CaptureSpool>) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap().join("device");
    let spool = CaptureSpool::open(root, CaptureSpoolLimits::default())
        .await
        .unwrap();
    (temp, spool)
}

#[tokio::test]
async fn pending_capture_is_scoped_f32_and_sealed_without_a_canonical_session() {
    let _capture = CAPTURE_TEST_LOCK.lock().await;
    let (temp, spool) = spool().await;
    let local = temp.path().join("sessions/same-id");
    std::fs::create_dir_all(&local).unwrap();
    std::fs::write(
        local.join("_meta.json"),
        b"local metadata remains untouched",
    )
    .unwrap();
    let observer = Arc::new(Observer::default());
    let identity = identity();
    let prepared = PreparedDeviceCapture::prepare(
        spool.clone(),
        identity.clone(),
        CaptureFormat::stereo(),
        observer.clone(),
    )
    .await
    .unwrap();
    let runtime = runtime::DeviceRuntime::new(prepared.sink.clone());
    let pcm: Vec<_> = [-0.0_f32, 0.75, -0.5, 0.25]
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    prepared
        .sink
        .append(0, 2, Arc::from(pcm.clone()))
        .await
        .unwrap();
    prepared
        .sink
        .finish(DeviceRecordingEnd::Complete)
        .await
        .unwrap();
    runtime.emit_lifecycle(DeviceSessionLifecycleEvent::Inactive {
        session_id: prepared.key.actor_id(),
        audio_path: None,
        error: None,
    });
    let snapshots = spool.inventory(identity.scope.clone()).await.unwrap();
    assert_eq!(snapshots[0].identity, identity);
    assert!(snapshots[0].session_id.is_none());
    assert_eq!(snapshots[0].seal.as_ref().unwrap().pcm_bytes, 16);
    let writer = spool.resume(identity.clone()).await.unwrap();
    assert_eq!(writer.read_chunk(0).await.unwrap().bytes, pcm);
    assert!(
        PreparedDeviceCapture::prepare(
            spool,
            identity.clone(),
            CaptureFormat::stereo(),
            observer.clone()
        )
        .await
        .is_err()
    );
    assert_eq!(
        std::fs::read(local.join("_meta.json")).unwrap(),
        b"local metadata remains untouched"
    );
    let events = observer.0.lock().unwrap();
    assert!(events.iter().all(|event| event.identity == identity));
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.event, DeviceCaptureEvent::Sealed(_)))
            .count(),
        1
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.event, DeviceCaptureEvent::Interrupted { .. }))
    );
}

#[tokio::test]
async fn interruption_flush_receipt_does_not_seal_or_mutate_baseline() {
    let _capture = CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let observer = Arc::new(Observer::default());
    let mut identity = identity();
    identity.target = CaptureTarget::ExistingSession {
        session_id: "same-id".into(),
        expected_audio_sha256: Some("c".repeat(64)),
    };
    let prepared = PreparedDeviceCapture::prepare(
        spool.clone(),
        identity.clone(),
        CaptureFormat::stereo(),
        observer.clone(),
    )
    .await
    .unwrap();
    prepared
        .sink
        .append(0, 1, Arc::from(vec![0; 8]))
        .await
        .unwrap();
    prepared
        .sink
        .finish(DeviceRecordingEnd::Interrupted)
        .await
        .unwrap();
    prepared.sink.stopped(None);
    let snapshot = spool
        .inventory(identity.scope.clone())
        .await
        .unwrap()
        .remove(0);
    assert_eq!(snapshot.identity.target, identity.target);
    assert_eq!(snapshot.frames, 1);
    assert!(snapshot.seal.is_none());
    assert!(
        observer
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, DeviceCaptureEvent::Interrupted { .. }))
    );
    assert!(
        !observer
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, DeviceCaptureEvent::Sealed(_)))
    );
}

#[tokio::test]
async fn retryable_busy_retains_identical_chunk_until_storage_accepts_it() {
    let _capture = CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let observer = Arc::new(Observer::default());
    let prepared =
        PreparedDeviceCapture::prepare(spool, identity(), CaptureFormat::stereo(), observer)
            .await
            .unwrap();
    let attempts = AtomicUsize::new(0);
    let pcm: Arc<[u8]> = Arc::from(vec![0; 8]);
    let retained = pcm.clone();
    let receipt = tokio::time::timeout(
        Duration::from_secs(3),
        prepared.sink.retry(|| {
            let attempt = attempts.fetch_add(1, Ordering::SeqCst);
            let pcm = pcm.clone();
            let retained = retained.clone();
            async move {
                assert!(Arc::ptr_eq(&pcm, &retained));
                if attempt < 5 {
                    Err(hypr_loofah_protocol::ApplicationError {
                        retryable: true,
                        mutation_outcome: Some(MutationOutcome::NotCommitted),
                        ..hypr_loofah_client::failure(ErrorCode::Unavailable, "busy")
                    })
                } else {
                    Ok(7_u64)
                }
            }
        }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(receipt, 7);
    assert_eq!(attempts.load(Ordering::SeqCst), 6);
}

struct FakeAudio {
    receiver: Mutex<
        Option<
            tokio::sync::mpsc::Receiver<
                Result<hypr_audio_actual::CaptureFrame, hypr_audio_actual::Error>,
            >,
        >,
    >,
    admitted: Arc<AtomicUsize>,
}

struct FakeDrain {
    stop: tokio::sync::watch::Sender<bool>,
    completed: tokio::sync::watch::Sender<Option<hypr_audio_actual::CaptureDrainReceipt>>,
}

impl FakeDrain {
    fn new() -> Self {
        Self {
            stop: tokio::sync::watch::channel(false).0,
            completed: tokio::sync::watch::channel(None).0,
        }
    }
}

impl hypr_audio_actual::CaptureDrainControl for FakeDrain {
    fn request_stop(&self) -> Result<(), hypr_audio_actual::CaptureDrainError> {
        self.stop.send_replace(true);
        Ok(())
    }

    fn completion(&self) -> hypr_audio_actual::CaptureDrainFuture<'_> {
        Box::pin(async move {
            let mut completed = self.completed.subscribe();
            loop {
                if let Some(receipt) = *completed.borrow_and_update() {
                    return Ok(receipt);
                }
                completed.changed().await.map_err(|_| {
                    hypr_audio_actual::CaptureDrainError::Failed(
                        "Fixture resource receipt closed".into(),
                    )
                })?;
            }
        })
    }
}

impl FakeAudio {
    fn stream(&self) -> hypr_audio_actual::CaptureStream {
        let mut receiver = self
            .receiver
            .lock()
            .unwrap()
            .take()
            .expect("capture source restarted unexpectedly");
        let admitted = self.admitted.clone();
        let drain = Arc::new(FakeDrain::new());
        let provider = drain.clone();
        hypr_audio_actual::CaptureStream::new(async_stream::stream! {
            let mut stop = provider.stop.subscribe();
            let mut stopping = false;
            let mut produced = 0;
            loop {
                tokio::select! {
                    _ = async { let _ = stop.wait_for(|requested| *requested).await; }, if !stopping => {
                        stopping = true;
                        receiver.close();
                    }
                    frame = receiver.recv() => {
                        let Some(frame) = frame else { break };
                        if let Ok(frame) = &frame {
                            produced += frame.raw_mic.len().max(frame.raw_speaker.len()) as u64;
                        }
                        yield frame;
                        // The producer polls again only after admitting the previous packet.
                        admitted.fetch_add(1, Ordering::SeqCst);
                    }
                }
            }
            provider.completed.send_replace(Some(hypr_audio_actual::CaptureDrainReceipt { frames: produced }));
        })
        .with_drain_control(drain)
    }
}
impl hypr_audio_actual::AudioProvider for FakeAudio {
    fn open_capture(
        &self,
        _: hypr_audio_actual::CaptureConfig,
    ) -> Result<hypr_audio_actual::CaptureStream, hypr_audio_actual::Error> {
        panic!("wrong stereo layout")
    }
    fn open_speaker_capture(
        &self,
        rate: u32,
        chunk: usize,
    ) -> Result<hypr_audio_actual::CaptureStream, hypr_audio_actual::Error> {
        assert_eq!((rate, chunk), (16_000, 1_920));
        Ok(self.stream())
    }
    fn open_mic_capture(
        &self,
        _: Option<String>,
        _: u32,
        _: usize,
    ) -> Result<hypr_audio_actual::CaptureStream, hypr_audio_actual::Error> {
        panic!("wrong microphone layout")
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
struct NoLocalRuntime;
impl hypr_storage::StorageRuntime for NoLocalRuntime {
    fn global_base(&self) -> Result<std::path::PathBuf, hypr_storage::Error> {
        panic!("opened default local storage")
    }
    fn vault_base(&self) -> Result<std::path::PathBuf, hypr_storage::Error> {
        panic!("opened default local vault")
    }
}
impl DeviceListenerRuntime for NoLocalRuntime {
    fn emit_lifecycle(&self, _: DeviceSessionLifecycleEvent) {
        panic!("emitted a legacy local lifecycle")
    }
    fn emit_progress(&self, _: tauri_plugin_transcription::DeviceSessionProgressEvent) {
        panic!("emitted legacy local progress")
    }
    fn emit_error(&self, _: tauri_plugin_transcription::DeviceSessionErrorEvent) {
        panic!("emitted a legacy local error")
    }
    fn emit_data(&self, _: tauri_plugin_transcription::DeviceSessionDataEvent) {
        panic!("emitted legacy local data")
    }
}

#[tokio::test]
async fn real_device_helper_drains_realistic_fake_audio_into_reopenable_exact_spool_bytes() {
    let _capture = CAPTURE_TEST_LOCK.lock().await;
    use hypr_loofah_client::capture_spool::CaptureChannel;
    use tauri_plugin_transcription::{DeviceCaptureRoot, DeviceCaptureRootArgs};
    let (temp, spool) = spool().await;
    let observer = Arc::new(Observer::default());
    let identity = identity();
    let format = CaptureFormat {
        channels: vec![CaptureChannel::SystemAudio],
        ..CaptureFormat::stereo()
    };
    let prepared =
        PreparedDeviceCapture::prepare(spool.clone(), identity.clone(), format, observer.clone())
            .await
            .unwrap();
    let (frames, receiver) = tokio::sync::mpsc::channel(32);
    let admitted = Arc::new(AtomicUsize::new(0));
    let audio = Arc::new(FakeAudio {
        receiver: Mutex::new(Some(receiver)),
        admitted: admitted.clone(),
    });
    let (root, root_handle) = ractor::Actor::spawn(
        Some(DeviceCaptureRoot::name()),
        DeviceCaptureRoot,
        DeviceCaptureRootArgs {
            maintenance: Default::default(),
            runtime: Arc::new(NoLocalRuntime),
            audio,
        },
    )
    .await
    .unwrap();
    let key = prepared.start().await.unwrap();
    let mut expected = Vec::new();
    for packet in 0..10 {
        let count = if packet == 9 { 777 } else { 1_920 };
        let value = if packet % 2 == 0 { -0.0_f32 } else { -0.25_f32 };
        let speaker = vec![value; count];
        expected.extend(speaker.iter().flat_map(|sample| sample.to_le_bytes()));
        frames
            .send(Ok(hypr_audio_actual::CaptureFrame {
                raw_mic: Arc::from([]),
                raw_speaker: Arc::from(speaker),
                aec_mic: None,
            }))
            .await
            .unwrap();
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while admitted.load(Ordering::SeqCst) != 10 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(tauri_plugin_transcription::stop_device_capture(key).await);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if observer
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|event| matches!(event.event, DeviceCaptureEvent::Sealed(_)))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    root.stop(None);
    root_handle.await.unwrap();
    drop(frames);
    drop(spool);
    let reopened = CaptureSpool::open(
        temp.path().canonicalize().unwrap().join("device"),
        CaptureSpoolLimits::default(),
    )
    .await
    .unwrap();
    let writer = reopened.resume(identity.clone()).await.unwrap();
    let snapshot = writer.snapshot().await.unwrap();
    assert_eq!(snapshot.frames, 9 * 1_920 + 777);
    assert_eq!(snapshot.chunk_count, 2);
    assert_eq!(
        snapshot.seal.as_ref().unwrap().pcm_bytes as usize,
        expected.len()
    );
    assert!(snapshot.session_id.is_none());
    let mut actual = Vec::new();
    for descriptor in writer.chunks(None, 10).await.unwrap() {
        actual.extend(writer.read_chunk(descriptor.sequence).await.unwrap().bytes);
    }
    assert_eq!(actual, expected);
    assert!(
        observer
            .0
            .lock()
            .unwrap()
            .iter()
            .all(|event| event.identity == identity)
    );
    assert!(
        !observer
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event.event, DeviceCaptureEvent::Interrupted { .. }))
    );
}

#[tokio::test]
async fn empty_abandoned_capture_identity_is_not_restarted_after_a_gap() {
    let _capture = CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let identity = identity();
    spool
        .begin(identity.clone(), CaptureFormat::stereo())
        .await
        .unwrap();
    let result = PreparedDeviceCapture::prepare(
        spool,
        identity,
        CaptureFormat::stereo(),
        Arc::new(Observer::default()),
    )
    .await;
    assert!(matches!(result, Err(error) if error.0.contains("Recover the previous capture")));
}

#[tokio::test]
async fn concurrent_prepare_claims_one_device_capture_identity() {
    let _capture = CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let identity = identity();
    let observer = Arc::new(Observer::default());
    let (first, second) = tokio::join!(
        PreparedDeviceCapture::prepare(
            spool.clone(),
            identity.clone(),
            CaptureFormat::stereo(),
            observer.clone()
        ),
        PreparedDeviceCapture::prepare(spool, identity, CaptureFormat::stereo(), observer),
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);
}

#[tokio::test]
async fn managed_native_lifecycle_requires_registered_idle_actor_and_original_persistence_owner() {
    use hypr_loofah_core::{
        VaultContext, local_cli::LocalCli, local_session_work::LocalSessionWorkRegistry,
        search::SearchService, settings::ConfigState, startup::StartupState,
    };
    use hypr_loofah_protocol::{LocalCliOperation, LocalCliReply};
    use hypr_storage::vault::{LeaseAttempt, VaultLease};
    use tauri_plugin_transcription::{
        BatchSessionRegistry, CaptureMaintenanceGate, DeviceCaptureRoot, DeviceCaptureRootArgs,
    };

    let _capture = CAPTURE_TEST_LOCK.lock().await;
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let vault = root.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    let LeaseAttempt::Acquired(lease) = VaultLease::try_acquire_existing(&vault).unwrap() else {
        panic!("fixture lease")
    };
    let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Note","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
    store
        .create_note_session(&meta, "Preserve note")
        .await
        .unwrap();
    let source = vault.join("sessions/note");
    std::fs::write(source.join("audio_mic.wav"), b"stale microphone").unwrap();
    std::fs::write(source.join("user.bin"), b"private attachment").unwrap();
    let context = Arc::new(
        VaultContext::new(
            "local:managed-lifecycle".into(),
            store,
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.path().join("operations"))
        .with_vault_lease(lease.clone())
        .unwrap(),
    );
    let registry = LocalSessionWorkRegistry::new(context.clone());
    let gate = CaptureMaintenanceGate::default();
    let app = tauri::test::mock_builder()
        .manage(registry.clone())
        .manage(gate.clone())
        .manage(Arc::new(BatchSessionRegistry::default()))
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    crate::local_owner::configure_lifecycle(app.handle(), &context).unwrap();
    let cli = Arc::new(LocalCli::new(context.clone()).unwrap());
    let lifecycle = || LocalCliOperation::SessionLifecycle {
        session_id: "note".into(),
    };
    assert!(
        cli.execute(
            uuid::Uuid::new_v4().to_string(),
            lifecycle(),
            None,
            Arc::new(|_| {})
        )
        .await
        .is_err()
    );
    let (_frames, receiver) = tokio::sync::mpsc::channel(1);
    let (actor, actor_task) = ractor::Actor::spawn(
        Some(DeviceCaptureRoot::name()),
        DeviceCaptureRoot,
        DeviceCaptureRootArgs {
            maintenance: gate,
            runtime: Arc::new(NoLocalRuntime),
            audio: Arc::new(FakeAudio {
                receiver: Mutex::new(Some(receiver)),
                admitted: Arc::new(AtomicUsize::new(0)),
            }),
        },
    )
    .await
    .unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let work = registry
        .begin("note".into(), request.clone())
        .await
        .unwrap();
    assert!(
        cli.execute(
            uuid::Uuid::new_v4().to_string(),
            lifecycle(),
            None,
            Arc::new(|_| {})
        )
        .await
        .is_err()
    );
    work.producer_finished().unwrap();
    assert!(
        cli.execute(
            uuid::Uuid::new_v4().to_string(),
            lifecycle(),
            None,
            Arc::new(|_| {})
        )
        .await
        .is_err()
    );
    registry.persistence_finished("note", &request).unwrap();
    let LocalCliReply::SessionLifecycle { revision, .. } = cli
        .execute(
            uuid::Uuid::new_v4().to_string(),
            lifecycle(),
            None,
            Arc::new(|_| {}),
        )
        .await
        .unwrap()
        .reply
    else {
        panic!("lifecycle")
    };
    let LocalCliReply::SessionDeleted { trash_path, .. } = cli
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::DeleteSession {
                session_id: "note".into(),
                expected_revision: revision,
            },
            None,
            Arc::new(|_| {}),
        )
        .await
        .unwrap()
        .reply
    else {
        panic!("deletion")
    };
    assert!(!source.exists());
    assert_eq!(
        std::fs::read(trash_path.join("audio_mic.wav")).unwrap(),
        b"stale microphone"
    );
    assert_eq!(
        std::fs::read(trash_path.join("user.bin")).unwrap(),
        b"private attachment"
    );
    assert_eq!(
        std::fs::read(trash_path.join("notes.md")).unwrap(),
        b"Preserve note"
    );
    assert!(matches!(
        VaultLease::try_acquire_existing(&vault).unwrap(),
        LeaseAttempt::Busy
    ));
    actor.stop(None);
    actor_task.await.unwrap();
}

#[tokio::test]
async fn mixed_append_sink_downmixes_native_stereo_without_changing_frames() {
    let _capture = CAPTURE_TEST_LOCK.lock().await;
    let (_temp, spool) = spool().await;
    let id = identity();
    let mut format = CaptureFormat::stereo();
    format.channels = vec![CaptureChannel::Mixed];
    let prepared = PreparedDeviceCapture::prepare(
        spool.clone(),
        id.clone(),
        format.clone(),
        Arc::new(Observer::default()),
    )
    .await
    .unwrap();
    assert!(matches!(
        prepared.args.mode,
        DeviceChannelMode::MicAndSpeaker
    ));
    let samples = [0.75f32, -0.25, f32::MAX, f32::MAX, -0.5, 0.5];
    let pcm: Vec<_> = samples.into_iter().flat_map(f32::to_le_bytes).collect();
    assert!(
        prepared
            .sink
            .append(0, 3, Arc::from(pcm[..12].to_vec()))
            .await
            .is_err()
    );
    let invalid: Vec<_> = [f32::NAN, 0.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    assert!(prepared.sink.append(0, 1, invalid.into()).await.is_err());
    let receipt = prepared.sink.append(0, 3, pcm.into()).await.unwrap();
    assert_eq!(receipt.frames, 3);
    let done = prepared
        .sink
        .finish(DeviceRecordingEnd::Complete)
        .await
        .unwrap();
    assert_eq!(done.frames, 3);
    assert_eq!(done.pcm_bytes, 12);
    let writer = spool.resume(id).await.unwrap();
    let bytes = writer.read_chunk(0).await.unwrap().bytes;
    let expected: Vec<_> = [0.25f32, f32::MAX, 0.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    assert_eq!(bytes, expected);
    assert_eq!(writer.snapshot().await.unwrap().format, format);
}
