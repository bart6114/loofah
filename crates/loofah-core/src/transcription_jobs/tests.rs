use super::*;
use hypr_vault_read::{TranscriptSpeakerHint, TranscriptWord};
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::{
        Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const MEMBER: &str = "6114bd31-62da-4d90-9a0d-41b9a8872201";
const OTHER: &str = "6114bd31-62da-4d90-9a0d-41b9a8872202";
const REQUEST: &str = "6114bd31-62da-4d90-9a0d-41b9a8872203";
const AUDIO: &[u8] = b"Synthetic canonical audio fixture";

#[derive(Clone)]
struct Fake(Arc<Model>);
struct Model {
    calls: AtomicUsize,
    slots: Arc<Semaphore>,
    started: Semaphore,
    gate: (Mutex<bool>, Condvar),
    empty: AtomicBool,
    fail: AtomicBool,
    return_early: AtomicBool,
    marker: Mutex<Option<PathBuf>>,
    input_bytes: Mutex<Vec<u8>>,
}
impl Fake {
    fn new(blocked: bool) -> Self {
        Self(Arc::new(Model {
            calls: AtomicUsize::new(0),
            slots: Arc::new(Semaphore::new(1)),
            started: Semaphore::new(0),
            gate: (Mutex::new(!blocked), Condvar::new()),
            empty: AtomicBool::new(false),
            fail: AtomicBool::new(false),
            return_early: AtomicBool::new(false),
            marker: Mutex::new(None),
            input_bytes: Mutex::new(Vec::new()),
        }))
    }
    fn release(&self) {
        *self.0.gate.0.lock().unwrap() = true;
        self.0.gate.1.notify_all();
    }
    async fn started(&self) {
        tokio::time::timeout(Duration::from_secs(10), self.0.started.acquire())
            .await
            .unwrap()
            .unwrap()
            .forget();
    }
}
impl Drop for Model {
    fn drop(&mut self) {
        *self.gate.0.lock().unwrap() = true;
        self.gate.1.notify_all();
    }
}
struct PreparedFake {
    model: Fake,
    permit: OwnedSemaphorePermit,
}
impl TranscriptionEngine for Fake {
    fn prepare<'a>(
        &'a self,
        _: &'a TranscriptionJobRequest,
        cancel: CancellationToken,
    ) -> TranscriptionFuture<'a, Result<Box<dyn PreparedTranscription>, TranscriptionFailure>> {
        Box::pin(async move {
            let permit = tokio::select! {biased;_ = cancel.cancelled()=>return Err(TranscriptionFailure::Interrupted),permit=self.0.slots.clone().acquire_owned()=>permit.map_err(|_|TranscriptionFailure::EngineUnavailable)?};
            Ok(Box::new(PreparedFake {
                model: self.clone(),
                permit,
            }) as Box<dyn PreparedTranscription>)
        })
    }
}
impl PreparedTranscription for PreparedFake {
    fn transcribe(
        self: Box<Self>,
        input: TranscriptionInput,
        _: CancellationToken,
    ) -> TranscriptionFuture<'static, Result<TranscriptWithData, TranscriptionFailure>> {
        let PreparedFake { model, permit } = *self;
        let early = model.0.return_early.load(Ordering::Acquire);
        let task = tokio::task::spawn_blocking(move || {
            let (path, owner) = input.source.into_parts();
            model.0.calls.fetch_add(1, Ordering::AcqRel);
            if let Some(path) = model.0.marker.lock().unwrap().as_ref() {
                use std::io::Write;
                File::options()
                    .create(true)
                    .append(true)
                    .open(path)
                    .unwrap()
                    .write_all(b"call\n")
                    .unwrap();
            }
            model.0.started.add_permits(1);
            let mut released = model.0.gate.0.lock().unwrap();
            while !*released {
                released = model.0.gate.1.wait(released).unwrap();
            }
            drop(released);
            *model.0.input_bytes.lock().unwrap() = std::fs::read(path).unwrap();
            let result = if model.0.fail.load(Ordering::Acquire) {
                Err(TranscriptionFailure::EngineFailed)
            } else {
                let mut transcript = input.metadata.empty_transcript();
                if !model.0.empty.load(Ordering::Acquire) {
                    let word = uuid::Uuid::new_v4().to_string();
                    transcript.words.push(TranscriptWord {id:Some(word.clone()),text:"Generated words.".into(),start_ms:0.0,end_ms:120.0,channel:2.0,speaker:None,metadata:Some(serde_json::from_value(serde_json::json!({"timing_source":"provider_word","capture_source":"unknown","future":{"preserve":true}})).unwrap())});
                    transcript.speaker_hints.push(TranscriptSpeakerHint {
                        id: Some(uuid::Uuid::new_v4().to_string()),
                        word_id: word,
                        hint_type: "provider_speaker_index".into(),
                        value: serde_json::json!(
                            "{\"provider\":\"whispercpp\",\"channel\":2,\"speaker_index\":0}"
                        ),
                    });
                }
                Ok(transcript)
            };
            // The test receipt proves actual simulated resource release, even if
            // the adapter future returned early and detached this blocking work.
            drop(permit);
            drop(owner);
            result
        });
        if early {
            drop(task);
            return Box::pin(async { Err(TranscriptionFailure::EngineFailed) });
        }
        Box::pin(async move { task.await.map_err(|_| TranscriptionFailure::EngineFailed)? })
    }
}
struct Unblock(Fake);
impl Drop for Unblock {
    fn drop(&mut self) {
        self.0.release();
    }
}

