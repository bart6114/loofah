use super::*;
use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use tokio::io::AsyncWrite;

async fn setup(
    mode: &'static str,
) -> (
    Fixture,
    Arc<RemoteClient>,
    tempfile::TempDir,
    Arc<MediaAuthority>,
    Arc<ProfileLease>,
    crate::media_jobs::MediaSubmission,
) {
    setup_headers(mode, None).await
}

async fn setup_headers(
    mode: &'static str,
    blocked_headers: Option<(Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>)>,
) -> (
    Fixture,
    Arc<RemoteClient>,
    tempfile::TempDir,
    Arc<MediaAuthority>,
    Arc<ProfileLease>,
    crate::media_jobs::MediaSubmission,
) {
    let input = declaration();
    let request = input.request_id.clone();
    let revision = uuid::Uuid::new_v4().to_string();
    let response_revision = revision.clone();
    let sha = input.operation.upload().unwrap().1.sha256.clone();
    let status_input = input.clone();
    let status_revision = revision.clone();
    let extra = Router::new().route(
        "/api/v1/vaults/vault/media/requests/{request}/source",
        get(
            move |axum::extract::Path(id): axum::extract::Path<String>, headers: HeaderMap| {
                let revision = response_revision.clone();
                let request = request.clone();
                let sha = sha.clone();
                let blocked_headers = blocked_headers.clone();
                async move {
                    if let Some((entered, release)) = blocked_headers {
                        entered.notify_one();
                        release.notified().await;
                    }
                    assert_eq!(id, request);
                    assert_eq!(headers["x-loofah-media-revision"], revision);
                    assert_eq!(headers["accept-encoding"], "identity");
                    assert!(!headers.contains_key("range"));
                    let mut response = axum::http::Response::builder()
                        .header("content-type", "application/octet-stream")
                        .header(
                            "x-loofah-media-revision",
                            if mode == "revision" {
                                uuid::Uuid::new_v4().to_string()
                            } else {
                                revision
                            },
                        )
                        .header(
                            "x-loofah-media-sha256",
                            if mode == "digest-header" {
                                "0".repeat(64)
                            } else {
                                sha
                            },
                        );
                    if mode == "range" {
                        response = response.header("content-range", "bytes 0-13/14");
                    }
                    if mode == "encoding" {
                        response = response.header("content-encoding", "gzip");
                    }
                    let bytes: &'static [u8] = match mode {
                        "corrupt" => b"changed! bytes",
                        "short" => b"short",
                        _ => b"original bytes",
                    };
                    response.body(axum::body::Body::from(bytes)).unwrap()
                }
            },
        ),
    );
    let extra = extra.route(
        "/api/v1/vaults/vault/media/requests/{request}",
        get(move || {
            let declaration = status_input.clone();
            let revision = status_revision.clone();
            async move {
                if mode == "status-denied" {
                    return (
                        StatusCode::FORBIDDEN,
                        Json(failure(ErrorCode::Forbidden, "Editor access revoked")),
                    )
                        .into_response();
                }
                Json(MediaRequestStatus {
                    declaration,
                    revision,
                    phase: MediaPhase::Review,
                    media: None,
                })
                .into_response()
            }
        }),
    );
    let fixture = Fixture::with_extra_routes(Some(Duration::ZERO), extra).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .extend(["media_jobs".into(), "media_source_export".into()]);
    let root = tempfile::tempdir().unwrap();
    let path = root.path().canonicalize().unwrap();
    let (bound, profile) = authority(&fixture, &client, &path, CancellationToken::new()).await;
    let receipts = MediaSubmissions::new(path.join("receipts")).unwrap();
    let (submission, _) = receipts
        .prepare(bound.scope().clone(), input.clone())
        .await
        .unwrap();
    let submission = receipts
        .observe_admission(
            &submission,
            &MediaAdmission {
                declaration: input.clone(),
                revision: revision.clone(),
                status: MediaRequestStatus {
                    declaration: input,
                    revision,
                    phase: MediaPhase::Review,
                    media: None,
                },
            },
        )
        .await
        .unwrap();
    (fixture, client, root, bound, profile, submission)
}

#[tokio::test]
async fn original_source_export_verifies_descriptor_and_actual_bytes() {
    for mode in [
        "valid",
        "revision",
        "digest-header",
        "range",
        "encoding",
        "corrupt",
        "short",
    ] {
        let (_fixture, _client, _root, authority, _profile, submission) = setup(mode).await;
        let result = authority
            .download_source_with_guard(&submission, Vec::new(), ())
            .await;
        if mode == "valid" {
            assert_eq!(result.unwrap().0, b"original bytes");
            authority
                .guard_source_export(&submission)
                .await
                .unwrap()
                .check_current()
                .unwrap();
        } else {
            assert!(result.is_err(), "accepted {mode}");
        }
    }
}

