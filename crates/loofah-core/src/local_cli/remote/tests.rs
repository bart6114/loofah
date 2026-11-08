use super::*;
use crate::{VaultRuntime, transcription_jobs::TranscriptionAccess};
use hypr_storage::vault::{LeaseAttempt, VaultLease};
use std::{future::Future, pin::Pin};

struct Authority {
    revoked: CancellationToken,
}
impl scope::MediaAuthorizer for Authority {
    fn authorize(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<Box<dyn Send>, ApplicationError>> + Send + '_>> {
        Box::pin(async move {
            if self.revoked.is_cancelled() {
                Err(error(ErrorCode::Forbidden, "Access revoked"))
            } else {
                Ok(Box::new(()) as Box<dyn Send>)
            }
        })
    }
    fn observe(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<TranscriptionAccess, ApplicationError>> + Send + '_>>
    {
        Box::pin(async move {
            drop(self.authorize().await?);
            Ok(TranscriptionAccess {
                cancelled: self.revoked.clone(),
                interrupted: CancellationToken::new(),
                owner: Box::new(()),
            })
        })
    }
}
impl crate::capture_processing::CaptureProcessingAuthorizer for Authority {
    fn authorize<'a>(
        &'a self,
        _: &'a str,
    ) -> crate::transcription_jobs::TranscriptionFuture<
        'a,
        Result<Box<dyn Send>, TranscriptionFailure>,
    > {
        Box::pin(async move { Ok(Box::new(()) as Box<dyn Send>) })
    }
}
struct Fixture {
    root: tempfile::TempDir,
    runtime: VaultRuntime,
    media: Arc<MediaDispatcher>,
    authority: Arc<Authority>,
    session: String,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let vault = root.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        let LeaseAttempt::Acquired(lease) = VaultLease::try_acquire_existing(&vault).unwrap()
        else {
            panic!("lease");
        };
        let mut runtime = VaultRuntime::open_leased(
            "test-vault".into(),
            lease,
            &root.path().join("cache"),
            &root.path().join("operations"),
            hypr_vault_write::ContentLimits::unlimited(),
        )
        .await
        .unwrap();
        runtime.context.startup.wait_until_ready().await.unwrap();
        let cli = Arc::new(LocalCli::new(runtime.context.clone()).unwrap());
        let reply = cli
            .execute(
                uuid::Uuid::new_v4().to_string(),
                LocalCliOperation::CreateSession {
                    title: "Media".into(),
                    markdown: Some("Keep note".into()),
                    metadata: Default::default(),
                },
                None,
                Arc::new(|_| {}),
            )
            .await
            .unwrap()
            .reply;
        let LocalCliReply::SessionCreated { meta } = reply else {
            panic!("session");
        };
        let authority = Arc::new(Authority {
            revoked: CancellationToken::new(),
        });
        runtime
            .prepare_capture_processing(
                crate::capture_processing::CaptureProcessingPolicy {
                    model: Some(crate::capture_processing::CaptureProcessingModel {
                        alias: "server-alias".into(),
                        sha256: "a".repeat(64),
                    }),
                    summary_enabled: false,
                    availability: Arc::new(|| TranscriptionAvailability::Ready),
                },
                authority.clone(),
            )
            .await
            .unwrap();
        let media = MediaDispatcher::new(
            runtime.context.clone(),
            "member-a".into(),
            authority.clone(),
            MediaLimits::default(),
        )
        .unwrap();
        Self {
            root,
            runtime,
            media,
            authority,
            session: meta.id,
        }
    }
    async fn declaration(&self, source: &[u8]) -> MediaDeclaration {
        MediaDeclaration {
            request_id: uuid::Uuid::new_v4().to_string(),
            operation: MediaOperation::ImportSubtitles {
                session_id: self.session.clone(),
                expected: self.media.source(&self.session).await.unwrap(),
                filename: "captions.srt".into(),
                source: MediaUploadSource {
                    size_bytes: source.len() as u64,
                    sha256: ledger::hash(source),
                },
            },
            transcription: None,
            summary: None,
        }
    }
    async fn stop(self) {
        self.media.shutdown().await;
        self.runtime.shutdown().await.unwrap();
    }
}
const SUBTITLE: &[u8] = b"1\n00:00:00,000 --> 00:00:01,000\nOriginal source words\n";
async fn done(media: &MediaDispatcher, request: &str) -> MediaRequestStatus {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let status = media.status(request).await.unwrap_or_else(|cause| {
                panic!(
                    "Media status failed for {request} under {}: {cause:?}",
                    media.root.display()
                )
            });
            if status.phase != MediaPhase::Running {
                return status;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn portable_subtitles_keep_original_member_and_canonical_note_without_native_host() {
    let fixture = Fixture::new().await;
    let input = fixture.declaration(SUBTITLE).await;
    let admission = fixture.media.declare(input.clone()).await.unwrap();
    assert_eq!(admission.status.phase, MediaPhase::Receiving);
    assert_eq!(admission.status.media.unwrap().session_id, fixture.session);
    fixture
        .media
        .receive(
            input.request_id.clone(),
            admission.revision.clone(),
            SUBTITLE,
        )
        .await
        .unwrap();
    let status = done(&fixture.media, &input.request_id).await;
    assert_eq!(status.phase, MediaPhase::Succeeded, "{:?}", status.media);
    let result = fixture
        .media
        .result(&input.request_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(result.user_id, "member-a");
    assert_eq!(result.session_id, fixture.session);
    assert_eq!(
        fixture
            .runtime
            .context
            .read_note(&fixture.session)
            .await
            .unwrap()
            .markdown
            .as_deref(),
        Some("Keep note")
    );
    assert_eq!(
        fixture.media.declare(input).await.unwrap().revision,
        admission.revision
    );
    fixture.stop().await;
}
#[tokio::test]
async fn changed_baseline_after_upload_declaration_never_adopts_current_transcript() {
    let fixture = Fixture::new().await;
    let input = fixture.declaration(SUBTITLE).await;
    let admission = fixture.media.declare(input.clone()).await.unwrap();
    let path = fixture
        .root
        .path()
        .join("vault/sessions")
        .join(&fixture.session)
        .join("transcript.json");
    std::fs::write(&path, b"{\"transcripts\":[],\"unknown\":true}").unwrap();
    let cause = fixture
        .media
        .receive(input.request_id.clone(), admission.revision, SUBTITLE)
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::Conflict);
    assert_eq!(
        std::fs::read(&path).unwrap(),
        b"{\"transcripts\":[],\"unknown\":true}"
    );
    assert!(
        fixture
            .media
            .result(&input.request_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        fixture.media.status(&input.request_id).await.unwrap().phase,
        MediaPhase::Review
    );
    fixture.stop().await;
}
#[tokio::test]
async fn member_private_requests_allow_same_uuid_without_cross_member_readback() {
    let fixture = Fixture::new().await;
    let input = fixture.declaration(SUBTITLE).await;
    let original = fixture.media.declare(input.clone()).await.unwrap();
    let other = MediaDispatcher::new(
        fixture.runtime.context.clone(),
        "member-b".into(),
        fixture.authority.clone(),
        MediaLimits::default(),
    )
    .unwrap();
    assert_eq!(
        other.status(&input.request_id).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    let separate = other.declare(input).await.unwrap();
    assert_ne!(separate.revision, original.revision);
    other.shutdown().await;
    fixture.stop().await;
}
#[tokio::test]
async fn interrupted_dispatched_work_requires_review_and_never_replays_parser() {
    let fixture = Fixture::new().await;
    let input = fixture.declaration(SUBTITLE).await;
    fixture.media.declare(input.clone()).await.unwrap();
    let mut record = fixture
        .media
        .load(&input.request_id)
        .await
        .unwrap()
        .unwrap();
    record.phase = MediaPhase::Running;
    fixture.media.save(record, false).await.unwrap();
    let resumed = MediaDispatcher::new(
        fixture.runtime.context.clone(),
        "member-a".into(),
        fixture.authority.clone(),
        MediaLimits::default(),
    )
    .unwrap();
    assert_eq!(
        resumed.status(&input.request_id).await.unwrap().phase,
        MediaPhase::Review
    );
    resumed.recover(&input.request_id).await.unwrap();
    assert_eq!(
        done(&resumed, &input.request_id).await.phase,
        MediaPhase::Review
    );
    assert!(resumed.result(&input.request_id).await.unwrap().is_none());
    resumed.shutdown().await;
    fixture.stop().await;
}
#[tokio::test]
async fn revocation_cancels_open_upload_and_shutdown_drains_original_receiver() {
    let fixture = Fixture::new().await;
    let input = fixture.declaration(SUBTITLE).await;
    let admission = fixture.media.declare(input.clone()).await.unwrap();
    let (sender, reader) = tokio::io::duplex(64);
    let media = fixture.media.clone();
    let id = input.request_id.clone();
    let receiving =
        tokio::spawn(async move { media.receive(id, admission.revision, reader).await });
    while fixture.media.receivers.lock().await.is_empty() {
        tokio::task::yield_now().await;
    }
    fixture.authority.revoked.cancel();
    assert_eq!(
        receiving.await.unwrap().unwrap_err().code,
        ErrorCode::Forbidden
    );
    fixture.media.shutdown().await;
    drop(sender);
    assert_eq!(
        fixture
            .media
            .status(&input.request_id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    fixture.stop().await;
}

#[tokio::test]
async fn cancellation_keeps_original_blocking_writer_owned_until_its_real_exit() {
    let fixture = Fixture::new().await;
    let input = fixture.declaration(SUBTITLE).await;
    let admission = fixture.media.declare(input.clone()).await.unwrap();
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (release, waiting) = std::sync::mpsc::channel();
    *fixture.media.source_write_pause.lock().unwrap() = Some((entered, waiting));
    let media = fixture.media.clone();
    let request = input.request_id.clone();
    let receiving =
        tokio::spawn(async move { media.receive(request, admission.revision, SUBTITLE).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), entry)
        .await
        .unwrap()
        .unwrap();
    let media = fixture.media.clone();
    let stopping = tokio::spawn(async move { media.shutdown().await });
    tokio::task::yield_now().await;
    assert!(!stopping.is_finished());
    assert!(!receiving.is_finished());
    let completion = fixture
        .media
        .receivers
        .lock()
        .await
        .get(&input.request_id)
        .map(|(_, done)| done.clone());
    // shutdown may already have taken the inventory, but must still await its actual writer.
    if let Some(done) = completion {
        assert!(!*done.borrow());
    }
    release.send(()).unwrap();
    assert!(receiving.await.unwrap().is_err());
    stopping.await.unwrap();
    assert!(
        !fixture
            .root
            .path()
            .join("vault/sessions")
            .join(&fixture.session)
            .join("transcript.json")
            .exists()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn declaration_and_recovery_share_one_dispatch_and_original_task_owner() {
    let fixture = Fixture::new().await;
    std::fs::write(
        fixture
            .root
            .path()
            .join("vault/sessions")
            .join(&fixture.session)
            .join("audio.wav"),
        b"immutable source",
    )
    .unwrap();
    let media = MediaDispatcher::with_capacity(
        fixture.runtime.context.clone(),
        "member-a".into(),
        fixture.authority.clone(),
        MediaLimits::default(),
        Arc::new(tokio::sync::Semaphore::new(0)),
    )
    .unwrap();
    let expected = media
        .source(&fixture.session)
        .await
        .unwrap()
        .recording
        .unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let input = MediaDeclaration {
        request_id: request.clone(),
        operation: MediaOperation::Transcribe { expected },
        transcription: Some(MediaTranscriptionOptions {
            model: "server-alias".into(),
            languages: vec![],
        }),
        summary: None,
    };
    let (entered, entry) = tokio::sync::oneshot::channel();
    let (release, waiting) = tokio::sync::oneshot::channel();
    *media.declaration_pause.lock().unwrap() = Some((entered, waiting));
    let declaring = media.clone();
    let declaration = tokio::spawn(async move { declaring.declare(input).await });
    tokio::time::timeout(std::time::Duration::from_secs(10), entry)
        .await
        .unwrap()
        .unwrap();
    let recovering = media.clone();
    let recovered_request = request.clone();
    let recovering = tokio::spawn(async move { recovering.recover(&recovered_request).await });
    tokio::task::yield_now().await;
    assert!(!recovering.is_finished());
    release.send(()).unwrap();
    declaration.await.unwrap().unwrap();
    assert_eq!(
        recovering.await.unwrap().unwrap().phase,
        MediaPhase::Running
    );
    assert_eq!(media.tasks.lock().await.len(), 1);
    media.shutdown().await;
    fixture.stop().await;
}

#[tokio::test]
async fn changed_or_unbound_operator_model_never_reinterprets_retained_media_source() {
    let fixture = Fixture::new().await;
    for original in [None, Some("b".repeat(64))] {
        let bytes = b"retained raw source before decode";
        let input = MediaDeclaration {
            request_id: uuid::Uuid::new_v4().to_string(),
            operation: MediaOperation::ImportAudio {
                session_id: None,
                expected: None,
                filename: "recording.wav".into(),
                content_type: Some("audio/wav".into()),
                created_at: None,
                initial: None,
                transcribe: true,
                source: MediaUploadSource {
                    size_bytes: bytes.len() as u64,
                    sha256: ledger::hash(bytes),
                },
            },
            transcription: Some(MediaTranscriptionOptions {
                model: "server-alias".into(),
                languages: vec![],
            }),
            summary: None,
        };
        let admitted = fixture.media.declare(input.clone()).await.unwrap();
        let request = input.request_id;
        let mut record = fixture.media.load(&request).await.unwrap().unwrap();
        assert_eq!(
            record.model_sha256.as_deref(),
            Some("a".repeat(64).as_str())
        );
        record.model_sha256 = original;
        fixture.media.save(record, false).await.unwrap();
        let review = fixture.media.status(&request).await.unwrap();
        assert_eq!(review.phase, MediaPhase::Review);
        assert!(review.media.unwrap().error.is_some());
        let received = fixture
            .media
            .receive(request.clone(), admitted.revision, bytes.as_slice())
            .await;
        assert_eq!(received.unwrap_err().code, ErrorCode::Conflict);
        assert!(fixture.media.tasks.lock().await.is_empty());
        let status = fixture.media.status(&request).await.unwrap();
        assert_eq!(status.phase, MediaPhase::Review);
        let retained = fixture.media.load(&request).await.unwrap().unwrap();
        assert!(
            retained
                .refusal
                .as_ref()
                .unwrap()
                .message
                .contains("model changed")
        );
        assert!(
            !fixture
                .runtime
                .context
                .store
                .vault_base()
                .join("sessions")
                .join(status.media.unwrap().session_id)
                .exists()
        );
        assert!(fixture.media.artifact(&retained).await.unwrap().is_some());
        assert_eq!(
            fixture
                .media
                .declare(status.declaration)
                .await
                .unwrap()
                .status
                .phase,
            MediaPhase::Review
        );
    }
    fixture.stop().await;
}

#[tokio::test]
async fn original_source_export_is_verified_private_and_never_dispatches() {
    let fixture = Fixture::new().await;
    let input = fixture.declaration(SUBTITLE).await;
    let request = input.request_id.clone();
    let admission = fixture.media.declare(input).await.unwrap();
    assert_eq!(
        fixture
            .media
            .open_source(&request, &admission.revision)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Unavailable
    );
    assert_eq!(
        fixture
            .media
            .open_source(&request, "another-revision")
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Conflict
    );
    let directory = fixture.media.root.join(format!("{request}.source"));
    ledger::ensure_directory(&directory).unwrap();
    std::fs::write(directory.join("partial"), SUBTITLE).unwrap();
    assert_eq!(
        fixture
            .media
            .open_source(&request, &admission.revision)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Unavailable
    );
    std::fs::write(directory.join("source"), SUBTITLE).unwrap();
    let reader = fixture
        .media
        .open_source(&request, &admission.revision)
        .await
        .unwrap();
    assert_eq!(reader.sha256(), ledger::hash(SUBTITLE));
    assert_eq!(
        reader
            .read_chunk_with_guard(0, 64 * 1024, ())
            .await
            .unwrap(),
        SUBTITLE
    );
    let other = MediaDispatcher::new(
        fixture.runtime.context.clone(),
        "member-b".into(),
        fixture.authority.clone(),
        MediaLimits::default(),
    )
    .unwrap();
    assert_eq!(
        other
            .open_source(&request, &admission.revision)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::NotFound
    );
    let mut changed = SUBTITLE.to_vec();
    changed[0] = b'9';
    std::fs::write(directory.join("source"), changed).unwrap();
    assert_eq!(
        reader
            .read_chunk_with_guard(0, 64 * 1024, ())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    assert_eq!(
        fixture
            .media
            .open_source(&request, &admission.revision)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Unavailable
    );
    fixture.authority.revoked.cancel();
    assert_eq!(
        reader
            .read_chunk_with_guard(0, 64 * 1024, ())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    assert!(fixture.media.tasks.lock().await.is_empty());
    assert!(
        !fixture
            .runtime
            .context
            .store
            .vault_base()
            .join("sessions")
            .join(&fixture.session)
            .join("transcript.json")
            .exists()
    );
    drop(reader);
    fixture.stop().await;
}

#[tokio::test]
async fn cancelled_original_source_reader_keeps_actual_io_and_shutdown_pinned() {
    use std::sync::atomic::{AtomicBool, Ordering};
    struct Guard(Arc<AtomicBool>);
    impl Drop for Guard {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }
    let fixture = Fixture::new().await;
    let input = fixture.declaration(SUBTITLE).await;
    let request = input.request_id.clone();
    let admission = fixture.media.declare(input).await.unwrap();
    let directory = fixture.media.root.join(format!("{request}.source"));
    ledger::ensure_directory(&directory).unwrap();
    std::fs::write(directory.join("source"), SUBTITLE).unwrap();
    let reader = fixture
        .media
        .open_source(&request, &admission.revision)
        .await
        .unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, paused) = std::sync::mpsc::channel();
    *reader.read_pause.lock().unwrap() = Some((entered, paused));
    let retired = Arc::new(AtomicBool::new(false));
    let guard = Guard(retired.clone());
    let caller =
        tokio::spawn(async move { reader.read_chunk_with_guard(0, 64 * 1024, guard).await });
    waiting.await.unwrap();
    caller.abort();
    let _ = caller.await;
    assert!(!retired.load(Ordering::Acquire));
    let shutdown = fixture.media.shutdown();
    tokio::pin!(shutdown);
    assert!(futures_util::poll!(&mut shutdown).is_pending());
    assert!(!retired.load(Ordering::Acquire));
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), &mut shutdown)
        .await
        .unwrap();
    assert!(retired.load(Ordering::Acquire));
    drop(shutdown);
    fixture.runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn operational_media_counts_recover_durable_receiving_without_reading_source() {
    use crate::operational::Sample;
    let fixture = Fixture::new().await;
    let declaration = fixture.declaration(b"PRIVATE-SOURCE-CANARY").await;
    fixture.media.declare(declaration.clone()).await.unwrap();
    let Sample::Available(counts) = fixture.runtime.context.sample_operational_media().await else {
        panic!("sample unavailable");
    };
    assert_eq!(counts.receiving, 1);
    assert_eq!(counts.declared_bytes, 21);
    assert!(!serde_json::to_string(&counts).unwrap().contains("CANARY"));
    fixture.media.shutdown().await;
    let path = fixture
        .media
        .root
        .join(format!("{}.admission.json", declaration.request_id));
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["version"] = 999.into();
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(matches!(
        fixture.runtime.context.sample_operational_media().await,
        Sample::Unavailable
    ));
    fixture.runtime.shutdown().await.unwrap();
}