struct Auth {
    allowed: AtomicBool,
    revoked: CancellationToken,
    interrupted: CancellationToken,
    final_gate: Mutex<Option<Arc<Semaphore>>>,
    final_entered: Semaphore,
    probe: Mutex<Option<std::sync::Weak<VaultContext>>>,
}
impl Auth {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            allowed: AtomicBool::new(true),
            revoked: CancellationToken::new(),
            interrupted: CancellationToken::new(),
            final_gate: Mutex::new(None),
            final_entered: Semaphore::new(0),
            probe: Mutex::new(None),
        })
    }
}
impl TranscriptionAuthorizer for Auth {
    fn observe<'a>(
        &'a self,
        _: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>> {
        Box::pin(async move {
            if self.interrupted.is_cancelled() {
                return Err(TranscriptionFailure::Interrupted);
            }
            if !self.allowed.load(Ordering::Acquire) {
                return Err(TranscriptionFailure::AccessDenied);
            }
            Ok(TranscriptionAccess {
                interrupted: self.interrupted.clone(),
                cancelled: self.revoked.child_token(),
                owner: Box::new(()),
            })
        })
    }
    fn authorize<'a>(
        &'a self,
        _: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        Box::pin(async move {
            self.final_entered.add_permits(1);
            let gate = self.final_gate.lock().unwrap().clone();
            if let Some(gate) = gate {
                gate.acquire().await.unwrap().forget();
            }
            if self.interrupted.is_cancelled() {
                return Err(TranscriptionFailure::Interrupted);
            }
            if !self.allowed.load(Ordering::Acquire) {
                return Err(TranscriptionFailure::AccessDenied);
            }
            let probe = self
                .probe
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|p| p.upgrade());
            if let Some(context) = probe {
                assert!(matches!(context.store.prepare_transcript_source("note").await.unwrap(),Some(hypr_vault_write::transcript_publication::TranscriptSourcePreparation::Ready(_))),"authorization must precede Store operation locks");
            }
            Ok(Box::new(()) as Box<dyn Send>)
        })
    }
}
struct Fixture {
    root: tempfile::TempDir,
    runtime: Option<VaultRuntime>,
    engine: Fake,
    auth: Arc<Auth>,
}
async fn seed(root: &Path) {
    let store = hypr_vault_write::SessionStore::new(root.join("vault"));
    let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Meeting","created_at":"2026-11-08T10:00:00Z","tags":[],"future":{"keep":true}})).unwrap();
    store
        .create_note_session(&meta, "# Meeting\n\nBefore transcription.")
        .await
        .unwrap();
    std::fs::write(root.join("vault/sessions/note/audio.wav"), AUDIO).unwrap();
    std::fs::write(
        root.join("vault/sessions/note/unknown.txt"),
        "User attachment",
    )
    .unwrap();
}
async fn open(
    root: &Path,
    engine: Fake,
    auth: Arc<Auth>,
    limits: TranscriptionLimits,
) -> VaultRuntime {
    let mut runtime = VaultRuntime::open_with_changes(
        "vault".into(),
        &root.join("vault"),
        &root.join("cache"),
        &root.join("operations"),
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap();
    runtime
        .enable_transcription(Arc::new(engine), auth, limits)
        .await
        .unwrap();
    runtime
}
impl Fixture {
    async fn new(blocked: bool) -> Self {
        Self::limited(blocked, TranscriptionLimits::default()).await
    }
    async fn limited(blocked: bool, limits: TranscriptionLimits) -> Self {
        let root = tempfile::tempdir().unwrap();
        seed(root.path()).await;
        let engine = Fake::new(blocked);
        let auth = Auth::new();
        let runtime = open(root.path(), engine.clone(), auth.clone(), limits).await;
        Self {
            root,
            runtime: Some(runtime),
            engine,
            auth,
        }
    }
    fn context(&self) -> Arc<VaultContext> {
        self.runtime.as_ref().unwrap().context.clone()
    }
    async fn submit(&self) -> TranscriptionJobSnapshot {
        self.context()
            .submit_transcription_job(MEMBER.into(), REQUEST.into(), request())
            .await
            .unwrap()
    }
    fn transcript(&self) -> PathBuf {
        self.root.path().join("vault/sessions/note/transcript.json")
    }
    async fn finish(mut self) {
        self.engine.release();
        self.runtime.take().unwrap().shutdown().await.unwrap();
    }
}
fn request() -> TranscriptionJobRequest {
    TranscriptionJobRequest {
        session_id: "note".into(),
        expected_recording_sha256: ledger::hash(AUDIO),
        expected_transcript: AbsentTranscriptExpectation::Absent,
        model: "cpu-base".into(),
        languages: vec!["en".into()],
        keywords: vec!["Loofah".into()],
    }
}
async fn terminal(context: &Arc<VaultContext>, id: &str) -> TranscriptionJobSnapshot {
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let snapshot = context
                .transcription_job(MEMBER.into(), id.into())
                .await
                .unwrap();
            if snapshot.state.is_terminal() {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}
fn unlocked(root: &Path) -> bool {
    let file = File::options()
        .read(true)
        .write(true)
        .open(root.join("vault/.loofah-vault.lock"))
        .unwrap();
    file.try_lock().is_ok()
}

#[tokio::test]
async fn exact_replay_preserves_metadata_ids_hints_and_unknown_files_without_regeneration() {
    let fixture = Fixture::new(false).await;
    let context = fixture.context();
    let snapshot = fixture.submit().await;
    let done = terminal(&context, &snapshot.id).await;
    assert_eq!(done.state, TranscriptionJobState::Succeeded);
    assert!(done.result_available);
    let result = context
        .transcription_job_result(MEMBER.into(), done.id.clone())
        .await
        .unwrap()
        .unwrap()
        .transcript;
    assert_eq!(result.user_id, MEMBER);
    assert_eq!(result.created_at, snapshot.created_at);
    assert_eq!(result.ended_at, None);
    assert_eq!(
        result.memo_md,
        serde_json::to_string(
            &hypr_tiptap::md_to_tiptap_json("# Meeting\n\nBefore transcription.").unwrap()
        )
        .unwrap()
    );
    assert_eq!(result.words[0].channel, 2.0);
    assert_eq!(
        result.words[0].metadata.as_ref().unwrap()["future"]["preserve"],
        true
    );
    assert!(result.speaker_hints[0].value.is_string());
    let canonical: hypr_vault_read::TranscriptJson =
        serde_json::from_slice(&std::fs::read(fixture.transcript()).unwrap()).unwrap();
    assert_eq!(canonical.transcripts, vec![result]);
    let replay = fixture.submit().await;
    assert_eq!(replay, done);
    assert_eq!(fixture.engine.0.calls.load(Ordering::Acquire), 1);
    assert_eq!(
        std::fs::read_to_string(fixture.root.path().join("vault/sessions/note/unknown.txt"))
            .unwrap(),
        "User attachment"
    );
    let mut changed = request();
    changed.keywords.push("changed".into());
    assert_eq!(
        context
            .submit_transcription_job(MEMBER.into(), REQUEST.into(), changed)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        context
            .transcription_job(OTHER.into(), done.id.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    assert!(
        context
            .list_transcription_jobs(OTHER.into(), Default::default())
            .await
            .unwrap()
            .jobs
            .is_empty()
    );
    fixture.finish().await;
}

#[tokio::test]
async fn immutable_copy_and_source_cas_keep_later_recording_note_and_transcript_edits() {
    for transcript_edit in [false, true] {
        let fixture = Fixture::new(true).await;
        let _unblock = Unblock(fixture.engine.clone());
        let context = fixture.context();
        let job = fixture.submit().await;
        fixture.engine.started().await;
        let directory = fixture.root.path().join("vault/sessions/note");
        std::fs::write(directory.join("notes.md"), "Newer user note").unwrap();
        if transcript_edit {
            std::fs::write(fixture.transcript(), b"{\"future\":true}").unwrap();
        } else {
            std::fs::rename(
                directory.join("audio.wav"),
                directory.join("retired-recording.wav"),
            )
            .unwrap();
            std::fs::write(directory.join("audio.wav"), b"Newer recording bytes").unwrap();
        }
        fixture.engine.release();
        let done = terminal(&context, &job.id).await;
        assert_eq!(done.state, TranscriptionJobState::Conflict);
        assert!(done.result_available);
        assert_eq!(*fixture.engine.0.input_bytes.lock().unwrap(), AUDIO);
        assert_eq!(
            std::fs::read_to_string(directory.join("notes.md")).unwrap(),
            "Newer user note"
        );
        if transcript_edit {
            assert_eq!(
                std::fs::read(fixture.transcript()).unwrap(),
                b"{\"future\":true}"
            );
        } else {
            assert!(!fixture.transcript().exists());
            assert_eq!(
                std::fs::read(directory.join("audio.wav")).unwrap(),
                b"Newer recording bytes"
            );
        }
        let result = context
            .transcription_job_result(MEMBER.into(), job.id)
            .await
            .unwrap()
            .unwrap()
            .transcript;
        assert!(result.memo_md.contains("Before transcription"));
        fixture.finish().await;
    }
}

#[tokio::test]
async fn empty_speech_still_checks_source_and_never_creates_a_canonical_transcript() {
    for replace in [false, true] {
        let fixture = Fixture::new(true).await;
        let _unblock = Unblock(fixture.engine.clone());
        fixture.engine.0.empty.store(true, Ordering::Release);
        let context = fixture.context();
        let job = fixture.submit().await;
        fixture.engine.started().await;
        if replace {
            std::fs::rename(
                fixture.root.path().join("vault/sessions/note/audio.wav"),
                fixture
                    .root
                    .path()
                    .join("vault/sessions/note/retired-recording.wav"),
            )
            .unwrap();
            std::fs::write(
                fixture.root.path().join("vault/sessions/note/audio.wav"),
                b"Changed during silence",
            )
            .unwrap();
        }
        fixture.engine.release();
        let done = terminal(&context, &job.id).await;
        assert_eq!(
            done.state,
            if replace {
                TranscriptionJobState::Conflict
            } else {
                TranscriptionJobState::Empty
            }
        );
        assert!(!fixture.transcript().exists());
        assert!(done.result_available);
        fixture.finish().await;
    }
}

#[tokio::test]
async fn cancellation_retains_late_output_and_native_admission_until_real_source_release() {
    let fixture = Fixture::new(true).await;
    let _unblock = Unblock(fixture.engine.clone());
    let context = fixture.context();
    let job = fixture.submit().await;
    fixture.engine.started().await;
    assert_eq!(
        context
            .cancel_transcription_job(MEMBER.into(), job.id.clone())
            .await
            .unwrap()
            .state,
        TranscriptionJobState::Cancelling
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(30), terminal(&context, &job.id))
            .await
            .is_err()
    );
    assert_eq!(fixture.engine.0.slots.available_permits(), 0);
    assert!(!fixture.transcript().exists());
    let materialization_after = chrono::Utc::now().timestamp_millis() as f64;
    fixture.engine.release();
    let done = terminal(&context, &job.id).await;
    assert_eq!(done.state, TranscriptionJobState::Cancelled);
    assert!(done.result_available);
    assert_eq!(fixture.engine.0.slots.available_permits(), 1);
    let output = context
        .transcription_job_result(MEMBER.into(), job.id)
        .await
        .unwrap()
        .unwrap()
        .transcript;
    assert!(output.started_at >= materialization_after);
    assert!(!fixture.transcript().exists());
    fixture.finish().await;
}

#[tokio::test]
async fn revoked_membership_cancels_but_preserves_output_and_never_promotes() {
    let fixture = Fixture::new(true).await;
    let _unblock = Unblock(fixture.engine.clone());
    let context = fixture.context();
    let job = fixture.submit().await;
    fixture.engine.started().await;
    fixture.auth.allowed.store(false, Ordering::Release);
    fixture.auth.revoked.cancel();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), terminal(&context, &job.id))
            .await
            .is_err()
    );
    fixture.engine.release();
    let done = terminal(&context, &job.id).await;
    assert_eq!(done.state, TranscriptionJobState::Cancelled);
    assert_eq!(done.failure, Some(TranscriptionFailure::AccessDenied));
    assert!(done.result_available);
    assert!(!fixture.transcript().exists());
    fixture.finish().await;
}

