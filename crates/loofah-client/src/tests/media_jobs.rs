use super::*;
use crate::media_jobs::{MediaAuthority, MediaScope, MediaSubmissions};

fn declaration() -> MediaDeclaration {
    MediaDeclaration {
        request_id: uuid::Uuid::new_v4().to_string(),
        operation: MediaOperation::ImportAudio {
            session_id: None,
            expected: None,
            filename: "original.wav".into(),
            content_type: Some("audio/wav".into()),
            created_at: Some("2026-11-08T10:00:00Z".into()),
            initial: None,
            transcribe: false,
            source: MediaUploadSource {
                sha256: Sha256::digest(b"original bytes")
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect(),
                size_bytes: 14,
            },
        },
        transcription: None,
        summary: None,
    }
}
async fn authority(
    fixture: &Fixture,
    client: &Arc<RemoteClient>,
    root: &std::path::Path,
    cancel: CancellationToken,
) -> (Arc<MediaAuthority>, Arc<ProfileLease>) {
    let profiles = ConnectionProfiles::new(root.join("profiles")).unwrap();
    let profile = profiles.create("media", client.address()).await.unwrap();
    let mut selected = profile.profile().await;
    selected.member_id = Some("member".into());
    selected.vault_id = Some("vault".into());
    selected
        .credential_accounts
        .push(client.credential_account().into());
    profile.save(selected.clone()).await.unwrap();
    fixture.login(client).await;
    let scope = MediaScope {
        profile_id: selected.id,
        server: client.address().into(),
        member_id: "member".into(),
        vault_id: "vault".into(),
    };
    (
        client
            .bind_media(profile.clone(), scope, cancel)
            .await
            .unwrap(),
        profile,
    )
}
#[tokio::test]
async fn media_receipts_are_immutable_scoped_and_read_only_until_prepare() {
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let store = MediaSubmissions::new(root.join("receipts")).unwrap();
    let scope = MediaScope {
        profile_id: uuid::Uuid::new_v4().to_string(),
        server: "https://original.example/".into(),
        member_id: "alice".into(),
        vault_id: "private".into(),
    };
    assert!(store.list(scope.clone()).await.unwrap().is_empty());
    assert!(!root.join("receipts").exists());
    let input = declaration();
    let (first, created) = store.prepare(scope.clone(), input.clone()).await.unwrap();
    assert!(created);
    assert!(!store.prepare(scope.clone(), input.clone()).await.unwrap().1);
    let mut changed = input.clone();
    if let MediaOperation::ImportAudio { created_at, .. } = &mut changed.operation {
        *created_at = None;
    }
    assert_eq!(
        store
            .prepare(scope.clone(), changed)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let mut other = scope.clone();
    other.member_id = "bob".into();
    assert!(
        store
            .load(other, input.request_id.clone())
            .await
            .unwrap()
            .is_none()
    );
    let status = MediaRequestStatus {
        declaration: input.clone(),
        revision: uuid::Uuid::new_v4().to_string(),
        phase: MediaPhase::Receiving,
        media: None,
    };
    let admission = MediaAdmission {
        declaration: input.clone(),
        revision: status.revision.clone(),
        status,
    };
    let observed = store.observe_admission(&first, &admission).await.unwrap();
    assert_eq!(
        observed.admission_revision.as_deref(),
        Some(admission.revision.as_str())
    );
    let mut changed = admission.clone();
    changed.revision = "b".repeat(64);
    changed.status.revision = changed.revision.clone();
    assert!(store.observe_admission(&observed, &changed).await.is_err());
    assert_eq!(
        store
            .load(scope, input.request_id)
            .await
            .unwrap()
            .unwrap()
            .admission_revision,
        observed.admission_revision
    );
}
#[tokio::test]
async fn media_capability_precedes_credentials_and_original_authority_rejects_login_aba() {
    let fixture = Fixture::with_tls(Some(Duration::ZERO)).await;
    let mut client = fixture.client().await;
    let before = fixture.credentials.loads.load(Ordering::SeqCst);
    assert_eq!(
        client
            .media_jobs_capability(&CancellationToken::new())
            .unwrap_err()
            .code,
        ErrorCode::IncompatibleVersion
    );
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), before);
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("media_jobs".into());
    assert_eq!(
        client
            .media_initial_metadata_capability(&CancellationToken::new())
            .unwrap_err()
            .code,
        ErrorCode::IncompatibleVersion
    );
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), before);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let (bound, _profile) = authority(&fixture, &client, &path, CancellationToken::new()).await;
    bound.check_current().await.unwrap();
    fixture.login(&client).await;
    assert_eq!(
        bound.check_current().await.unwrap_err().code,
        ErrorCode::Conflict
    );
}
#[tokio::test]
async fn media_request_cancellation_keeps_original_authority_for_explicit_recovery() {
    let fixture = Fixture::with_tls(Some(Duration::ZERO)).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("media_jobs".into());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let lifetime = CancellationToken::new();
    let (bound, profile) = authority(&fixture, &client, &path, lifetime.clone()).await;
    let request = CancellationToken::new();
    let call = bound.for_request(request.clone());
    request.cancel();
    assert!(call.check_current().await.is_err());
    bound.check_current().await.unwrap();
    let original = profile.profile().await;
    let mut changed = original.clone();
    changed.vault_id = Some("other".into());
    profile.save(changed).await.unwrap();
    profile.save(original).await.unwrap();
    assert_eq!(
        bound.check_current().await.unwrap_err().code,
        ErrorCode::Conflict
    );
    lifetime.cancel();
    assert!(
        bound
            .for_request(CancellationToken::new())
            .check_current()
            .await
            .is_err()
    );
}
#[tokio::test]
async fn media_upload_uses_original_admission_and_lost_reply_never_replays() {
    #[derive(Default)]
    struct Server {
        admission: Option<MediaAdmission>,
        writes: usize,
    }
    let state = Arc::new(Mutex::new(Server::default()));
    let declaring = state.clone();
    let receiving = state.clone();
    let extra = Router::new()
        .route(
            "/api/v1/vaults/vault/media/requests",
            post(move |Json(input): Json<MediaDeclaration>| {
                let state = declaring.clone();
                async move {
                    let status = MediaRequestStatus {
                        declaration: input.clone(),
                        revision: uuid::Uuid::new_v4().to_string(),
                        phase: MediaPhase::Receiving,
                        media: None,
                    };
                    let admission = MediaAdmission {
                        declaration: input,
                        revision: status.revision.clone(),
                        status,
                    };
                    state.lock().await.admission = Some(admission.clone());
                    Json(admission)
                }
            }),
        )
        .route(
            "/api/v1/vaults/vault/media/requests/{request}/source",
            axum::routing::put(move |headers: HeaderMap, bytes: axum::body::Bytes| {
                let state = receiving.clone();
                async move {
                    let mut state = state.lock().await;
                    assert_eq!(
                        headers["x-loofah-media-revision"].to_str().unwrap(),
                        state.admission.as_ref().unwrap().revision
                    );
                    assert_eq!(bytes.as_ref(), b"original bytes");
                    assert!(!headers.contains_key("content-length"));
                    state.writes += 1;
                    (
                        StatusCode::OK,
                        [("content-type", "application/json")],
                        "lost original response",
                    )
                }
            }),
        );
    let fixture = Fixture::with_extra_routes(Some(Duration::ZERO), extra).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("media_jobs".into());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let (bound, _profile) = authority(&fixture, &client, &path, CancellationToken::new()).await;
    let store = MediaSubmissions::new(path.join("receipts")).unwrap();
    let (submission, _) = store
        .prepare(bound.scope().clone(), declaration())
        .await
        .unwrap();
    let admission = bound.declare(&submission).await.unwrap();
    assert_eq!(
        bound
            .upload_source(
                &submission,
                &admission,
                std::io::Cursor::new(b"original bytes")
            )
            .await
            .unwrap_err()
            .mutation_outcome,
        Some(MutationOutcome::NotCommitted)
    );
    assert_eq!(state.lock().await.writes, 0);
    let submission = store
        .observe_admission(&submission, &admission)
        .await
        .unwrap();
    let cause = bound
        .upload_source(
            &submission,
            &admission,
            std::io::Cursor::new(b"original bytes"),
        )
        .await
        .unwrap_err();
    assert_eq!(cause.mutation_outcome, Some(MutationOutcome::Unknown));
    assert_eq!(
        cause.request_id.as_deref(),
        Some(submission.declaration.request_id.as_str())
    );
    assert_eq!(state.lock().await.writes, 1);
    assert_eq!(
        store
            .load(
                bound.scope().clone(),
                submission.declaration.request_id.clone()
            )
            .await
            .unwrap()
            .unwrap()
            .declaration,
        submission.declaration
    );
}

