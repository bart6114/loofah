use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use std::{
    fs::File,
    path::Path,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::{Notify, Semaphore, mpsc, watch};

const MEMBER: &str = "6114bd31-62da-4d90-9a0d-41b9a8872201";

#[derive(Clone)]
pub(crate) struct Fake {
    slots: Arc<Semaphore>,
    calls: Arc<AtomicUsize>,
    frames: Arc<Mutex<Vec<u32>>>,
    released: Arc<Semaphore>,
    joined: Arc<Notify>,
    bad_receipt: bool,
}
impl Fake {
    pub(crate) fn new() -> Self {
        Self {
            slots: Arc::new(Semaphore::new(1)),
            calls: Arc::new(AtomicUsize::new(0)),
            frames: Default::default(),
            released: Arc::new(Semaphore::new(0)),
            joined: Default::default(),
            bad_receipt: false,
        }
    }
    pub(crate) fn release_processing_fixture(&self) {
        self.released.add_permits(1);
    }
}
struct Control {
    input: mpsc::Sender<(u64, Arc<[f32]>)>,
    next: AtomicU64,
    channels: usize,
    stop: CancellationToken,
    finish: std::sync::atomic::AtomicBool,
    wake: Notify,
}
impl CaptureLiveControl for Control {
    fn try_accept(
        &self,
        first: u64,
        pcm: &Arc<[f32]>,
    ) -> Result<CaptureLiveIngress, CaptureLiveFailure> {
        if first != self.next.load(Ordering::Acquire) {
            return Err(CaptureLiveFailure::SourceChanged);
        }
        match self.input.try_send((first, pcm.clone())) {
            Ok(()) => {
                self.next
                    .fetch_add((pcm.len() / self.channels) as u64, Ordering::AcqRel);
                Ok(CaptureLiveIngress::Accepted)
            }
            Err(mpsc::error::TrySendError::Full(_)) => Ok(CaptureLiveIngress::Busy),
            Err(_) => Err(CaptureLiveFailure::Interrupted),
        }
    }
    fn finish(&self, end: u64) -> Result<(), CaptureLiveFailure> {
        if self.next.load(Ordering::Acquire) != end {
            return Err(CaptureLiveFailure::SourceChanged);
        }
        self.finish.store(true, Ordering::Release);
        self.wake.notify_one();
        Ok(())
    }
    fn cancel(&self) {
        self.stop.cancel();
    }
}
impl CaptureLiveEngine for Fake {
    fn start(
        &self,
        spec: CaptureLiveEngineSpec,
        owner: Box<dyn Send>,
    ) -> crate::transcription_jobs::TranscriptionFuture<
        '_,
        Result<CaptureLiveSession, CaptureLiveFailure>,
    > {
        Box::pin(async move {
            let permit = self
                .slots
                .clone()
                .try_acquire_owned()
                .map_err(|_| CaptureLiveFailure::ResourceLimit)?;
            self.calls.fetch_add(1, Ordering::AcqRel);
            let (input, mut frames) = mpsc::channel(8);
            let control = Arc::new(Control {
                input,
                next: 0.into(),
                channels: spec.format.channels.len(),
                stop: CancellationToken::new(),
                finish: false.into(),
                wake: Notify::new(),
            });
            let inner = control.clone();
            let (captions, output) = mpsc::channel(16);
            let (done, receipt) = watch::channel(None);
            let fake = self.clone();
            tokio::spawn(async move {
                let mut count = 0;
                let mut consumed = 0;
                let mut failure = None;
                loop {
                    if inner.finish.load(Ordering::Acquire) && frames.is_empty() {
                        break;
                    }
                    tokio::select! {
                        _ = inner.stop.cancelled() => {failure=Some(CaptureLiveFailure::Interrupted); break;},
                        _ = inner.wake.notified() => {},
                        frame = frames.recv() => {
                            let Some((first, pcm)) = frame else {failure=Some(CaptureLiveFailure::Interrupted);break;};
                            fake.frames.lock().unwrap().extend(pcm.iter().map(|v|v.to_bits()));
                            consumed = first + (pcm.len()/inner.channels) as u64;
                            for (channel, role) in spec.format.channels.iter().enumerate() {
                                let canonical = if inner.channels == 1 {2} else {channel};
                                let word = serde_json::from_value(serde_json::json!({
                                    "id":uuid::Uuid::new_v4().to_string(), "text":format!(" word{channel}"),
                                    "start_ms":first*1000/16000,"end_ms":consumed*1000/16000,
                                    "channel":canonical,"state":"final"
                                })).unwrap();
                                if captions.try_send(CaptureLiveCaption {generation_id:spec.generation_id.clone(),sequence:count,physical_channel:channel as u8,role:*role,new_words:vec![word],partials:vec![],replaced_ids:vec![],languages:vec!["en".into()]}).is_err() {
                                    failure=Some(CaptureLiveFailure::InvalidOutput); break;
                                }
                                count += 1;
                            }
                        }
                    }
                }
                fake.joined.notify_one();
                let _release = fake.released.acquire().await.unwrap();
                drop(permit);
                drop(owner);
                done.send_replace(Some(Arc::new(CaptureLiveEngineReceipt {
                    generation_id: spec.generation_id,
                    admitted_end_frame: inner.next.load(Ordering::Acquire),
                    input_consumed_end_frame: consumed,
                    completed_through_frame: failure.is_none().then_some(consumed),
                    caption_count: count + u64::from(fake.bad_receipt),
                    failure,
                    native_joined: true,
                    owners_released: true,
                })));
            });
            Ok(CaptureLiveSession {
                control,
                captions: output,
                completion: receipt,
            })
        })
    }
}

struct Auth {
    revoked: CancellationToken,
    interrupted: CancellationToken,
    deny: std::sync::atomic::AtomicBool,
    before_authorize: Mutex<Option<(tokio::sync::oneshot::Sender<()>, Arc<Semaphore>)>>,
}
impl Auth {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            revoked: CancellationToken::new(),
            interrupted: CancellationToken::new(),
            deny: false.into(),
            before_authorize: Mutex::new(None),
        })
    }
}
impl CaptureLiveAuthorizer for Auth {
    fn observe<'a>(
        &'a self,
        _: &'a CaptureLiveWork,
    ) -> crate::transcription_jobs::TranscriptionFuture<
        'a,
        Result<crate::transcription_jobs::TranscriptionAccess, TranscriptionFailure>,
    > {
        Box::pin(async move {
            Ok(crate::transcription_jobs::TranscriptionAccess {
                cancelled: self.revoked.clone(),
                interrupted: self.interrupted.clone(),
                owner: Box::new(()),
            })
        })
    }
    fn authorize<'a>(
        &'a self,
        _: &'a CaptureLiveWork,
    ) -> crate::transcription_jobs::TranscriptionFuture<
        'a,
        Result<Box<dyn Send>, TranscriptionFailure>,
    > {
        Box::pin(async move {
            let pause = self.before_authorize.lock().unwrap().take();
            if let Some((entered, release)) = pause {
                let _ = entered.send(());
                let _release = release.acquire().await.unwrap();
            }

            if self.deny.load(Ordering::Acquire) {
                Err(TranscriptionFailure::AccessDenied)
            } else {
                Ok(Box::new(()) as Box<dyn Send>)
            }
        })
    }
}