#[tokio::test]
async fn host_interruption_is_distinct_from_member_revocation_and_keeps_late_result() {
    let fixture = Fixture::new(true).await;
    let _unblock = Unblock(fixture.engine.clone());
    let context = fixture.context();
    let job = fixture.submit().await;
    fixture.engine.started().await;
    fixture.auth.interrupted.cancel();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), terminal(&context, &job.id))
            .await
            .is_err()
    );
    fixture.engine.release();
    let done = terminal(&context, &job.id).await;
    assert_eq!(done.state, TranscriptionJobState::Interrupted);
    assert_eq!(done.failure, Some(TranscriptionFailure::Interrupted));
    assert!(done.result_available);
    assert!(!fixture.transcript().exists());
    fixture.finish().await;

    let fixture = Fixture::new(false).await;
    fixture.auth.interrupted.cancel();
    let context = fixture.context();
    let job = fixture.submit().await;
    let done = terminal(&context, &job.id).await;
    assert_eq!(done.state, TranscriptionJobState::Interrupted);
    assert_eq!(done.failure, Some(TranscriptionFailure::Interrupted));
    assert_eq!(fixture.engine.0.calls.load(Ordering::Acquire), 0);
    fixture.finish().await;

    let fixture = Fixture::new(false).await;
    let context = fixture.context();
    let gate = Arc::new(Semaphore::new(0));
    *fixture.auth.final_gate.lock().unwrap() = Some(gate.clone());
    let job = fixture.submit().await;
    tokio::time::timeout(
        Duration::from_secs(10),
        fixture.auth.final_entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    fixture.auth.interrupted.cancel();
    gate.add_permits(1);
    let done = terminal(&context, &job.id).await;
    assert_eq!(done.state, TranscriptionJobState::Interrupted);
    assert_eq!(done.failure, Some(TranscriptionFailure::Interrupted));
    assert!(done.result_available);
    assert!(!fixture.transcript().exists());
    fixture.finish().await;
}

