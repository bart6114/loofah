use super::*;
use crate::media_jobs::MediaSubmission;

fn retirement(
    submission: &MediaSubmission,
    disposition: MediaSourceDisposition,
) -> MediaRetirementRequest {
    MediaRetirementRequest {
        retirement_id: uuid::Uuid::new_v4().to_string(),
        expected_revision: submission.admission_revision.clone().unwrap(),
        expected_declaration_sha256: media_declaration_sha256(&submission.declaration).unwrap(),
        expected_source: submission
            .declaration
            .operation
            .upload()
            .map(|(_, source)| source.clone()),
        disposition,
    }
}
fn status(submission: &MediaSubmission) -> MediaRetirementStatus {
    MediaRetirementStatus {
        request_id: submission.declaration.request_id.clone(),
        revision: submission.admission_revision.clone().unwrap(),
        declaration_sha256: media_declaration_sha256(&submission.declaration).unwrap(),
        source: submission
            .declaration
            .operation
            .upload()
            .map(|(_, source)| source.clone()),
        state: MediaRetirementState::Eligible,
        retirement: None,
        reason: None,
        quota_released: false,
        allowed_dispositions: vec![
            MediaSourceDisposition::Exported,
            MediaSourceDisposition::Discard,
        ],
    }
}
async fn admitted(store: &Arc<MediaSubmissions>, scope: MediaScope) -> MediaSubmission {
    let (submission, _) = store.prepare(scope, declaration()).await.unwrap();
    let revision = uuid::Uuid::new_v4().to_string();
    store
        .observe_admission(
            &submission,
            &MediaAdmission {
                declaration: submission.declaration.clone(),
                revision: revision.clone(),
                status: MediaRequestStatus {
                    declaration: submission.declaration.clone(),
                    revision,
                    phase: MediaPhase::Review,
                    media: None,
                },
            },
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn media_retirement_receipt_preserves_original_export_decision_and_only_matching_acknowledgement()
 {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap().join("receipts");
    let store = MediaSubmissions::new(directory.clone()).unwrap();
    let scope = MediaScope {
        profile_id: uuid::Uuid::new_v4().to_string(),
        server: "https://original.example/".into(),
        member_id: "member".into(),
        vault_id: "vault".into(),
    };
    let original = admitted(&store, scope.clone()).await;
    let input = retirement(&original, MediaSourceDisposition::Exported);
    assert!(
        store
            .prepare_retirement(&original, input.clone())
            .await
            .is_err()
    );
    let exported = store.observe_source_export(&original).await.unwrap();
    assert!(exported.has_verified_source_export());
    let prepared = store
        .prepare_retirement(&exported, input.clone())
        .await
        .unwrap();
    let reopened = MediaSubmissions::new(directory).unwrap();
    let retained = reopened
        .load(scope.clone(), original.declaration.request_id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.retirement, prepared.retirement);
    assert!(!retained.retirement.as_ref().unwrap().retired);
    let mut changed = input.clone();
    changed.retirement_id = uuid::Uuid::new_v4().to_string();
    assert!(
        reopened
            .prepare_retirement(&retained, changed)
            .await
            .is_err()
    );
    let mut changed = input.clone();
    changed.disposition = MediaSourceDisposition::Discard;
    assert!(
        reopened
            .prepare_retirement(&retained, changed)
            .await
            .is_err()
    );
    assert_eq!(
        reopened
            .observe_source_export(&retained)
            .await
            .unwrap()
            .retirement,
        retained.retirement
    );
    let mut reply = status(&retained);
    reply.state = MediaRetirementState::Retired;
    reply.allowed_dispositions.clear();
    reply.retirement = Some(input);
    reply.quota_released = true;
    let mut wrong = reply.clone();
    wrong.request_id = uuid::Uuid::new_v4().to_string();
    assert!(
        reopened
            .observe_retirement(&retained, &wrong)
            .await
            .is_err()
    );
    assert!(
        !reopened
            .load(scope, original.declaration.request_id.clone())
            .await
            .unwrap()
            .unwrap()
            .retirement
            .unwrap()
            .retired
    );
    assert!(
        reopened
            .observe_retirement(&retained, &reply)
            .await
            .unwrap()
            .retirement
            .unwrap()
            .retired
    );
}

#[tokio::test]
async fn media_retirement_lost_reply_keeps_uuid_and_readonly_status_never_redispatches() {
    let posts = Arc::new(AtomicUsize::new(0));
    let server = Arc::new(tokio::sync::Mutex::new(None::<MediaRetirementStatus>));
    let route = Router::new().route(
        "/api/v1/vaults/vault/media/requests/{request}/retirement",
        get({
            let server = server.clone();
            move || {
                let server = server.clone();
                async move { Json(server.lock().await.clone().unwrap()) }
            }
        })
        .post({
            let server = server.clone();
            let posts = posts.clone();
            move |headers: HeaderMap, Json(input): Json<MediaRetirementRequest>| {
                let server = server.clone();
                let posts = posts.clone();
                async move {
                    assert_eq!(headers["x-request-id"], input.retirement_id);
                    posts.fetch_add(1, Ordering::SeqCst);
                    let mut current = server.lock().await;
                    let current = current.as_mut().unwrap();
                    current.state = MediaRetirementState::Retired;
                    current.retirement = Some(input);
                    current.allowed_dispositions.clear();
                    current.quota_released = true;
                    (
                        StatusCode::SERVICE_UNAVAILABLE,
                        "retirement committed, acknowledgement unavailable",
                    )
                }
            }
        }),
    );
    let fixture = Fixture::with_extra_routes(Some(Duration::ZERO), route).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .extend([
            "media_jobs".into(),
            "media_retirement".into(),
            "media_source_export".into(),
        ]);
    let root = tempfile::tempdir().unwrap();
    let (authority, _) = authority(&fixture, &client, root.path(), CancellationToken::new()).await;
    let store =
        MediaSubmissions::new(root.path().canonicalize().unwrap().join("receipts")).unwrap();
    let original = admitted(&store, authority.scope().clone()).await;
    *server.lock().await = Some(status(&original));
    let prepared = store
        .prepare_retirement(
            &original,
            retirement(&original, MediaSourceDisposition::Discard),
        )
        .await
        .unwrap();
    let mut wrong_scope = prepared.clone();
    wrong_scope.scope.vault_id = "unrelated-vault".into();
    assert!(authority.retire(&wrong_scope).await.is_err());
    assert_eq!(posts.load(Ordering::SeqCst), 0);
    let error = authority.retire(&prepared).await.unwrap_err();
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
    let retained = store
        .load(
            authority.scope().clone(),
            original.declaration.request_id.clone(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.retirement, prepared.retirement);
    let observed = authority.retirement_status(&retained).await.unwrap();
    assert_eq!(observed.state, MediaRetirementState::Retired);
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    let replay = authority.retire(&retained).await.unwrap();
    assert_eq!(replay, observed);
    assert_eq!(posts.load(Ordering::SeqCst), 1);
    assert!(
        store
            .observe_retirement(&retained, &replay)
            .await
            .unwrap()
            .retirement
            .unwrap()
            .retired
    );
    assert!(authority.recover(&retained).await.is_err());
    assert!(
        authority
            .download_source_with_guard(&retained, tokio::io::sink(), ())
            .await
            .is_err()
    );
    assert_eq!(posts.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn media_retirement_original_profile_and_login_changes_refuse_before_dispatch() {
    for change in ["profile", "login"] {
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let fixture = Fixture::with_extra_routes(
            Some(Duration::ZERO),
            Router::new().route(
                "/api/v1/vaults/vault/media/requests/{request}/retirement",
                get(move || {
                    let seen = seen.clone();
                    async move {
                        seen.fetch_add(1, Ordering::SeqCst);
                        StatusCode::INTERNAL_SERVER_ERROR
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
            .extend(["media_jobs".into(), "media_retirement".into()]);
        let root = tempfile::tempdir().unwrap();
        let (authority, profile) =
            authority(&fixture, &client, root.path(), CancellationToken::new()).await;
        let store =
            MediaSubmissions::new(root.path().canonicalize().unwrap().join("receipts")).unwrap();
        let original = admitted(&store, authority.scope().clone()).await;
        let prepared = store
            .prepare_retirement(
                &original,
                retirement(&original, MediaSourceDisposition::Discard),
            )
            .await
            .unwrap();
        if change == "profile" {
            profile.save(profile.profile().await).await.unwrap();
        } else {
            fixture.login(&client).await;
        }
        assert!(authority.retire(&prepared).await.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            store
                .load(authority.scope().clone(), original.declaration.request_id)
                .await
                .unwrap()
                .unwrap()
                .retirement,
            prepared.retirement
        );
    }
}

#[tokio::test]
async fn media_retirement_blocked_before_post_preserves_verified_source_export() {
    let posts = Arc::new(AtomicUsize::new(0));
    let reads = Arc::new(AtomicUsize::new(0));
    let server = Arc::new(tokio::sync::Mutex::new(None::<MediaRetirementStatus>));
    let media = Arc::new(tokio::sync::Mutex::new(None::<MediaRequestStatus>));
    let route = Router::new()
        .route(
            "/api/v1/vaults/vault/media/requests/{request}/retirement",
            get({
                let server = server.clone();
                move || {
                    let server = server.clone();
                    async move { Json(server.lock().await.clone().unwrap()) }
                }
            })
            .post({
                let posts = posts.clone();
                move || {
                    posts.fetch_add(1, Ordering::SeqCst);
                    async { StatusCode::CONFLICT }
                }
            }),
        )
        .route(
            "/api/v1/vaults/vault/media/requests/{request}",
            get({
                let media = media.clone();
                move || {
                    let media = media.clone();
                    async move { Json(media.lock().await.clone().unwrap()) }
                }
            }),
        )
        .route(
            "/api/v1/vaults/vault/media/requests/{request}/source",
            get({
                let server = server.clone();
                let reads = reads.clone();
                move |headers: HeaderMap| {
                    let server = server.clone();
                    let reads = reads.clone();
                    async move {
                        reads.fetch_add(1, Ordering::SeqCst);
                        let current = server.lock().await.clone().unwrap();
                        assert_eq!(headers["x-loofah-media-revision"], current.revision);
                        if current.retirement.is_some() {
                            return (
                                StatusCode::CONFLICT,
                                Json(failure(ErrorCode::Conflict, "Original source is retiring")),
                            )
                                .into_response();
                        }
                        axum::http::Response::builder()
                            .header("content-type", "application/octet-stream")
                            .header("x-loofah-media-revision", current.revision)
                            .header("x-loofah-media-sha256", current.source.unwrap().sha256)
                            .body(axum::body::Body::from(&b"original bytes"[..]))
                            .unwrap()
                    }
                }
            }),
        );
    let fixture = Fixture::with_extra_routes(Some(Duration::ZERO), route).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .extend([
            "media_jobs".into(),
            "media_retirement".into(),
            "media_source_export".into(),
        ]);
    let root = tempfile::tempdir().unwrap();
    let (authority, _) = authority(&fixture, &client, root.path(), CancellationToken::new()).await;
    let store =
        MediaSubmissions::new(root.path().canonicalize().unwrap().join("receipts")).unwrap();
    let original = admitted(&store, authority.scope().clone()).await;
    let mut blocked = status(&original);
    blocked.state = MediaRetirementState::Blocked;
    blocked.allowed_dispositions.clear();
    blocked.reason = Some("Session has active work".into());
    *server.lock().await = Some(blocked.clone());
    *media.lock().await = Some(MediaRequestStatus {
        declaration: original.declaration.clone(),
        revision: original.admission_revision.clone().unwrap(),
        phase: MediaPhase::Succeeded,
        media: None,
    });
    let input = retirement(&original, MediaSourceDisposition::Discard);
    let pending = store
        .prepare_retirement(&original, input.clone())
        .await
        .unwrap();
    assert_eq!(
        authority.retire(&pending).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(posts.load(Ordering::SeqCst), 0);
    let (bytes, ()) = authority
        .download_source_with_guard(&pending, Vec::new(), ())
        .await
        .unwrap();
    assert_eq!(bytes, b"original bytes");
    authority
        .guard_source_export(&pending)
        .await
        .unwrap()
        .check_current()
        .unwrap();
    let exported = store.observe_source_export(&pending).await.unwrap();
    assert!(exported.has_verified_source_export());
    assert_eq!(exported.retirement, pending.retirement);
    assert!(authority.recover(&exported).await.is_err());
    assert!(authority.declare(&exported).await.is_err());
    assert_eq!(posts.load(Ordering::SeqCst), 0);
    for state in [
        MediaRetirementState::Retiring,
        MediaRetirementState::Retired,
    ] {
        let mut accepted = blocked.clone();
        accepted.state = state;
        accepted.retirement = Some(input.clone());
        accepted.quota_released = accepted.state == MediaRetirementState::Retired;
        *server.lock().await = Some(accepted);
        assert_eq!(
            authority
                .download_source_with_guard(&exported, Vec::new(), ())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
    }
    assert_eq!(reads.load(Ordering::SeqCst), 3);
    assert_eq!(posts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn media_retirement_export_observation_race_preserves_original_decision_and_ack() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().canonicalize().unwrap().join("receipts");
    let store = MediaSubmissions::new(directory.clone()).unwrap();
    let original = admitted(
        &store,
        MediaScope {
            profile_id: uuid::Uuid::new_v4().to_string(),
            server: "https://original.example/".into(),
            member_id: "member".into(),
            vault_id: "vault".into(),
        },
    )
    .await;
    let input = retirement(&original, MediaSourceDisposition::Discard);
    let pending = store
        .prepare_retirement(&original, input.clone())
        .await
        .unwrap();
    let mut completed = status(&pending);
    completed.state = MediaRetirementState::Retired;
    completed.retirement = Some(input.clone());
    completed.allowed_dispositions.clear();
    completed.quota_released = true;
    let other = MediaSubmissions::new(directory).unwrap();
    let (exported, acknowledged) = tokio::join!(
        store.observe_source_export(&original),
        other.observe_retirement(&pending, &completed),
    );
    exported.unwrap();
    acknowledged.unwrap();
    // A late publication callback carries a pre-cleanup snapshot; it must merge
    // historical export evidence without reverting the disk's acknowledgement.
    let final_receipt = store.observe_source_export(&original).await.unwrap();
    assert!(final_receipt.has_verified_source_export());
    let retained = final_receipt.retirement.as_ref().unwrap();
    assert_eq!(retained.request, input);
    assert!(retained.retired);
    let replay = other.prepare_retirement(&pending, input).await.unwrap();
    assert_eq!(replay.retirement, final_receipt.retirement);
    assert_eq!(replay.source_export, final_receipt.source_export);
}