async fn context(root: &Path, fake: Fake, auth: Arc<Auth>) -> Arc<VaultContext> {
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(root.join("lease"))
        .unwrap();
    file.try_lock().unwrap();
    let mut context = VaultContext::new(
        "vault".into(),
        Arc::new(hypr_vault_write::SessionStore::new(root.into())),
        StartupState::new(Some(root)),
        Arc::new(ConfigState::load_or_default(root)),
        Arc::new(SearchService::default()),
    )
    .with_operation_storage(root.join("operations"));
    context.lease = Some(file.into());
    context
        .enable_capture_ingestion("https://capture.example/".into(), Default::default())
        .unwrap();
    let (lease, gates) = VaultContext::bootstrap_recording_publications(
        context.store.clone(),
        "vault".into(),
        root.join("operations/recording-publications"),
        context.lease.take().unwrap(),
    )
    .await
    .unwrap();
    context.lease = Some(lease);
    context.recording_publications = gates;
    let context = Arc::new(context);
    let metadata = serde_json::from_value(serde_json::json!({"id":"session","title":"Saved note","created_at":"2026-11-08T00:00:00Z","tags":[]})).unwrap();
    context
        .store
        .create_note_session(&metadata, "Keep the user's notes")
        .await
        .unwrap();
    let journal = ledger::Journal::open(root.join("live"), context.clone())
        .await
        .unwrap();
    context
        .capture_live
        .set(Arc::new(CaptureLiveService {
            journal,
            engine: Arc::new(fake),
            authorizer: auth,
            stop: CancellationToken::new(),
            tasks: Mutex::new(BTreeMap::new()),
            admissions: Arc::new(Semaphore::new(2)),
            failed: false.into(),
        }))
        .ok()
        .unwrap();
    context
}

async fn begin(context: &Arc<VaultContext>) -> (CaptureIngestRequest, CaptureLiveRequest, String) {
    let input = CaptureIngestRequest {
        identity: CaptureIdentity {
            scope: CaptureScope {
                server: "https://capture.example/".into(),
                member_id: MEMBER.into(),
                vault_id: "vault".into(),
            },
            capture_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            target: CaptureTarget::ExistingSession {
                session_id: "session".into(),
                expected_audio_sha256: None,
            },
        },
        format: CaptureFormat::stereo(),
        first_admitted_audio_observed_at_ms: 1_794_139_200_123,
    };
    let status = context
        .begin_capture_authorized(MEMBER.into(), input.clone(), || async { Ok(()) })
        .await
        .unwrap();
    let request = CaptureLiveRequest {
        capture_fingerprint: status.prefix.capture_fingerprint,
        expected_transcript: AbsentTranscriptExpectation::Absent,
        model: "fake".into(),
        languages: vec!["en".into()],
        keywords: vec![],
    };
    (input, request, uuid::Uuid::new_v4().to_string())
}