#[tokio::test]
async fn fresh_commit_authorization_runs_before_store_locks_and_refusal_keeps_saved_result() {
    let fixture = Fixture::new(false).await;
    let context = fixture.context();
    *fixture.auth.probe.lock().unwrap() = Some(Arc::downgrade(&context));
    let job = fixture.submit().await;
    assert_eq!(
        terminal(&context, &job.id).await.state,
        TranscriptionJobState::Succeeded
    );
    fixture.finish().await;
    let fixture = Fixture::new(false).await;
    let context = fixture.context();
    let gate = Arc::new(Semaphore::new(0));
    *fixture.auth.final_gate.lock().unwrap() = Some(gate.clone());
    let job = fixture.submit().await;
    tokio::time::timeout(
        Duration::from_secs(10),
        fixture.auth.final_entered.acquire(),
    )
    .await
    .unwrap()
    .unwrap()
    .forget();
    fixture.auth.allowed.store(false, Ordering::Release);
    gate.add_permits(1);
    let done = terminal(&context, &job.id).await;
    assert_eq!(done.state, TranscriptionJobState::Failed);
    assert_eq!(done.failure, Some(TranscriptionFailure::AccessDenied));
    assert!(done.result_available);
    assert!(!fixture.transcript().exists());
    fixture.finish().await;
}