#[tokio::test]
async fn source_export_capability_precedes_credentials_and_final_fence_rejects_login_aba() {
    let fixture = Fixture::with_tls(Some(Duration::ZERO)).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("media_jobs".into());
    let loads = fixture.credentials.loads.load(Ordering::SeqCst);
    assert_eq!(
        client
            .media_source_export_capability(&CancellationToken::new())
            .unwrap_err()
            .code,
        ErrorCode::IncompatibleVersion
    );
    assert_eq!(fixture.credentials.loads.load(Ordering::SeqCst), loads);
    let (fixture, client, _root, authority, _profile, submission) = setup("valid").await;
    authority
        .download_source_with_guard(&submission, Vec::new(), ())
        .await
        .unwrap();
    fixture.login(&client).await;
    assert_eq!(
        authority
            .guard_source_export(&submission)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Conflict
    );
}

#[tokio::test]
async fn final_source_export_fence_rejects_profile_aba() {
    let (_fixture, _client, _root, authority, profile, submission) = setup("valid").await;
    authority
        .download_source_with_guard(&submission, Vec::new(), ())
        .await
        .unwrap();
    let original = profile.profile().await;
    let mut changed = original.clone();
    changed.vault_id = Some("other".into());
    profile.save(changed).await.unwrap();
    profile.save(original).await.unwrap();
    assert_eq!(
        authority
            .guard_source_export(&submission)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Conflict
    );
}

struct HeldFlush {
    entered: Arc<tokio::sync::Notify>,
    release: tokio::sync::oneshot::Receiver<()>,
    dropped: Arc<AtomicBool>,
}
impl AsyncWrite for HeldFlush {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        value: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Poll::Ready(Ok(value.len()))
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        self.entered.notify_one();
        Pin::new(&mut self.release).poll(cx).map(|_| Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}
impl Drop for HeldFlush {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}
#[tokio::test]
async fn aborted_source_download_keeps_private_owner_and_capacity_until_actual_flush() {
    let (_fixture, client, _root, authority, _profile, submission) = setup("valid").await;
    let owner = Arc::new(tokio::sync::Semaphore::new(1));
    let permit = owner.clone().acquire_owned().await.unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let dropped = Arc::new(AtomicBool::new(false));
    let (release, blocked) = tokio::sync::oneshot::channel();
    let sink = HeldFlush {
        entered: entered.clone(),
        release: blocked,
        dropped: dropped.clone(),
    };
    let task = tokio::spawn(async move {
        authority
            .download_source_with_guard(&submission, sink, permit)
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    task.abort();
    assert!(task.await.err().unwrap().is_cancelled());
    assert!(!dropped.load(Ordering::SeqCst));
    assert!(owner.clone().try_acquire_owned().is_err());
    assert_eq!(client.media_source_readers.available_permits(), 1);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if dropped.load(Ordering::SeqCst)
                && owner.available_permits() == 1
                && client.media_source_readers.available_permits() == 2
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn source_header_verification_does_not_hold_profile_edit_locks() {
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (_fixture, _client, _root, authority, profile, submission) =
        setup_headers("valid", Some((entered.clone(), release.clone()))).await;
    let task = tokio::spawn(async move {
        authority
            .download_source_with_guard(&submission, Vec::new(), ())
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .unwrap();
    let mut changed = profile.profile().await;
    changed.vault_id = Some("other".into());
    let saved = tokio::time::timeout(Duration::from_secs(1), profile.save(changed)).await;
    release.notify_one();
    let result = task.await.unwrap();
    saved
        .expect("server verification must not block profile edits")
        .unwrap();
    assert_eq!(result.unwrap_err().code, ErrorCode::Conflict);
}

#[tokio::test]
async fn final_source_export_fence_requires_current_original_vault_editor_access() {
    let (_fixture, _client, _root, authority, _profile, submission) = setup("status-denied").await;
    let (bytes, _) = authority
        .download_source_with_guard(&submission, Vec::new(), ())
        .await
        .unwrap();
    assert_eq!(bytes, b"original bytes");
    assert_eq!(
        authority
            .guard_source_export(&submission)
            .await
            .err()
            .unwrap()
            .code,
        ErrorCode::Forbidden
    );
}