async fn append(
    context: &Arc<VaultContext>,
    input: &CaptureIngestRequest,
    seconds: u32,
) -> Vec<u8> {
    let writer = context
        .capture_writer(MEMBER.into(), input.identity.capture_id.clone())
        .await
        .unwrap();
    let bytes: Vec<_> = (0..16000 * seconds * 2)
        .flat_map(|n| ((n % 37) as f32 / 37.0).to_le_bytes())
        .collect();
    let descriptor = writer
        .append_chunk(0, 16000 * seconds, bytes.clone())
        .await
        .unwrap();
    let replay = context
        .append_capture_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            descriptor,
            bytes.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    assert_eq!(replay.frames, u64::from(16000 * seconds));
    bytes
}

async fn until(
    context: &Arc<VaultContext>,
    capture: &str,
    id: &str,
    condition: impl Fn(&CaptureLiveSnapshot) -> bool,
) -> CaptureLiveSnapshot {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            match context
                .capture_live_status_authorized(
                    MEMBER.into(),
                    capture.into(),
                    id.into(),
                    || async { Ok(()) },
                )
                .await
            {
                Ok(snapshot) if condition(&snapshot) => return snapshot,
                Ok(_) => {}
                Err(cause) if cause.code == ErrorCode::Unavailable => {}
                Err(cause) => panic!("{cause:?}"),
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn captions_are_durable_before_seal_and_replay_does_not_restart_native() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    context
        .start_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            request.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let bytes = append(&context, &input, 1).await;
    let snapshot = until(&context, &input.identity.capture_id, &id, |s| {
        s.next_caption_sequence == 2
    })
    .await;
    assert_eq!(snapshot.completed_through_frame, None);
    let fed: Vec<u8> = fake
        .frames
        .lock()
        .unwrap()
        .iter()
        .flat_map(|bits| bits.to_le_bytes())
        .collect();
    assert_eq!(fed, bytes);
    assert!(context.ensure_capture_processing_ready("session").is_err());
    let page = context
        .capture_live_captions_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            None,
            128,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    assert_eq!(page.captions.len(), 2);
    assert!(
        !context
            .store
            .vault_base()
            .join("sessions/session/transcript.json")
            .exists()
    );
    let replay = context
        .start_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            request.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    assert_eq!(replay.next_caption_sequence, 2);
    assert_eq!(fake.calls.load(Ordering::Acquire), 1);
    let mut changed = request;
    changed.languages = vec!["nl".into()];
    assert_eq!(
        context
            .start_capture_live_authorized(
                MEMBER.into(),
                input.identity.capture_id.clone(),
                id.clone(),
                changed,
                || async { Ok(()) }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    context
        .cancel_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    fake.joined.notified().await;
    assert_eq!(fake.slots.available_permits(), 0);
    fake.released.add_permits(1);
    until(&context, &input.identity.capture_id, &id, |s| {
        s.state.is_terminal()
    })
    .await;
    let writer = context
        .capture_writer(MEMBER.into(), input.identity.capture_id.clone())
        .await
        .unwrap();
    assert_eq!(writer.read_chunk(0).await.unwrap().bytes, bytes);
    assert!(writer.snapshot().await.unwrap().seal.is_none());
    context
        .capture_live
        .get()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

#[tokio::test]
async fn joined_receipt_and_persisted_words_promote_only_matching_published_recording() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let directory = context.store.vault_base().join("sessions/session");
    std::fs::write(directory.join("unknown.txt"), b"user attachment").unwrap();
    let (input, request, id) = begin(&context).await;
    context
        .start_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            request,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    append(&context, &input, 1).await;
    until(&context, &input.identity.capture_id, &id, |s| {
        s.next_caption_sequence == 2
    })
    .await;
    let writer = context
        .capture_writer(MEMBER.into(), input.identity.capture_id.clone())
        .await
        .unwrap();
    let seal = writer.seal().await.unwrap();
    fake.joined.notified().await;
    assert!(writer.snapshot().await.unwrap().publication.is_none());
    fake.released.add_permits(1);
    until(&context, &input.identity.capture_id, &id, |s| {
        s.state == CaptureLiveState::AwaitingRecording
    })
    .await;
    context
        .publish_capture_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            seal,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let snapshot = until(&context, &input.identity.capture_id, &id, |s| {
        s.state.is_terminal()
    })
    .await;
    assert_eq!(snapshot.state, CaptureLiveState::Completed);
    assert_eq!(snapshot.completed_through_frame, Some(16000));
    let bytes = std::fs::read(directory.join("transcript.json")).unwrap();
    let transcript: hypr_vault_read::TranscriptJson = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(transcript.transcripts[0].user_id, MEMBER);
    assert_eq!(
        transcript.transcripts[0].started_at,
        input.first_admitted_audio_observed_at_ms as f64
    );
    assert_eq!(transcript.transcripts[0].words.len(), 2);
    assert_eq!(
        std::fs::read(directory.join("unknown.txt")).unwrap(),
        b"user attachment"
    );
    let retained = context
        .capture_live_result_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id,
            || async { Ok(()) },
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.transcript, transcript.transcripts[0]);
    assert!(context.ensure_capture_processing_ready("session").is_ok());
    context
        .capture_live
        .get()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

#[tokio::test]
async fn revoked_membership_cancels_only_live_and_shutdown_retains_native_owner() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let auth = Auth::new();
    let context = context(&root, fake.clone(), auth.clone()).await;
    let (input, request, id) = begin(&context).await;
    context
        .start_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            request,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let bytes = append(&context, &input, 1).await;
    until(&context, &input.identity.capture_id, &id, |s| {
        s.next_caption_sequence == 2
    })
    .await;
    auth.revoked.cancel();
    fake.joined.notified().await;
    assert_eq!(fake.slots.available_permits(), 0);
    let service = context.capture_live.get().unwrap().clone();
    let shutdown = tokio::spawn(async move {
        service.shutdown().await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(!shutdown.is_finished());
    fake.released.add_permits(1);
    shutdown.await.unwrap();
    let snapshot = until(&context, &input.identity.capture_id, &id, |s| {
        s.state.is_terminal()
    })
    .await;
    assert_eq!(snapshot.failure, Some(CaptureLiveFailure::AccessDenied));
    assert_eq!(snapshot.completed_through_frame, None);
    let writer = context
        .capture_writer(MEMBER.into(), input.identity.capture_id.clone())
        .await
        .unwrap();
    assert_eq!(writer.read_chunk(0).await.unwrap().bytes, bytes);
    assert!(writer.snapshot().await.unwrap().seal.is_none());
}

#[tokio::test]
async fn malformed_completion_never_claims_complete_coverage_or_publishes() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let mut fake = Fake::new();
    fake.bad_receipt = true;
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    context
        .start_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            request,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    append(&context, &input, 1).await;
    until(&context, &input.identity.capture_id, &id, |s| {
        s.next_caption_sequence == 2
    })
    .await;
    context
        .capture_writer(MEMBER.into(), input.identity.capture_id.clone())
        .await
        .unwrap()
        .seal()
        .await
        .unwrap();
    fake.joined.notified().await;
    fake.released.add_permits(1);
    let snapshot = until(&context, &input.identity.capture_id, &id, |s| {
        s.state.is_terminal()
    })
    .await;
    assert_eq!(snapshot.state, CaptureLiveState::RepairRequired);
    assert_eq!(snapshot.completed_through_frame, None);
    assert!(
        !context
            .store
            .vault_base()
            .join("sessions/session/transcript.json")
            .exists()
    );
    context
        .capture_live
        .get()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

#[tokio::test]
async fn generation_delivery_is_capture_and_member_scoped_and_denial_precedes_ledger_io() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    assert_eq!(
        context
            .start_capture_live_authorized(
                MEMBER.into(),
                input.identity.capture_id.clone(),
                id.clone(),
                request.clone(),
                || async { Err::<(), _>(error(ErrorCode::Forbidden, "denied")) }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    assert_eq!(fake.calls.load(Ordering::Acquire), 0);
    assert!(!root.join("live").join(&id).exists());
    context
        .start_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            request,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    assert_eq!(
        context
            .capture_live_status_authorized(
                "other".into(),
                input.identity.capture_id.clone(),
                id.clone(),
                || async { Ok(()) }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert_eq!(
        context
            .capture_live_status_authorized(
                MEMBER.into(),
                uuid::Uuid::new_v4().to_string(),
                id.clone(),
                || async { Ok(()) }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    context
        .cancel_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    fake.released.add_permits(1);
    until(&context, &input.identity.capture_id, &id, |s| {
        s.state.is_terminal()
    })
    .await;
    context
        .capture_live
        .get()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

#[tokio::test]
async fn ledger_restart_interrupts_without_restarting_engine_and_rejects_future_or_duplicate_manifest()
 {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    let path = root.join("restart-live");
    let journal = ledger::Journal::open(path.clone(), context.clone())
        .await
        .unwrap();
    let record = ledger::Record::new(
        input.identity,
        input.format,
        request,
        id.clone(),
        "session".into(),
        1234,
        "",
    );
    journal.admit(record, String::new()).await.unwrap();
    drop(journal);
    let reopened = ledger::Journal::open(path.clone(), context.clone())
        .await
        .unwrap();
    let record = reopened.get(MEMBER.into(), id.clone()).await.unwrap();
    assert_eq!(record.snapshot.state, CaptureLiveState::Interrupted);
    assert_eq!(record.snapshot.completed_through_frame, None);
    assert_eq!(fake.calls.load(Ordering::Acquire), 0);
    drop(reopened);
    let manifest = path.join(id).join("record.json");
    let original = std::fs::read(&manifest).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    value["version"] = serde_json::json!(999);
    let future = serde_json::to_vec(&value).unwrap();
    std::fs::write(&manifest, &future).unwrap();
    assert!(
        ledger::Journal::open(path.clone(), context.clone())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&manifest).unwrap(), future);
    let duplicate = String::from_utf8(original)
        .unwrap()
        .replacen('{', "{\"version\":1,", 1);
    std::fs::write(&manifest, duplicate.as_bytes()).unwrap();
    assert!(ledger::Journal::open(path, context.clone()).await.is_err());
    assert_eq!(std::fs::read(&manifest).unwrap(), duplicate.as_bytes());
}

#[tokio::test]
async fn abandoned_start_callers_retain_bounded_admission_until_owned_preparation_settles() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    let gate = Arc::new(Semaphore::new(0));
    let entered = Arc::new(Semaphore::new(0));
    let mut callers = Vec::new();
    for _ in 0..2 {
        let context = context.clone();
        let request = request.clone();
        let capture = input.identity.capture_id.clone();
        let id = id.clone();
        let gate = gate.clone();
        let entered = entered.clone();
        callers.push(tokio::spawn(async move {
            context
                .start_capture_live_authorized(
                    MEMBER.into(),
                    capture,
                    id,
                    request,
                    move || async move {
                        entered.add_permits(1);
                        let permit = gate.acquire_owned().await.unwrap();
                        permit.forget();
                        Ok(())
                    },
                )
                .await
        }));
    }
    entered.acquire_many(2).await.unwrap().forget();
    for caller in callers {
        caller.abort();
        let _ = caller.await;
    }
    let unexpected = Arc::new(AtomicUsize::new(0));
    let observed = unexpected.clone();
    let third = context
        .start_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            uuid::Uuid::new_v4().to_string(),
            request,
            move || async move {
                observed.fetch_add(1, Ordering::AcqRel);
                Ok(())
            },
        )
        .await
        .unwrap_err();
    assert_eq!(third.code, ErrorCode::Unavailable);
    assert_eq!(unexpected.load(Ordering::Acquire), 0);
    gate.add_permits(2);
    tokio::time::timeout(Duration::from_secs(15), async {
        while fake.calls.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fake.calls.load(Ordering::Acquire), 1);
    context
        .cancel_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    fake.released.add_permits(1);
    until(&context, &input.identity.capture_id, &id, |s| {
        s.state.is_terminal()
    })
    .await;
    context
        .capture_live
        .get()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

#[tokio::test]
async fn future_caption_payload_and_prospective_quota_failure_preserve_existing_bytes() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    context
        .start_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            request,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    append(&context, &input, 1).await;
    until(&context, &input.identity.capture_id, &id, |s| {
        s.next_caption_sequence == 2
    })
    .await;
    let manifest = root.join("live").join(&id).join("record.json");
    let before = std::fs::read(&manifest).unwrap();
    assert!(
        context
            .capture_live
            .get()
            .unwrap()
            .journal
            .change(id.clone(), |record| record.snapshot.next_caption_sequence =
                u64::MAX)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(manifest).unwrap(), before);
    let caption = root
        .join("live")
        .join(&id)
        .join("0000000000000000.caption.json");
    let mut future: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&caption).unwrap()).unwrap();
    future["caption"]["new_words"][0]["future_word_field"] = serde_json::json!("preserve");
    let bytes = serde_json::to_vec(&future).unwrap();
    std::fs::write(&caption, &bytes).unwrap();
    assert!(
        context
            .capture_live_captions_authorized(
                MEMBER.into(),
                input.identity.capture_id.clone(),
                id.clone(),
                None,
                128,
                || async { Ok(()) }
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(caption).unwrap(), bytes);
    context
        .cancel_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    fake.released.add_permits(1);
    until(&context, &input.identity.capture_id, &id, |s| {
        s.state.is_terminal()
    })
    .await;
    context
        .capture_live
        .get()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

#[tokio::test]
async fn restart_retains_pending_commit_as_uncertain_with_result_without_inference_replay() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    let path = root.join("pending-live");
    let journal = ledger::Journal::open(path.clone(), context.clone())
        .await
        .unwrap();
    let record = ledger::Record::new(
        input.identity,
        input.format,
        request,
        id.clone(),
        "session".into(),
        1234,
        "",
    );
    let (record, _) = journal.admit(record, String::new()).await.unwrap();
    journal
        .retain_result(
            id.clone(),
            hypr_vault_read::TranscriptWithData {
                id: uuid::Uuid::new_v4().to_string(),
                user_id: MEMBER.into(),
                session_id: "session".into(),
                created_at: record.created_at,
                started_at: 1234.0,
                ended_at: None,
                memo_md: String::new(),
                words: vec![],
                speaker_hints: vec![],
            },
            CaptureSeal {
                capture_fingerprint: record.request.capture_fingerprint.clone(),
                chunk_count: 0,
                frames: 0,
                pcm_bytes: 0,
                chain_digest: capture_chain_seed(&record.request.capture_fingerprint),
                source_sha256: crate::transcription_jobs::ledger::hash(b""),
            },
        )
        .await
        .unwrap();
    journal
        .change(id.clone(), |record| {
            record.snapshot.state = CaptureLiveState::Publishing;
            record.snapshot.completed_through_frame = Some(0);
            record.commit_pending = true;
        })
        .await
        .unwrap();
    assert!(
        !journal
            .get(MEMBER.into(), id.clone())
            .await
            .unwrap()
            .snapshot
            .state
            .is_terminal()
    );
    drop(journal);
    let reopened = ledger::Journal::open(path, context).await.unwrap();
    let record = reopened.get(MEMBER.into(), id.clone()).await.unwrap();
    assert_eq!(record.snapshot.state, CaptureLiveState::Uncertain);
    assert_eq!(record.snapshot.completed_through_frame, None);
    assert!(record.commit_pending);
    assert!(record.snapshot.result_available);
    assert!(reopened.result(MEMBER.into(), id).await.unwrap().is_some());
    assert_eq!(fake.calls.load(Ordering::Acquire), 0);
}

#[tokio::test]
async fn disabled_live_service_checks_retained_journal_read_only_with_bounds_and_no_model() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let mut context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, _) = begin(&context).await;
    let id = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_owned();
    let record = ledger::Record::new(
        input.identity.clone(),
        input.format.clone(),
        request,
        id.clone(),
        "session".into(),
        input.first_admitted_audio_observed_at_ms,
        "",
    );
    context
        .capture_live
        .get()
        .unwrap()
        .journal
        .admit(record, String::new())
        .await
        .unwrap();
    append(&context, &input, 1).await;
    let writer = context
        .capture_writer(MEMBER.into(), input.identity.capture_id.clone())
        .await
        .unwrap();
    let seal = writer.seal().await.unwrap();
    context
        .publish_capture_authorized(MEMBER.into(), input.identity.capture_id, seal, || async {
            Ok(())
        })
        .await
        .unwrap();
    drop(
        Arc::get_mut(&mut context)
            .unwrap()
            .capture_live
            .take()
            .unwrap(),
    );
    let retained = context
        .operations
        .as_ref()
        .unwrap()
        .root
        .join("capture-live");
    let original = root.join("live");
    std::fs::rename(&original, &retained).unwrap();
    let path = retained.join(&id).join("record.json");
    let template: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let exclusive = context.try_session_lifecycle_change("session").unwrap();
    for kind in [
        "starting",
        "interrupted",
        "uncertain",
        "future",
        "duplicate",
        "wrong_vault",
        "corrupt",
    ] {
        let mut record = template.clone();
        record["snapshot"]["state"] = serde_json::json!(if kind == "starting" {
            "starting"
        } else if kind == "uncertain" {
            "uncertain"
        } else {
            "interrupted"
        });
        if kind == "uncertain" {
            record["commit_pending"] = serde_json::json!(true);
        }
        if kind == "future" {
            record["version"] = serde_json::json!(2);
        }
        if kind == "wrong_vault" {
            record["identity"]["scope"]["vault_id"] = serde_json::json!("another-vault");
        }
        let mut bytes = serde_json::to_vec(&record).unwrap();
        if kind == "duplicate" {
            bytes.splice(1..1, b"\"version\":1,".iter().copied());
        }
        if kind == "corrupt" {
            bytes = b"{incomplete".to_vec();
        }
        std::fs::write(&path, &bytes).unwrap();
        let checked = context
            .ensure_capture_session_lifecycle_idle("session")
            .await;
        if kind == "interrupted" {
            checked.unwrap();
        } else {
            assert_eq!(
                checked.unwrap_err().mutation_outcome,
                Some(MutationOutcome::NotCommitted),
                "{kind}"
            );
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "read-only guard changed {kind}"
        );
    }
    std::fs::rename(&retained, &original).unwrap();
    context
        .ensure_capture_session_lifecycle_idle("session")
        .await
        .unwrap();
    assert!(!retained.exists());
    std::fs::rename(&original, &retained).unwrap();
    std::fs::write(&path, serde_json::to_vec(&template).unwrap()).unwrap();
    let alias = retained.join(id.to_uppercase());
    let alias_stage = retained.join("alias-stage");
    std::fs::rename(retained.join(&id), &alias_stage).unwrap();
    std::fs::rename(&alias_stage, &alias).unwrap();
    assert!(
        context
            .ensure_capture_session_lifecycle_idle("session")
            .await
            .is_err()
    );
    std::fs::rename(alias, &alias_stage).unwrap();
    std::fs::rename(&alias_stage, retained.join(&id)).unwrap();
    for number in 0..256 {
        std::fs::write(
            retained.join(format!("unknown-{number}")),
            b"preserve ignored entry",
        )
        .unwrap();
    }
    assert!(
        context
            .ensure_capture_session_lifecycle_idle("session")
            .await
            .is_err()
    );
    assert_eq!(fake.calls.load(Ordering::Acquire), 0);
    assert!(writer.snapshot().await.unwrap().publication.is_some());
    assert_eq!(
        std::fs::read(retained.join("unknown-0")).unwrap(),
        b"preserve ignored entry"
    );
    drop(exclusive);
}

#[tokio::test]
async fn already_sealed_capture_never_starts_a_substitute_live_generation() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    context
        .capture_writer(MEMBER.into(), input.identity.capture_id.clone())
        .await
        .unwrap()
        .seal()
        .await
        .unwrap();
    let error = context
        .start_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id,
            id.clone(),
            request,
            || async { Ok(()) },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(fake.calls.load(Ordering::Acquire), 0);
    assert!(!root.join("live").join(id).exists());
}

#[tokio::test]
async fn lifecycle_exclusion_keeps_live_and_uncertain_generations_blocked_after_raw_publication() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    let record = ledger::Record::new(
        input.identity.clone(),
        input.format.clone(),
        request,
        id.clone(),
        "session".into(),
        input.first_admitted_audio_observed_at_ms,
        "",
    );
    let service = context.capture_live.get().unwrap();
    service.journal.admit(record, String::new()).await.unwrap();
    append(&context, &input, 1).await;
    let writer = context
        .capture_writer(MEMBER.into(), input.identity.capture_id.clone())
        .await
        .unwrap();
    let seal = writer.seal().await.unwrap();
    context
        .publish_capture_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            seal,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    assert!(writer.snapshot().await.unwrap().publication.is_some());
    let exclusive = context.try_session_lifecycle_change("session").unwrap();
    assert!(
        context
            .ensure_capture_session_lifecycle_idle("session")
            .await
            .is_err()
    );
    service
        .journal
        .change(id.clone(), |r| {
            r.snapshot.state = CaptureLiveState::Interrupted;
            r.snapshot.failure = Some(CaptureLiveFailure::Interrupted);
        })
        .await
        .unwrap();
    context
        .ensure_capture_session_lifecycle_idle("session")
        .await
        .unwrap();
    service
        .journal
        .change(id.clone(), |r| {
            r.snapshot.state = CaptureLiveState::Uncertain;
            r.snapshot.failure = Some(CaptureLiveFailure::Storage);
            r.commit_pending = true;
        })
        .await
        .unwrap();
    assert!(
        context
            .ensure_capture_session_lifecycle_idle("session")
            .await
            .is_err()
    );
    service
        .journal
        .change(id, |r| {
            r.snapshot.state = CaptureLiveState::Interrupted;
            r.snapshot.failure = Some(CaptureLiveFailure::Interrupted);
            r.commit_pending = false;
        })
        .await
        .unwrap();
    service.failed.store(true, Ordering::Release);
    assert!(
        context
            .ensure_capture_session_lifecycle_idle("session")
            .await
            .is_err()
    );
    assert_eq!(fake.calls.load(Ordering::Acquire), 0);
    drop(exclusive);
}

#[tokio::test]
async fn dropped_live_start_waiter_respects_exclusive_lifecycle_and_durably_admits_before_inference()
 {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    let exclusive = context.try_session_lifecycle_change("session").unwrap();
    let entered = Arc::new(Notify::new());
    let owned = context.clone();
    let capture = input.identity.capture_id.clone();
    let generation = id.clone();
    let mut caller = tokio::spawn({
        let entered = entered.clone();
        async move {
            owned
                .start_capture_live_authorized(
                    MEMBER.into(),
                    capture,
                    generation,
                    request,
                    move || async move {
                        entered.notify_one();
                        Ok(())
                    },
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(25), &mut caller)
            .await
            .is_err()
    );
    assert_eq!(fake.calls.load(Ordering::Acquire), 0);
    assert!(!root.join("live").join(&id).exists());
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    drop(exclusive);
    tokio::time::timeout(Duration::from_secs(10), async {
        while fake.calls.load(Ordering::Acquire) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(context.try_session_lifecycle_change("session").is_err());
    assert!(
        context
            .ensure_capture_session_lifecycle_idle("session")
            .await
            .is_err()
    );
    fake.released.add_permits(1);
    context
        .cancel_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    until(&context, &input.identity.capture_id, &id, |s| {
        s.state.is_terminal()
    })
    .await;
    context
        .capture_live
        .get()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
    let _exclusive = context.try_session_lifecycle_change("session").unwrap();
}

#[tokio::test]
async fn saturated_public_ledger_reads_do_not_fail_or_restart_the_owned_live_worker() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let fake = Fake::new();
    fake.release_processing_fixture();
    let context = context(&root, fake.clone(), Auth::new()).await;
    let (input, request, id) = begin(&context).await;
    context
        .start_capture_live_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            id.clone(),
            request,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    until(&context, &input.identity.capture_id, &id, |snapshot| {
        snapshot.state == CaptureLiveState::Running
    })
    .await;
    let service = context.capture_live.get().unwrap();
    let busy = service.journal.hold_io().await;
    append(&context, &input, 1).await;
    tokio::time::timeout(
        Duration::from_secs(5),
        service.journal.wait_for_worker_admission(),
    )
    .await
    .unwrap();
    assert_eq!(
        service
            .journal
            .get(MEMBER.into(), id.clone())
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Unavailable
    );
    assert_eq!(
        service
            .journal
            .page(MEMBER.into(), id.clone(), None, 16)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Unavailable
    );
    assert!(!service.failed.load(Ordering::Acquire));
    assert!(
        !service
            .tasks
            .lock()
            .unwrap()
            .get(&id)
            .unwrap()
            .handle
            .is_finished()
    );
    assert_eq!(fake.calls.load(Ordering::Acquire), 1);
    drop(busy);
    until(&context, &input.identity.capture_id, &id, |snapshot| {
        snapshot.next_caption_sequence == 2
    })
    .await;
    let writer = context
        .capture_writer(MEMBER.into(), input.identity.capture_id.clone())
        .await
        .unwrap();
    let seal = writer.seal().await.unwrap();
    context
        .publish_capture_authorized(
            MEMBER.into(),
            input.identity.capture_id.clone(),
            seal,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let completed = until(&context, &input.identity.capture_id, &id, |snapshot| {
        snapshot.state.is_terminal()
    })
    .await;
    assert_eq!(
        completed.state,
        CaptureLiveState::Completed,
        "{completed:?}"
    );
    service.shutdown().await.unwrap();
    let (completed, drained, safe) = context
        .capture_live_processing_outcome(MEMBER.into(), id)
        .await
        .unwrap();
    assert_eq!(completed.state, CaptureLiveState::Completed);
    assert!(drained && safe);
    assert_eq!(fake.calls.load(Ordering::Acquire), 1);
    assert_eq!(writer.read_chunk(0).await.unwrap().descriptor.frames, 16000);
    assert!(
        context
            .store
            .vault_base()
            .join("sessions/session/transcript.json")
            .exists()
    );
}

#[tokio::test]
async fn busy_source_and_commit_admission_reuse_retained_live_result_with_fresh_guards() {
    for stage in ["source", "commit", "changed_transcript"] {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let fake = Fake::new();
        let auth = Auth::new();
        let context = context(&root, fake.clone(), auth.clone()).await;
        let (input, request, id) = begin(&context).await;
        context
            .start_capture_live_authorized(
                MEMBER.into(),
                input.identity.capture_id.clone(),
                id.clone(),
                request,
                || async { Ok(()) },
            )
            .await
            .unwrap();
        append(&context, &input, 1).await;
        until(&context, &input.identity.capture_id, &id, |snapshot| {
            snapshot.next_caption_sequence == 2
        })
        .await;
        let writer = context
            .capture_writer(MEMBER.into(), input.identity.capture_id.clone())
            .await
            .unwrap();
        let seal = writer.seal().await.unwrap();
        fake.joined.notified().await;
        context
            .publish_capture_authorized(
                MEMBER.into(),
                input.identity.capture_id.clone(),
                seal,
                || async { Ok(()) },
            )
            .await
            .unwrap();
        let audio = if stage == "source" {
            let audio = context.store.lock_session_audio("session").await.unwrap();
            fake.released.add_permits(1);
            audio
        } else {
            let (entered, reached) = tokio::sync::oneshot::channel();
            let release = Arc::new(Semaphore::new(0));
            *auth.before_authorize.lock().unwrap() = Some((entered, release.clone()));
            fake.released.add_permits(1);
            tokio::time::timeout(Duration::from_secs(5), reached)
                .await
                .unwrap()
                .unwrap();
            let audio = context.store.lock_session_audio("session").await.unwrap();
            release.add_permits(1);
            audio
        };
        let service = context.capture_live.get().unwrap();
        tokio::time::timeout(
            Duration::from_secs(5),
            service.journal.wait_for_promotion_busy(),
        )
        .await
        .unwrap();
        let waiting = service
            .journal
            .worker()
            .get(MEMBER.into(), id.clone())
            .await
            .unwrap();
        assert_eq!(
            waiting.snapshot.state,
            CaptureLiveState::AwaitingRecording,
            "{stage}"
        );
        assert!(!waiting.commit_pending, "{stage}");
        assert!(waiting.snapshot.result_available);
        assert_eq!(waiting.snapshot.failure, None);
        assert!(!service.failed.load(Ordering::Acquire));
        let retained = root.join("live").join(&id).join("result.json");
        let result = std::fs::read(&retained).unwrap();
        let canonical = context
            .store
            .vault_base()
            .join("sessions/session/transcript.json");
        assert!(!canonical.exists());
        let edited = if stage == "changed_transcript" {
            let mut edited: serde_json::Value = serde_json::from_slice(&result).unwrap();
            edited["transcripts"][0]["words"][0]["speaker"] =
                serde_json::json!("user-edited-speaker");
            let bytes = serde_json::to_vec_pretty(&edited).unwrap();
            std::fs::write(&canonical, &bytes).unwrap();
            Some(bytes)
        } else {
            None
        };
        drop(audio);
        let finished = until(&context, &input.identity.capture_id, &id, |snapshot| {
            snapshot.state.is_terminal()
        })
        .await;
        if let Some(edited) = edited {
            assert_eq!(finished.state, CaptureLiveState::Conflict);
            assert_eq!(std::fs::read(&canonical).unwrap(), edited);
        } else {
            assert_eq!(
                finished.state,
                CaptureLiveState::Completed,
                "{stage}: {finished:?}"
            );
            assert!(finished.publication.is_some());
        }
        service.shutdown().await.unwrap();
        assert!(!service.failed.load(Ordering::Acquire));
        assert_eq!(fake.calls.load(Ordering::Acquire), 1);
        assert_eq!(std::fs::read(retained).unwrap(), result);
        assert_eq!(writer.read_chunk(0).await.unwrap().descriptor.frames, 16000);
    }
}