#[tokio::test]
async fn an_early_adapter_error_and_runtime_cancellation_cannot_release_actual_native_ownership() {
    let mut fixture = Fixture::new(true).await;
    let _unblock = Unblock(fixture.engine.clone());
    fixture.engine.0.return_early.store(true, Ordering::Release);
    let context = fixture.context();
    let startup = context.startup.clone();
    let job = fixture.submit().await;
    fixture.engine.started().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(30), terminal(&context, &job.id))
            .await
            .is_err()
    );
    drop(context);
    assert!(
        tokio::time::timeout(
            Duration::from_millis(30),
            fixture.runtime.take().unwrap().shutdown()
        )
        .await
        .is_err()
    );
    assert_ne!(
        startup.snapshot().phase,
        crate::startup::StartupPhase::Stopped
    );
    assert!(!unlocked(fixture.root.path()));
    assert_eq!(fixture.engine.0.slots.available_permits(), 0);
    fixture.engine.release();
    tokio::time::timeout(Duration::from_secs(10), startup.wait_until_stopped())
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !unlocked(fixture.root.path()) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(fixture.engine.0.slots.available_permits(), 1);
}

#[tokio::test]
async fn existing_even_empty_corrupt_or_unknown_transcripts_and_private_quota_prevent_inference() {
    for bytes in [b"".as_slice(), b"not json", b"{\"transcripts\":[]}"] {
        let fixture = Fixture::new(false).await;
        std::fs::write(fixture.transcript(), bytes).unwrap();
        let job = fixture.submit().await;
        assert_eq!(
            terminal(&fixture.context(), &job.id).await.state,
            TranscriptionJobState::Conflict
        );
        assert_eq!(fixture.engine.0.calls.load(Ordering::Acquire), 0);
        assert_eq!(std::fs::read(fixture.transcript()).unwrap(), bytes);
        fixture.finish().await;
    }
    let fixture = Fixture::limited(
        false,
        TranscriptionLimits {
            private_audio_bytes: AUDIO.len() as u64 - 1,
            ..Default::default()
        },
    )
    .await;
    let job = fixture.submit().await;
    let done = terminal(&fixture.context(), &job.id).await;
    assert_eq!(done.state, TranscriptionJobState::Failed);
    assert_eq!(done.failure, Some(TranscriptionFailure::ResourceLimit));
    assert_eq!(fixture.engine.0.calls.load(Ordering::Acquire), 0);
    assert_eq!(fixture.engine.0.slots.available_permits(), 1);
    assert!(!fixture.transcript().exists());
    fixture.finish().await;
}

