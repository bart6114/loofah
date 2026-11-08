mod append;
use super::*;
use crate::{jobs::*, transcription_jobs::*};
use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::Duration,
};
use tokio::sync::Notify;

const MEMBER: &str = "6114bd31-62da-4d90-9a0d-41b9a8872201";
struct Auth(AtomicBool);
impl crate::capture_live::CaptureLiveAuthorizer for Auth {
    fn observe<'a>(
        &'a self,
        _: &'a crate::capture_live::CaptureLiveWork,
    ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>> {
        Box::pin(async {
            Ok(TranscriptionAccess {
                cancelled: CancellationToken::new(),
                interrupted: CancellationToken::new(),
                owner: Box::new(()),
            })
        })
    }
    fn authorize<'a>(
        &'a self,
        _: &'a crate::capture_live::CaptureLiveWork,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        CaptureProcessingAuthorizer::authorize(self, MEMBER)
    }
}
impl CaptureProcessingAuthorizer for Auth {
    fn authorize<'a>(
        &'a self,
        _: &'a str,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        Box::pin(async move {
            if self.0.load(Ordering::Acquire) {
                Err(TranscriptionFailure::AccessDenied)
            } else {
                Ok(Box::new(()) as Box<dyn Send>)
            }
        })
    }
}
impl TranscriptionAuthorizer for Auth {
    fn observe<'a>(
        &'a self,
        _: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<TranscriptionAccess, TranscriptionFailure>> {
        Box::pin(async {
            Ok(TranscriptionAccess {
                cancelled: CancellationToken::new(),
                interrupted: CancellationToken::new(),
                owner: Box::new(()),
            })
        })
    }
    fn authorize<'a>(
        &'a self,
        _: &'a TranscriptionWork,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>> {
        CaptureProcessingAuthorizer::authorize(self, MEMBER)
    }
}
impl JobAuthorizer for Auth {
    fn authorize<'a>(
        &'a self,
        _: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>> {
        Box::pin(async { Ok(Box::new(()) as Box<dyn Send>) })
    }
}
#[derive(Default)]
struct Batch {
    calls: AtomicUsize,
    observed_frames: AtomicUsize,
    empty: AtomicBool,
    fail: AtomicBool,
    prepare_busy: AtomicBool,
}
struct Prepared(Arc<Batch>);
impl TranscriptionEngine for Arc<Batch> {
    fn prepare<'a>(
        &'a self,
        _: &'a TranscriptionJobRequest,
        _: CancellationToken,
    ) -> TranscriptionFuture<'a, Result<Box<dyn PreparedTranscription>, TranscriptionFailure>> {
        let owner = self.clone();
        Box::pin(async move {
            if owner.prepare_busy.load(Ordering::Acquire) {
                return Err(TranscriptionFailure::ResourceLimit);
            }
            Ok(Box::new(Prepared(owner)) as Box<dyn PreparedTranscription>)
        })
    }
}
impl PreparedTranscription for Prepared {
    fn transcribe(
        self: Box<Self>,
        input: TranscriptionInput,
        _: CancellationToken,
    ) -> TranscriptionFuture<
        'static,
        Result<hypr_vault_read::TranscriptWithData, TranscriptionFailure>,
    > {
        Box::pin(async move {
            self.0.calls.fetch_add(1, Ordering::AcqRel);
            self.0.observed_frames.store(
                hound::WavReader::open(input.source.path())
                    .map(|reader| reader.duration() as usize)
                    .unwrap_or(0),
                Ordering::Release,
            );
            if self.0.fail.load(Ordering::Acquire) {
                return Err(TranscriptionFailure::EngineFailed);
            }
            let mut result = input.metadata.empty_transcript();
            if !self.0.empty.load(Ordering::Acquire) {
                result.words = vec![hypr_vault_read::TranscriptWord {
                    id: Some(uuid::Uuid::new_v4().to_string()),
                    text: "hello".into(),
                    start_ms: 0.,
                    end_ms: 5.,
                    channel: 0.,
                    speaker: None,
                    metadata: None,
                }];
            }
            drop(input);
            Ok(result)
        })
    }
}
#[derive(Default)]
struct Summary {
    calls: AtomicUsize,
    entered: Notify,
    release: Notify,
    blocked: AtomicBool,
}
impl JobGenerator for Summary {
    fn generate(
        &self,
        _: JobWork,
        _: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::AcqRel);
            self.entered.notify_one();
            if self.blocked.load(Ordering::Acquire) {
                self.release.notified().await;
            }
            Ok(GeneratedDocument {
                markdown: "## Result\n\nA complete summary".into(),
                suggested_tags: None,
            })
        })
    }
}
fn policy(sha: &str) -> CaptureProcessingPolicy {
    CaptureProcessingPolicy {
        model: Some(CaptureProcessingModel {
            alias: "base".into(),
            sha256: sha.repeat(64),
        }),
        summary_enabled: true,
        availability: Arc::new(|| TranscriptionAvailability::Ready),
    }
}
async fn open_runtime(root: &std::path::Path) -> VaultRuntime {
    let runtime = VaultRuntime::open_with_changes(
        "vault".into(),
        &root.join("vault"),
        &root.join("cache"),
        &root.join("operations"),
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap();
    runtime
        .context
        .enable_capture_ingestion("https://capture.example/".into(), Default::default())
        .unwrap();
    runtime
}
async fn fixture() -> (
    tempfile::TempDir,
    VaultRuntime,
    Arc<Batch>,
    Arc<Summary>,
    Arc<Auth>,
) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut runtime = open_runtime(&root).await;
    let auth = Arc::new(Auth(false.into()));
    let batch = Arc::new(Batch::default());
    let summary = Arc::new(Summary::default());
    runtime
        .prepare_capture_processing(policy("a"), auth.clone())
        .await
        .unwrap();
    runtime
        .enable_transcription(Arc::new(batch.clone()), auth.clone(), Default::default())
        .await
        .unwrap();
    runtime
        .enable_processing(summary.clone(), auth.clone())
        .await
        .unwrap();
    runtime.start_capture_processing();
    (temp, runtime, batch, summary, auth)
}
async fn capture(context: &Arc<VaultContext>, pending: bool) -> CaptureIngestStatus {
    let target = if pending {
        CaptureTarget::PendingSession {
            create_request_id: uuid::Uuid::new_v4().to_string(),
        }
    } else {
        let note = context
            .create_session(
                MEMBER.into(),
                uuid::Uuid::new_v4().to_string(),
                CreateSessionRequest {
                    title: "Meeting".into(),
                    markdown: "Notes".into(),
                },
            )
            .await
            .unwrap();
        CaptureTarget::ExistingSession {
            session_id: note.meta.id,
            expected_audio_sha256: None,
        }
    };
    context
        .begin_capture_authorized(
            MEMBER.into(),
            CaptureIngestRequest {
                identity: CaptureIdentity {
                    scope: CaptureScope {
                        server: "https://capture.example/".into(),
                        member_id: MEMBER.into(),
                        vault_id: "vault".into(),
                    },
                    capture_id: uuid::Uuid::new_v4().to_string(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    target,
                },
                format: CaptureFormat::stereo(),
                first_admitted_audio_observed_at_ms: 1_700_000_000_123,
            },
            || async { Ok(()) },
        )
        .await
        .unwrap()
}
fn request(source: &CaptureIngestStatus, summary: bool) -> CaptureProcessingRequest {
    CaptureProcessingRequest {
        capture_fingerprint: source.prefix.capture_fingerprint.clone(),
        model: None,
        languages: vec!["en".into()],
        keywords: vec!["Loofah".into()],
        summary: summary.then_some(CaptureProcessingSummary {
            target: DocumentTarget::Summary,
            title_policy: TitlePolicy::Preserve,
        }),
    }
}
async fn submit(
    context: &Arc<VaultContext>,
    source: &CaptureIngestStatus,
    summary: bool,
) -> CaptureProcessingSnapshot {
    context
        .submit_capture_processing_authorized(
            MEMBER.into(),
            source.identity.capture_id.clone(),
            source.identity.request_id.clone(),
            request(source, summary),
            || async { Ok(()) },
        )
        .await
        .unwrap()
}
async fn publish(context: &Arc<VaultContext>, source: &CaptureIngestStatus) {
    let writer = context
        .capture_writer(MEMBER.into(), source.identity.capture_id.clone())
        .await
        .unwrap();
    writer
        .append_chunk(0, 1600, vec![0; 1600 * 2 * 4])
        .await
        .unwrap();
    let seal = writer.seal().await.unwrap();
    context
        .publish_capture_authorized(
            MEMBER.into(),
            source.identity.capture_id.clone(),
            seal,
            || async { Ok(()) },
        )
        .await
        .unwrap();
}
async fn terminal(
    context: &Arc<VaultContext>,
    source: &CaptureIngestStatus,
) -> CaptureProcessingSnapshot {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let state = context
                .capture_processing_status_authorized(
                    MEMBER.into(),
                    source.identity.capture_id.clone(),
                    || async { Ok(()) },
                )
                .await
                .unwrap();
            if state.state.is_terminal() {
                break state;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn sealed_pending_capture_repairs_then_summarizes_once_without_a_frontend() {
    let (_temp, runtime, batch, summary, _) = fixture().await;
    let context = &runtime.context;
    let source = capture(context, true).await;
    let admitted = submit(context, &source, true).await;
    let replay = submit(context, &source, true).await;
    assert_eq!(admitted.intent_id, replay.intent_id);
    publish(context, &source).await;
    let done = terminal(context, &source).await;
    assert_eq!(done.state, CaptureProcessingState::Succeeded, "{done:?}");
    assert!(done.live_generation_id.is_none());
    assert!(done.repair_job_id.is_some());
    assert!(done.summary_job_id.is_some());
    assert_eq!(batch.calls.load(Ordering::Acquire), 1);
    assert_eq!(summary.calls.load(Ordering::Acquire), 1);
    let session = done.session_id.unwrap();
    assert!(
        context
            .read_document(&session, &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .is_some()
    );
    assert_eq!(
        context
            .store
            .read_meta(&session)
            .await
            .unwrap()
            .unwrap()
            .created_at,
        "2023-11-14T22:13:20.123Z"
    );
    assert_eq!(
        submit(context, &source, true).await.state,
        CaptureProcessingState::Succeeded
    );
    assert!(
        context
            .capture_writer(MEMBER.into(), source.identity.capture_id)
            .await
            .unwrap()
            .snapshot()
            .await
            .unwrap()
            .seal
            .is_some()
    );
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn no_declaration_never_starts_old_capture_processing() {
    let (_temp, runtime, batch, summary, _) = fixture().await;
    let source = capture(&runtime.context, true).await;
    publish(&runtime.context, &source).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(batch.calls.load(Ordering::Acquire), 0);
    assert_eq!(summary.calls.load(Ordering::Acquire), 0);
    assert_eq!(
        runtime
            .context
            .capture_processing_status_authorized(
                MEMBER.into(),
                source.identity.capture_id,
                || async { Ok(()) }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn complete_empty_transcript_summarizes_saved_notes_only_when_explicitly_requested() {
    let (_temp, runtime, batch, summary, _) = fixture().await;
    batch.empty.store(true, Ordering::Release);
    let source = capture(&runtime.context, false).await;
    submit(&runtime.context, &source, true).await;
    publish(&runtime.context, &source).await;
    let done = terminal(&runtime.context, &source).await;
    assert_eq!(done.state, CaptureProcessingState::Empty, "{done:?}");
    assert!(done.transcript_publication.is_none());
    assert!(done.summary_job_id.is_some());
    assert_eq!(summary.calls.load(Ordering::Acquire), 1);
    let session = done.session_id.unwrap();
    assert!(
        runtime
            .context
            .read_document(&session, &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .is_some()
    );
    assert!(
        !runtime
            .context
            .store
            .vault_base()
            .join("sessions")
            .join(session)
            .join("transcript.json")
            .exists()
    );
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn cancellation_stops_processing_without_sealing_or_discarding_raw_audio() {
    let (_temp, runtime, batch, _, _) = fixture().await;
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, false).await;
    let cancelled = runtime
        .context
        .cancel_capture_processing_authorized(
            MEMBER.into(),
            source.identity.capture_id.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    assert_eq!(cancelled.state, CaptureProcessingState::Cancelled);
    publish(&runtime.context, &source).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(batch.calls.load(Ordering::Acquire), 0);
    assert!(
        runtime
            .context
            .capture_writer(MEMBER.into(), source.identity.capture_id)
            .await
            .unwrap()
            .snapshot()
            .await
            .unwrap()
            .publication
            .is_some()
    );
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn failed_native_inference_is_not_automatically_retried() {
    let (_temp, runtime, batch, _, _) = fixture().await;
    batch.fail.store(true, Ordering::Release);
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, false).await;
    publish(&runtime.context, &source).await;
    assert_eq!(
        terminal(&runtime.context, &source).await.state,
        CaptureProcessingState::Failed
    );
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(batch.calls.load(Ordering::Acquire), 1);
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn exact_intent_replay_rejects_retarget_options_and_foreign_members() {
    let (_temp, runtime, _, _, _) = fixture().await;
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, false).await;
    let mut changed = request(&source, false);
    changed.languages = vec!["nl".into()];
    assert_eq!(
        runtime
            .context
            .submit_capture_processing_authorized(
                MEMBER.into(),
                source.identity.capture_id.clone(),
                source.identity.request_id.clone(),
                changed,
                || async { Ok(()) }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert_eq!(
        runtime
            .context
            .capture_processing_status_authorized(
                uuid::Uuid::new_v4().to_string(),
                source.identity.capture_id,
                || async { Ok(()) }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn revoked_membership_prevents_children_and_keeps_source_readiness_blocked() {
    let (_temp, runtime, batch, _, auth) = fixture().await;
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, false).await;
    auth.0.store(true, Ordering::Release);
    let stopped = terminal(&runtime.context, &source).await;
    assert_eq!(
        stopped.failure,
        Some(CaptureProcessingFailure::AccessDenied)
    );
    publish(&runtime.context, &source).await;
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(batch.calls.load(Ordering::Acquire), 0);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn summary_source_changes_retain_generated_output_without_paid_retry() {
    let (temp, runtime, batch, summary, _) = fixture().await;
    summary.blocked.store(true, Ordering::Release);
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, true).await;
    publish(&runtime.context, &source).await;
    tokio::time::timeout(Duration::from_secs(20), summary.entered.notified())
        .await
        .unwrap();
    let current = runtime
        .context
        .capture_processing_status_authorized(
            MEMBER.into(),
            source.identity.capture_id.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let session = current.session_id.unwrap();
    let directory = temp.path().join("vault/sessions").join(&session);
    std::fs::write(directory.join("attachment.txt"), b"untouched attachment").unwrap();
    std::fs::write(
        directory.join("transcript.json"),
        br#"{"transcripts":[],"external":true}"#,
    )
    .unwrap();
    summary.release.notify_one();
    let done = terminal(&runtime.context, &source).await;
    assert_eq!(done.state, CaptureProcessingState::Conflict, "{done:?}");
    assert!(
        runtime
            .context
            .processing_job_result(MEMBER.into(), done.summary_job_id.unwrap())
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(summary.calls.load(Ordering::Acquire), 1);
    assert_eq!(batch.calls.load(Ordering::Acquire), 1);
    assert_eq!(
        std::fs::read(directory.join("attachment.txt")).unwrap(),
        b"untouched attachment"
    );
    assert!(
        runtime
            .context
            .read_document(&session, &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .is_none()
    );
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn restart_reuses_completed_children_without_repeating_paid_or_native_work() {
    let (temp, runtime, batch, summary, _) = fixture().await;
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, true).await;
    publish(&runtime.context, &source).await;
    let first = terminal(&runtime.context, &source).await;
    runtime.shutdown().await.unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut runtime = open_runtime(&root).await;
    let auth = Arc::new(Auth(false.into()));
    runtime
        .prepare_capture_processing(policy("a"), auth.clone())
        .await
        .unwrap();
    runtime
        .enable_transcription(Arc::new(batch.clone()), auth.clone(), Default::default())
        .await
        .unwrap();
    runtime
        .enable_processing(summary.clone(), auth)
        .await
        .unwrap();
    runtime.start_capture_processing();
    let replay = submit(&runtime.context, &source, true).await;
    assert_eq!(replay, first);
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(batch.calls.load(Ordering::Acquire), 1);
    assert_eq!(summary.calls.load(Ordering::Acquire), 1);
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn restart_model_rebind_never_runs_queued_old_intent_with_new_bytes() {
    let (temp, runtime, batch, _, _) = fixture().await;
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, false).await;
    runtime.shutdown().await.unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut runtime = open_runtime(&root).await;
    let auth = Arc::new(Auth(false.into()));
    runtime
        .prepare_capture_processing(policy("b"), auth.clone())
        .await
        .unwrap();
    runtime
        .enable_transcription(Arc::new(batch.clone()), auth, Default::default())
        .await
        .unwrap();
    runtime.start_capture_processing();
    publish(&runtime.context, &source).await;
    let done = terminal(&runtime.context, &source).await;
    assert_eq!(done.state, CaptureProcessingState::Uncertain);
    assert_eq!(done.failure, Some(CaptureProcessingFailure::ModelChanged));
    assert_eq!(batch.calls.load(Ordering::Acquire), 0);
    let session = done
        .session_id
        .unwrap_or_else(|| VaultContext::capture_target_session_id(&source.identity).unwrap());
    assert!(
        runtime
            .context
            .ensure_capture_intent_idle(&session, false)
            .await
            .is_err()
    );
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn unknown_future_duplicate_or_foreign_journals_fail_closed_without_rewrite() {
    for corrupt in ["future", "duplicate", "foreign"] {
        let (temp, runtime, _, _, _) = fixture().await;
        let source = capture(&runtime.context, true).await;
        submit(&runtime.context, &source, false).await;
        runtime.shutdown().await.unwrap();
        let root = temp.path().canonicalize().unwrap();
        let path = root
            .join("operations/capture-processing")
            .join(format!("{}.json", source.identity.capture_id));
        let original = std::fs::read_to_string(&path).unwrap();
        let modified = match corrupt {
            "future" => original.replacen("\"version\":1", "\"version\":999", 1),
            "duplicate" => original.replacen("\"version\":1", "\"version\":1,\"version\":1", 1),
            _ => original.replacen("\"vault_id\":\"vault\"", "\"vault_id\":\"foreign\"", 1),
        };
        assert_ne!(modified, original);
        std::fs::write(&path, &modified).unwrap();
        let mut runtime = open_runtime(&root).await;
        assert!(
            runtime
                .prepare_capture_processing(policy("a"), Arc::new(Auth(false.into())))
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), modified);
        runtime.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn dropped_admission_caller_keeps_owned_authorization_and_durable_declaration() {
    let (_temp, runtime, _, _, _) = fixture().await;
    let source = capture(&runtime.context, true).await;
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let caller = tokio::spawn({
        let context = runtime.context.clone();
        let source = source.clone();
        let entered = entered.clone();
        let release = release.clone();
        async move {
            context
                .submit_capture_processing_authorized(
                    MEMBER.into(),
                    source.identity.capture_id.clone(),
                    source.identity.request_id.clone(),
                    request(&source, false),
                    move || async move {
                        entered.notify_one();
                        release.notified().await;
                        Ok(())
                    },
                )
                .await
        }
    });
    entered.notified().await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if runtime
                .context
                .capture_processing_service()
                .unwrap()
                .journal
                .get(MEMBER, &source.identity.capture_id)
                .is_ok()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn existing_capture_delivers_captions_before_seal_then_uses_live_publication() {
    let (_temp, mut runtime, batch, summary, auth) = fixture().await;
    let fake = crate::capture_live::tests::Fake::new();
    fake.release_processing_fixture();
    runtime
        .enable_capture_live(Arc::new(fake), auth)
        .await
        .unwrap();
    let source = capture(&runtime.context, false).await;
    submit(&runtime.context, &source, true).await;
    let writer = runtime
        .context
        .capture_writer(MEMBER.into(), source.identity.capture_id.clone())
        .await
        .unwrap();
    writer
        .append_chunk(0, 1600, vec![0; 1600 * 2 * 4])
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let page = runtime
                .context
                .capture_live_captions_authorized(
                    MEMBER.into(),
                    source.identity.capture_id.clone(),
                    source.identity.request_id.clone(),
                    None,
                    16,
                    || async { Ok(()) },
                )
                .await;
            if page.as_ref().is_ok_and(|page| !page.captions.is_empty()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert!(writer.snapshot().await.unwrap().seal.is_none());
    let seal = writer.seal().await.unwrap();
    runtime
        .context
        .publish_capture_authorized(
            MEMBER.into(),
            source.identity.capture_id.clone(),
            seal,
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let done = terminal(&runtime.context, &source).await;
    let live = runtime
        .context
        .capture_live_processing_outcome(MEMBER.into(), source.identity.request_id.clone())
        .await;
    assert_eq!(
        done.state,
        CaptureProcessingState::Succeeded,
        "{done:?}; live={live:?}"
    );
    assert!(done.live_generation_id.is_some());
    assert!(done.repair_job_id.is_none());
    assert_eq!(batch.calls.load(Ordering::Acquire), 0);
    assert_eq!(summary.calls.load(Ordering::Acquire), 1);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn unavailable_before_native_prepare_retries_the_same_repair_without_duplicate_inference() {
    let (_temp, runtime, batch, _, _) = fixture().await;
    batch.prepare_busy.store(true, Ordering::Release);
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, false).await;
    publish(&runtime.context, &source).await;
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let state = runtime
                .context
                .capture_processing_service()
                .unwrap()
                .journal
                .get(MEMBER, &source.identity.capture_id)
                .unwrap();
            if let Some(id) = state.snapshot.repair_job_id {
                if runtime
                    .context
                    .transcription_job(MEMBER.into(), id)
                    .await
                    .unwrap()
                    .failure
                    == Some(TranscriptionFailure::ResourceLimit)
                {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(batch.calls.load(Ordering::Acquire), 0);
    batch.prepare_busy.store(false, Ordering::Release);
    assert_eq!(
        terminal(&runtime.context, &source).await.state,
        CaptureProcessingState::Succeeded
    );
    assert_eq!(batch.calls.load(Ordering::Acquire), 1);
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn cancellation_retains_lifecycle_exclusion_until_admitted_summary_has_actually_drained() {
    let (_temp, runtime, _, summary, _) = fixture().await;
    summary.blocked.store(true, Ordering::Release);
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, true).await;
    publish(&runtime.context, &source).await;
    tokio::time::timeout(Duration::from_secs(15), summary.entered.notified())
        .await
        .unwrap();
    let cancelled = runtime
        .context
        .cancel_capture_processing_authorized(
            MEMBER.into(),
            source.identity.capture_id.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    let session = cancelled.session_id.unwrap();
    assert!(
        runtime
            .context
            .ensure_capture_intent_idle(&session, false)
            .await
            .is_err()
    );
    summary.release.notify_one();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            if runtime
                .context
                .ensure_capture_intent_idle(&session, false)
                .await
                .is_ok()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        runtime
            .context
            .read_document(&session, &DocumentTarget::Summary)
            .await
            .unwrap()
            .document
            .is_none()
    );
    assert_eq!(summary.calls.load(Ordering::Acquire), 1);
    runtime.shutdown().await.unwrap();
}
#[test]
fn processing_crash_child() {
    let Ok(root) = std::env::var("LOOFAH_CAPTURE_PROCESSING_CRASH_ROOT") else {
        return;
    };
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let mut runtime = open_runtime(std::path::Path::new(&root)).await;
        let auth = Arc::new(Auth(false.into()));
        let batch = Arc::new(Batch::default());
        let summary = Arc::new(Summary::default());
        summary.blocked.store(true, Ordering::Release);
        runtime
            .prepare_capture_processing(policy("a"), auth.clone())
            .await
            .unwrap();
        runtime
            .enable_transcription(Arc::new(batch), auth.clone(), Default::default())
            .await
            .unwrap();
        runtime
            .enable_processing(summary.clone(), auth)
            .await
            .unwrap();
        runtime.start_capture_processing();
        let source = capture(&runtime.context, true).await;
        std::fs::write(
            std::path::Path::new(&root).join("test-capture.json"),
            serde_json::to_vec(&source).unwrap(),
        )
        .unwrap();
        submit(&runtime.context, &source, true).await;
        publish(&runtime.context, &source).await;
        summary.entered.notified().await;
        // Process death after the provider has accepted an attempt: no Drop or
        // invented failure response may authorize repeating that paid request.
        std::process::exit(73);
    });
}
#[tokio::test]
async fn crashed_paid_attempt_is_interrupted_and_never_inferred_again_on_restart() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let executable = std::env::current_exe().unwrap();
    let child_root = root.clone();
    let status = tokio::task::spawn_blocking(move || {
        std::process::Command::new(executable)
            .args([
                "--exact",
                "capture_processing::tests::processing_crash_child",
                "--nocapture",
            ])
            .env("LOOFAH_CAPTURE_PROCESSING_CRASH_ROOT", &child_root)
            .status()
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(status.code(), Some(73));
    let source: CaptureIngestStatus =
        serde_json::from_slice(&std::fs::read(root.join("test-capture.json")).unwrap()).unwrap();
    let mut runtime = open_runtime(&root).await;
    let batch = Arc::new(Batch::default());
    let summary = Arc::new(Summary::default());
    let auth = Arc::new(Auth(false.into()));
    runtime
        .prepare_capture_processing(policy("a"), auth.clone())
        .await
        .unwrap();
    runtime
        .enable_transcription(Arc::new(batch.clone()), auth.clone(), Default::default())
        .await
        .unwrap();
    runtime
        .enable_processing(summary.clone(), auth)
        .await
        .unwrap();
    runtime.start_capture_processing();
    assert_eq!(
        terminal(&runtime.context, &source).await.state,
        CaptureProcessingState::Interrupted
    );
    assert_eq!(batch.calls.load(Ordering::Acquire), 0);
    assert_eq!(summary.calls.load(Ordering::Acquire), 0);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn disabled_coordinator_blocks_queued_pipeline_children_but_keeps_manual_summaries_available()
{
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut runtime = open_runtime(&root).await;
    let auth = Arc::new(Auth(false.into()));
    let batch = Arc::new(Batch::default());
    runtime
        .prepare_capture_processing(policy("a"), auth.clone())
        .await
        .unwrap();
    runtime
        .enable_transcription(Arc::new(batch), auth, Default::default())
        .await
        .unwrap();
    runtime.start_capture_processing();
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, true).await;
    publish(&runtime.context, &source).await;
    let record = tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let record = runtime
                .context
                .capture_processing_service()
                .unwrap()
                .journal
                .get(MEMBER, &source.identity.capture_id)
                .unwrap();
            if record.summary_request.is_some() {
                break record;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let manual = runtime
        .context
        .create_session(
            MEMBER.into(),
            uuid::Uuid::new_v4().to_string(),
            CreateSessionRequest {
                title: "Unrelated manual note".into(),
                markdown: "Saved notes".into(),
            },
        )
        .await
        .unwrap();
    let document = runtime
        .context
        .read_document(&manual.meta.id, &DocumentTarget::Summary)
        .await
        .unwrap();
    let journal = JobJournal::open(root.join("operations/jobs"), runtime.context.events.clone())
        .await
        .unwrap();
    let owned = journal
        .submit(
            MEMBER.into(),
            child_request_id(&record, "summary"),
            record.summary_request.unwrap(),
        )
        .await
        .unwrap();
    let manual = journal
        .submit(
            MEMBER.into(),
            uuid::Uuid::new_v4().to_string(),
            SummaryJobRequest {
                only_if_empty: false,
                session_id: manual.meta.id,
                target: DocumentTarget::Summary,
                expected_revision: document.revision,
                title_policy: TitlePolicy::Preserve,
            },
        )
        .await
        .unwrap();
    drop(journal);
    runtime.shutdown().await.unwrap();
    let mut runtime = open_runtime(&root).await;
    let generator = Arc::new(Summary::default());
    // Intentionally no CPU engine or processing coordinator on this restart.
    runtime
        .enable_processing(generator.clone(), Arc::new(Auth(false.into())))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let owned = runtime
                .context
                .processing_job(MEMBER.into(), owned.id.clone())
                .await
                .unwrap();
            let manual = runtime
                .context
                .processing_job(MEMBER.into(), manual.id.clone())
                .await
                .unwrap();
            if owned.state.is_terminal() && manual.state.is_terminal() {
                assert_eq!(owned.state, ProcessingJobState::Interrupted);
                assert_eq!(manual.state, ProcessingJobState::Succeeded);
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(generator.calls.load(Ordering::Acquire), 1);
    assert!(
        runtime
            .context
            .ensure_capture_intent_idle(&record.target_session, false)
            .await
            .is_err()
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn coordinator_panic_fails_closed_without_stopping_durable_capture_or_restarting_children() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut runtime = open_runtime(&root).await;
    let mut policy = policy("a");
    policy.availability = Arc::new(|| panic!("injected host observer failure"));
    runtime
        .prepare_capture_processing(policy, Arc::new(Auth(false.into())))
        .await
        .unwrap();
    runtime.start_capture_processing();
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, false).await;
    let service = runtime.context.capture_processing.get().unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while !service.failed.load(Ordering::Acquire) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        runtime
            .context
            .capture_processing_status_authorized(
                MEMBER.into(),
                source.identity.capture_id.clone(),
                || async { Ok(()) }
            )
            .await
            .is_err()
    );
    let writer = runtime
        .context
        .capture_writer(MEMBER.into(), source.identity.capture_id.clone())
        .await
        .unwrap();
    writer.append_chunk(0, 16, vec![0; 16 * 8]).await.unwrap();
    assert_eq!(writer.snapshot().await.unwrap().frames, 16);
    let retained = service
        .journal
        .get(MEMBER, &source.identity.capture_id)
        .unwrap();
    assert!(!retained.snapshot.state.is_terminal());
    assert!(runtime.shutdown().await.is_err());
}

#[tokio::test]
async fn complete_empty_without_saved_note_input_fails_before_any_summary_provider_call() {
    let (_temp, runtime, batch, summary, _) = fixture().await;
    batch.empty.store(true, Ordering::Release);
    let source = capture(&runtime.context, true).await;
    submit(&runtime.context, &source, true).await;
    publish(&runtime.context, &source).await;
    let done = terminal(&runtime.context, &source).await;
    assert_eq!(done.state, CaptureProcessingState::Failed, "{done:?}");
    assert_eq!(
        done.failure,
        Some(CaptureProcessingFailure::SourceUnavailable)
    );
    assert!(done.transcript_publication.is_none());
    assert!(done.summary_job_id.is_none());
    assert_eq!(summary.calls.load(Ordering::Acquire), 0);
    runtime.shutdown().await.unwrap();
}
#[tokio::test]
async fn complete_empty_without_summary_declaration_never_starts_a_provider() {
    let (_temp, runtime, batch, summary, _) = fixture().await;
    batch.empty.store(true, Ordering::Release);
    let source = capture(&runtime.context, false).await;
    submit(&runtime.context, &source, false).await;
    publish(&runtime.context, &source).await;
    let done = terminal(&runtime.context, &source).await;
    assert_eq!(done.state, CaptureProcessingState::Empty);
    assert!(done.transcript_publication.is_none());
    assert!(done.summary_job_id.is_none());
    assert_eq!(summary.calls.load(Ordering::Acquire), 0);
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn missing_parent_never_turns_marked_queued_summary_into_manual_inference() {
    for attach in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let mut runtime = open_runtime(&root).await;
        let auth = Arc::new(Auth(false.into()));
        let batch = Arc::new(Batch::default());
        runtime
            .prepare_capture_processing(policy("a"), auth.clone())
            .await
            .unwrap();
        runtime
            .enable_transcription(Arc::new(batch), auth, Default::default())
            .await
            .unwrap();
        runtime.start_capture_processing();
        let source = capture(&runtime.context, true).await;
        submit(&runtime.context, &source, true).await;
        publish(&runtime.context, &source).await;
        let record = tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let record = runtime
                    .context
                    .capture_processing_service()
                    .unwrap()
                    .journal
                    .get(MEMBER, &source.identity.capture_id)
                    .unwrap();
                if record.summary_request.is_some() {
                    break record;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let journal =
            JobJournal::open(root.join("operations/jobs"), runtime.context.events.clone())
                .await
                .unwrap();
        let request_id = child_request_id(&record, "summary");
        let origin = child_origin(&record, "summary").unwrap();
        let child = journal
            .submit_with_origin(
                MEMBER.into(),
                request_id,
                record.summary_request.clone().unwrap(),
                Some(origin),
            )
            .await
            .unwrap();
        drop(journal);
        runtime.shutdown().await.unwrap();
        std::fs::rename(
            root.join("operations/capture-processing"),
            root.join("retained-capture-processing"),
        )
        .unwrap();
        let mut runtime = open_runtime(&root).await;
        if attach {
            runtime
                .prepare_capture_processing(policy("a"), Arc::new(Auth(false.into())))
                .await
                .unwrap();
            runtime.start_capture_processing();
        }
        let generator = Arc::new(Summary::default());
        runtime
            .enable_processing(generator.clone(), Arc::new(Auth(false.into())))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                let state = runtime
                    .context
                    .processing_job(MEMBER.into(), child.id.clone())
                    .await
                    .unwrap();
                if state.state.is_terminal() {
                    assert_eq!(state.state, ProcessingJobState::Interrupted);
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(generator.calls.load(Ordering::Acquire), 0);
        assert!(
            runtime
                .context
                .capture_writer(MEMBER.into(), source.identity.capture_id)
                .await
                .unwrap()
                .snapshot()
                .await
                .unwrap()
                .publication
                .is_some()
        );
        runtime.shutdown().await.unwrap();
    }
}
