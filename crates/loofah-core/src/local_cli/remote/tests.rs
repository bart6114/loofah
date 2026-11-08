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
        let runtime = VaultRuntime::open_leased(
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
        MediaPhase::Ready
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
    assert_eq!(
        resumed.recover(&input.request_id).await.unwrap().phase,
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