#[tokio::test]
async fn shared_engine_admission_precedes_copy_across_vaults_and_cancels_queued_work() {
    let first = Fixture::new(true).await;
    let _unblock = Unblock(first.engine.clone());
    let first_job = first.submit().await;
    first.engine.started().await;
    let root = tempfile::tempdir().unwrap();
    seed(root.path()).await;
    let auth = Auth::new();
    let second = open(root.path(), first.engine.clone(), auth, Default::default()).await;
    let context = second.context.clone();
    let job = context
        .submit_transcription_job(MEMBER.into(), REQUEST.into(), request())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if context
                .transcription_job(MEMBER.into(), job.id.clone())
                .await
                .unwrap()
                .state
                == TranscriptionJobState::Running
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let private = context
        .operations
        .as_ref()
        .unwrap()
        .root
        .join("transcription-jobs");
    assert!(!private.join(format!("{}.source.wav", job.id)).exists());
    assert_eq!(first.engine.0.calls.load(Ordering::Acquire), 1);
    context
        .cancel_transcription_job(MEMBER.into(), job.id.clone())
        .await
        .unwrap();
    assert_eq!(
        terminal(&context, &job.id).await.state,
        TranscriptionJobState::Cancelled
    );
    first.engine.release();
    assert_eq!(
        terminal(&first.context(), &first_job.id).await.state,
        TranscriptionJobState::Succeeded
    );
    second.shutdown().await.unwrap();
    first.finish().await;
}

#[tokio::test]
async fn retained_result_resume_is_checked_without_reresolving_a_model_or_regenerating() {
    let fixture = Fixture::new(true).await;
    let _unblock = Unblock(fixture.engine.clone());
    let context = fixture.context();
    let job = fixture.submit().await;
    fixture.engine.started().await;
    context
        .cancel_transcription_job(MEMBER.into(), job.id.clone())
        .await
        .unwrap();
    fixture.engine.release();
    let done = terminal(&context, &job.id).await;
    assert_eq!(done.state, TranscriptionJobState::Cancelled);
    let result = context
        .transcription_job_result(MEMBER.into(), job.id.clone())
        .await
        .unwrap()
        .unwrap()
        .transcript;
    context
        .resume_transcription_result(MEMBER.into(), job.id.clone())
        .await
        .unwrap();
    let done = terminal(&context, &job.id).await;
    assert_eq!(done.state, TranscriptionJobState::Succeeded);
    assert_eq!(fixture.engine.0.calls.load(Ordering::Acquire), 1);
    let canonical: hypr_vault_read::TranscriptJson =
        serde_json::from_slice(&std::fs::read(fixture.transcript()).unwrap()).unwrap();
    assert_eq!(canonical.transcripts, vec![result]);
    fixture.finish().await;
}

#[tokio::test]
async fn job_events_are_private_and_transcript_publication_invalidates_canonical_readers() {
    let fixture = Fixture::new(false).await;
    let context = fixture.context();
    let mut owner = context.events.subscribe_for(MEMBER, None);
    let mut other = context.events.subscribe_for(OTHER, None);
    let job = fixture.submit().await;
    assert_eq!(
        terminal(&context, &job.id).await.state,
        TranscriptionJobState::Succeeded
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        let (mut private, mut transcript) = (false, false);
        while !private || !transcript {
            match owner.recv().await.unwrap().change {
                Change::JobChanged { job_id } if job_id == job.id => private = true,
                Change::IndexChanged {
                    entity: IndexEntity::Transcripts,
                    ids,
                    ..
                } if ids.contains(&"note".into()) => transcript = true,
                _ => {}
            }
        }
    })
    .await
    .unwrap();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_millis(20), other.recv()).await
    {
        assert!(!matches!(event.change, Change::JobChanged { .. }));
    }
    fixture.finish().await;
}