#[tokio::test]
async fn cancelled_media_upload_keeps_original_profile_until_actual_source_read_drains() {
    struct BlockedSource {
        entered: Option<tokio::sync::oneshot::Sender<()>>,
        release: std::sync::mpsc::Receiver<()>,
        dropped: Option<tokio::sync::oneshot::Sender<()>>,
    }
    impl std::io::Read for BlockedSource {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            if let Some(entered) = self.entered.take() {
                let _ = entered.send(());
                let _ = self.release.recv();
            }
            Ok(0)
        }
    }
    impl Drop for BlockedSource {
        fn drop(&mut self) {
            if let Some(dropped) = self.dropped.take() {
                let _ = dropped.send(());
            }
        }
    }
    let extra = Router::new()
        .route(
            "/api/v1/vaults/vault/media/requests",
            post(|Json(input): Json<MediaDeclaration>| async move {
                let status = MediaRequestStatus {
                    declaration: input.clone(),
                    revision: uuid::Uuid::new_v4().to_string(),
                    phase: MediaPhase::Receiving,
                    media: None,
                };
                Json(MediaAdmission {
                    declaration: input,
                    revision: status.revision.clone(),
                    status,
                })
            }),
        )
        .route(
            "/api/v1/vaults/vault/media/requests/{request}/source",
            axum::routing::put(|_: axum::body::Bytes| async { StatusCode::NO_CONTENT }),
        );
    let fixture = Fixture::with_extra_routes(Some(Duration::ZERO), extra).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("media_jobs".into());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let (bound, profile) = authority(&fixture, &client, &path, CancellationToken::new()).await;
    let store = MediaSubmissions::new(path.join("receipts")).unwrap();
    let (submission, _) = store
        .prepare(bound.scope().clone(), declaration())
        .await
        .unwrap();
    let admission = bound.declare(&submission).await.unwrap();
    let submission = store
        .observe_admission(&submission, &admission)
        .await
        .unwrap();
    let (entered_send, entered) = tokio::sync::oneshot::channel();
    let (release, receive) = std::sync::mpsc::channel();
    let (dropped_send, dropped) = tokio::sync::oneshot::channel();
    let source = BlockedSource {
        entered: Some(entered_send),
        release: receive,
        dropped: Some(dropped_send),
    };
    let call = bound.for_request(CancellationToken::new());
    let pending =
        tokio::spawn(async move { call.upload_source(&submission, &admission, source).await });
    let started = tokio::time::timeout(Duration::from_secs(10), entered).await;
    if !matches!(started, Ok(Ok(()))) {
        let _ = release.send(());
        panic!("source reader did not start");
    }
    pending.abort();
    let _ = pending.await;
    drop(bound);
    drop(profile);
    drop(client);
    let profiles = ConnectionProfiles::new(path.join("profiles")).unwrap();
    let retained = profiles.open("media").await.is_err();
    let _ = release.send(());
    assert!(
        retained,
        "cancelled caller released original profile while source read remained active"
    );
    tokio::time::timeout(Duration::from_secs(10), dropped)
        .await
        .unwrap()
        .unwrap();
    let reopened = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(profile) = profiles.open("media").await {
                break profile;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(reopened.profile().await.name, "media");
}