#[tokio::test]
async fn strict_journal_reopen_rejects_future_duplicate_or_changed_private_records_without_rewriting()
 {
    for mutation in [
        "version",
        "duplicate",
        "action",
        "requeue",
        "uuid",
        "result",
        "missing",
        "future_result",
    ] {
        let mut fixture = Fixture::new(false).await;
        let context = fixture.context();
        let job = fixture.submit().await;
        terminal(&context, &job.id).await;
        let private = context
            .operations
            .as_ref()
            .unwrap()
            .root
            .join("transcription-jobs");
        drop(context);
        fixture.runtime.take().unwrap().shutdown().await.unwrap();
        let record = private.join(format!("{}.json", job.id));
        let old = std::fs::read_to_string(&record).unwrap();
        let changed = if mutation == "version" {
            old.replacen("\"schema_version\":1", "\"schema_version\":2", 1)
        } else if mutation == "duplicate" {
            old.replacen(
                "\"schema_version\":1",
                "\"schema_version\":1,\"schema_version\":1",
                1,
            )
        } else if mutation == "action" {
            old.replacen(
                "\"action\":\"generate\"",
                "\"action\":{\"generate\":{\"future\":true}}",
                1,
            )
        } else if mutation == "requeue" {
            let mut data: serde_json::Value = serde_json::from_str(&old).unwrap();
            data["snapshot"]["state"] = serde_json::json!("queued");
            data["snapshot"]["phase"] = serde_json::Value::Null;
            data["snapshot"]["publication"] = serde_json::Value::Null;
            serde_json::to_string(&data).unwrap()
        } else if mutation == "uuid" {
            old.replace(REQUEST, &REQUEST.to_uppercase())
        } else if mutation == "future_result" {
            let path = private.join(format!("{}.result.json", job.id));
            let output = std::fs::read_to_string(&path).unwrap().replacen(
                "\"session_id\":\"note\"",
                "\"session_id\":\"note\",\"future\":{\"keep\":true}",
                1,
            );
            std::fs::write(path, &output).unwrap();
            let mut data: serde_json::Value = serde_json::from_str(&old).unwrap();
            data["result"]["sha256"] = serde_json::json!(ledger::hash(output.as_bytes()));
            data["result"]["bytes"] = serde_json::json!(output.len());
            serde_json::to_string(&data).unwrap()
        } else {
            old.clone()
        };
        std::fs::write(&record, &changed).unwrap();
        if mutation == "result" {
            std::fs::write(
                private.join(format!("{}.result.json", job.id)),
                "Changed private output",
            )
            .unwrap();
        }
        if mutation == "missing" {
            std::fs::remove_file(private.join(format!("{}.result.json", job.id))).unwrap();
        }
        let result_path = record
            .parent()
            .unwrap()
            .join(format!("{}.result.json", job.id));
        let result_before = std::fs::read(&result_path).ok();
        let canonical_before = std::fs::read(fixture.transcript()).unwrap();
        assert!(
            ledger::Journal::open(
                private,
                crate::events::EventHub::new("vault".into(), 8),
                Default::default(),
                None
            )
            .await
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(record).unwrap(), changed);
        assert_eq!(std::fs::read(&result_path).ok(), result_before);
        assert_eq!(
            std::fs::read(fixture.transcript()).unwrap(),
            canonical_before
        );
        assert_eq!(fixture.engine.0.calls.load(Ordering::Acquire), 1);
    }
}

#[tokio::test]
async fn crash_child() {
    let Ok(root) = std::env::var("LOOFAH_TRANSCRIPTION_CHILD_ROOT") else {
        return;
    };
    let point = std::env::var("LOOFAH_TRANSCRIPTION_CHILD_POINT").unwrap();
    let engine = Fake::new(false);
    *engine.0.marker.lock().unwrap() = Some(Path::new(&root).join("calls"));
    let runtime = open(Path::new(&root), engine, Auth::new(), Default::default()).await;
    runtime
        .context
        .transcription
        .get()
        .unwrap()
        .journal
        .set_fault(Arc::new(move |current| {
            if current == point {
                std::process::exit(73);
            }
        }));
    let job = runtime
        .context
        .submit_transcription_job(MEMBER.into(), REQUEST.into(), request())
        .await
        .unwrap();
    let _ = terminal(&runtime.context, &job.id).await;
    panic!("fault point not reached");
}

#[tokio::test]
async fn killed_children_reconcile_retained_results_without_paid_replay_or_resurrection() {
    for point in [
        "claimed",
        "result_intent",
        "result_file",
        "publication_commit",
        "publication_commit_missing_receipt",
    ] {
        let root = tempfile::tempdir().unwrap();
        seed(root.path()).await;
        let log = File::create(root.path().join("child.log")).unwrap();
        let error = log.try_clone().unwrap();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "transcription_jobs::tests::crash_child",
                "--nocapture",
            ])
            .env("LOOFAH_TRANSCRIPTION_CHILD_ROOT", root.path())
            .env(
                "LOOFAH_TRANSCRIPTION_CHILD_POINT",
                if point == "publication_commit_missing_receipt" {
                    "publication_commit"
                } else {
                    point
                },
            )
            .stdout(log)
            .stderr(error)
            .spawn()
            .unwrap();
        let status = tokio::task::spawn_blocking(move || child.wait().unwrap())
            .await
            .unwrap();
        assert_eq!(
            status.code(),
            Some(73),
            "{}",
            std::fs::read_to_string(root.path().join("child.log")).unwrap()
        );
        let engine = Fake::new(false);
        *engine.0.marker.lock().unwrap() = Some(root.path().join("calls"));
        let runtime = open(root.path(), engine.clone(), Auth::new(), Default::default()).await;
        let context = runtime.context.clone();
        let jobs = context
            .list_transcription_jobs(MEMBER.into(), Default::default())
            .await
            .unwrap();
        assert_eq!(jobs.jobs.len(), 1);
        let job = &jobs.jobs[0];
        assert_eq!(
            job.state,
            if point.starts_with("publication_commit") {
                TranscriptionJobState::Uncertain
            } else {
                TranscriptionJobState::Interrupted
            }
        );
        let replay = context
            .submit_transcription_job(MEMBER.into(), REQUEST.into(), request())
            .await
            .unwrap();
        assert_eq!(replay.state, job.state);
        assert_eq!(engine.0.calls.load(Ordering::Acquire), 0);
        let retained = point == "result_file" || point.starts_with("publication_commit");
        assert_eq!(job.result_available, retained);
        if retained {
            if point.starts_with("publication_commit") {
                std::fs::remove_file(root.path().join("vault/sessions/note/transcript.json"))
                    .unwrap();
            }
            if point == "publication_commit_missing_receipt" {
                let operations = context
                    .operations
                    .as_ref()
                    .unwrap()
                    .root
                    .join("transcript-publications");
                let mut removed = 0;
                for entry in std::fs::read_dir(operations).unwrap() {
                    let entry = entry.unwrap();
                    let name = entry.file_name();
                    let name = name.to_str().unwrap();
                    if name
                        .strip_prefix("transcript-publication-")
                        .and_then(|name| name.strip_suffix(".json"))
                        .is_some_and(is_transcription_sha256)
                    {
                        std::fs::remove_file(entry.path()).unwrap();
                        removed += 1;
                    }
                }
                assert_eq!(removed, 1, "fixture must remove the actual Store receipt");
            }
            context
                .resume_transcription_result(MEMBER.into(), job.id.clone())
                .await
                .unwrap();
            assert_eq!(
                terminal(&context, &job.id).await.state,
                if point == "publication_commit_missing_receipt" {
                    TranscriptionJobState::Uncertain
                } else {
                    TranscriptionJobState::Succeeded
                }
            );
            if point.starts_with("publication_commit") {
                assert!(
                    !root
                        .path()
                        .join("vault/sessions/note/transcript.json")
                        .exists()
                );
            }
        } else {
            assert!(
                context
                    .resume_transcription_result(MEMBER.into(), job.id.clone())
                    .await
                    .is_err()
            );
        }
        assert_eq!(engine.0.calls.load(Ordering::Acquire), 0);
        let calls = std::fs::read_to_string(root.path().join("calls")).unwrap_or_default();
        assert_eq!(
            calls.lines().count(),
            if point == "claimed" { 0 } else { 1 }
        );
        runtime.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn readonly_queries_and_empty_claims_do_not_wake_an_idle_worker() {
    let fixture = Fixture::new(false).await;
    let root = std::fs::canonicalize(fixture.root.path())
        .unwrap()
        .join("idle-journal");
    let journal = ledger::Journal::open(
        root,
        crate::events::EventHub::new("vault".into(), 8),
        Default::default(),
        Some(fixture.context()),
    )
    .await
    .unwrap();
    let snapshot = journal
        .submit(MEMBER.into(), REQUEST.into(), request())
        .await
        .unwrap();
    journal.notify().notified().await;
    journal.owned(MEMBER.into(), snapshot.id).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), journal.notify().notified())
            .await
            .is_err()
    );
    assert!(
        journal
            .claim(CancellationToken::new())
            .await
            .unwrap()
            .is_some()
    );
    journal.notify().notified().await;
    assert!(
        journal
            .claim(CancellationToken::new())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(20), journal.notify().notified())
            .await
            .is_err()
    );
    drop(journal);
    fixture.finish().await;
}

#[tokio::test]
async fn managed_context_starts_one_owned_worker_without_a_second_runtime_or_store() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path()).await;
    let runtime = VaultRuntime::open_with_changes(
        "vault".into(),
        &root.path().join("vault"),
        &root.path().join("cache"),
        &root.path().join("operations"),
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap();
    let context = runtime.context.clone();
    let original = context.store.clone();
    let model = Fake::new(true);
    let auth = Auth::new();
    let worker = context
        .start_transcription(
            Arc::new(model.clone()),
            auth.clone(),
            TranscriptionLimits::default(),
        )
        .await
        .unwrap();
    let duplicate = context
        .start_transcription(
            Arc::new(model.clone()),
            auth,
            TranscriptionLimits::default(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(duplicate.code, ErrorCode::Conflict);
    let job = context
        .submit_transcription_job(MEMBER.into(), REQUEST.into(), request())
        .await
        .unwrap();
    model.started().await;
    assert!(Arc::ptr_eq(&context.store, &original));
    assert!(!unlocked(root.path()));
    model.release();
    assert_eq!(
        terminal(&context, &job.id).await.state,
        TranscriptionJobState::Succeeded
    );
    worker.shutdown().await.unwrap();
    assert!(!unlocked(root.path()));
    drop(original);
    drop(context);
    runtime.shutdown().await.unwrap();
    assert!(unlocked(root.path()));
}