mod source_export;

#[tokio::test]
async fn media_json_delayed_headers_do_not_hold_profile_commit_lock() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let fixture = Fixture::with_extra_routes(
        Some(Duration::ZERO),
        Router::new().route(
            "/api/v1/vaults/vault/media/requests",
            get({
                let entered = entered.clone();
                let release = release.clone();
                move || {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        entered.notify_one();
                        release.notified().await;
                        Json(Vec::<MediaRequestStatus>::new())
                    }
                }
            }),
        ),
    )
    .await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("media_jobs".into());
    let root = tempfile::tempdir().unwrap();
    let (authority, profile) =
        authority(&fixture, &client, root.path(), CancellationToken::new()).await;
    let original = profile.profile().await;
    let task = tokio::spawn(async move { authority.list(None).await });
    tokio::time::timeout(Duration::from_secs(2), entered.notified())
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_millis(200), profile.save(original))
        .await
        .unwrap()
        .unwrap();
    release.notify_one();
    tokio::time::timeout(Duration::from_secs(2), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn queued_media_dispatch_rejects_original_profile_and_login_aba_before_http() {
    for change in ["profile", "login"] {
        let requests = Arc::new(AtomicUsize::new(0));
        let seen = requests.clone();
        let fixture = Fixture::with_extra_routes(
            Some(Duration::ZERO),
            Router::new().route(
                "/api/v1/vaults/vault/media/requests",
                get(move || {
                    let seen = seen.clone();
                    async move {
                        seen.fetch_add(1, Ordering::SeqCst);
                        Json(Vec::<MediaRequestStatus>::new())
                    }
                }),
            ),
        )
        .await;
        let mut client = fixture.client().await;
        Arc::get_mut(&mut client)
            .unwrap()
            .capabilities
            .features
            .push("media_jobs".into());
        let root = tempfile::tempdir().unwrap();
        let (authority, profile) =
            authority(&fixture, &client, root.path(), CancellationToken::new()).await;
        // Freeze the already-bound authority behind a FIFO profile admission gate.
        // The authorized profile edit is queued first, before a retained request.
        let baseline = profile.generation().await.unwrap();
        let gate = profile.guard_generation(&baseline).await.unwrap();
        if change == "profile" {
            let value = baseline.profile().clone();
            let update = profile.save(value);
            tokio::pin!(update);
            assert!(matches!(
                futures_util::poll!(&mut update),
                std::task::Poll::Pending
            ));
            let read = authority.list(None);
            tokio::pin!(read);
            assert!(matches!(
                futures_util::poll!(&mut read),
                std::task::Poll::Pending
            ));
            drop(gate);
            update.await.unwrap();
            assert!(read.await.is_err());
        } else {
            let read = authority.list(None);
            tokio::pin!(read);
            assert!(matches!(
                futures_util::poll!(&mut read),
                std::task::Poll::Pending
            ));
            fixture.login(&client).await;
            drop(gate);
            assert!(read.await.is_err());
        }
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }
}
