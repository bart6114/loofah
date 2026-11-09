use super::*;
use axum::extract::Path;

fn metadata(name: &str) -> FileMetadata {
    FileMetadata {
        filename: name.into(),
        content_type: "application/octet-stream".into(),
        size_bytes: 10,
        sha256: "0".repeat(64),
    }
}
fn lease(id: &str, name: &str) -> AttachmentLease {
    AttachmentLease {
        id: id.into(),
        vault_id: "test".into(),
        session_id: "note".into(),
        attachment_id: name.into(),
        metadata: metadata(name),
        idle_timeout_secs: 600,
    }
}
fn authenticated(headers: &HeaderMap) {
    assert!(
        headers["authorization"]
            .to_str()
            .unwrap()
            .starts_with("Bearer access-")
    );
    assert_eq!(headers["x-loofah-api-major"], "1");
    assert!(uuid::Uuid::parse_str(headers["x-request-id"].to_str().unwrap()).is_ok());
}

pub(super) async fn read_metadata(
    State(state): State<Arc<Provider>>,
    Path((vault, session, attachment)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> axum::response::Response {
    authenticated(&headers);
    assert_eq!((vault.as_str(), session.as_str()), ("test", "note"));
    let mut value = serde_json::to_value(metadata(&attachment)).unwrap();
    match state.attachment_mode.load(Ordering::SeqCst) {
        1 => value["filename"] = json!("wrong.bin"),
        2 => value["sha256"] = json!("A".repeat(64)),
        3 => value["contentType"] = json!("image/png\r\ninjected: true"),
        10 => value = Value::Null,
        _ => {}
    }
    Json(value).into_response()
}

pub(super) async fn remove(
    State(state): State<Arc<Provider>>,
    Path((vault, session, attachment)): Path<(String, String, String)>,
    axum::extract::Query(input): axum::extract::Query<
        hypr_loofah_protocol::RemoveAttachmentRequest,
    >,
    headers: HeaderMap,
) -> axum::response::Response {
    authenticated(&headers);
    assert_eq!((vault.as_str(), session.as_str()), ("test", "note"));
    assert_eq!(attachment, "chart + # ü.pdf");
    assert_eq!(input.expected_sha256, "0".repeat(64));
    state
        .attachment_removals
        .lock()
        .await
        .push(headers["x-request-id"].to_str().unwrap().into());
    if state.attachment_mode.load(Ordering::SeqCst) == 21 {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    Json(true).into_response()
}

pub(super) async fn info(
    State(state): State<Arc<Provider>>,
    Path((vault, session, attachment)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> axum::response::Response {
    authenticated(&headers);
    assert_eq!(vault, "test");
    assert_eq!(session, "note");
    let mode = state.attachment_mode.load(Ordering::SeqCst);
    if mode == 10 {
        return Json(Value::Null).into_response();
    }
    if mode == 11 {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    let mut value =
        json!({"filename":attachment,"contentType":"application/octet-stream","sizeBytes":10});
    match mode {
        1 => value["filename"] = json!("different.bin"),
        3 => value["contentType"] = json!("image/png\r\nx-injected: true"),
        12 => value["padding"] = json!("a".repeat(20 * 1024)),
        13 => value["sizeBytes"] = json!(-1),
        14 => value["contentType"] = json!("a".repeat(257)),
        _ => {}
    }
    Json(value).into_response()
}

pub(super) async fn open(
    State(state): State<Arc<Provider>>,
    Path((vault, session, attachment, id)): Path<(String, String, String, String)>,
    headers: HeaderMap,
) -> axum::response::Response {
    authenticated(&headers);
    assert_eq!(vault, "test");
    assert_eq!(session, "note");
    let mode = state.attachment_mode.load(Ordering::SeqCst);
    if mode >= 30 {
        return super::downloads::open(state, &id, &attachment);
    }
    if mode == 10 {
        return Json(Value::Null).into_response();
    }
    if mode == 11 {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    let mut value = serde_json::to_value(lease(&id, &attachment)).unwrap();
    match mode {
        1 => value["metadata"]["filename"] = json!("another.bin"),
        2 => value["attachment_id"] = json!("another.bin"),
        3 => value["metadata"]["contentType"] = json!("application/bin\r\nx-injected: true"),
        4 => value["metadata"]["sha256"] = json!("A".repeat(64)),
        5 => value["idle_timeout_secs"] = json!(0),
        6 => value["vault_id"] = json!("other-vault"),
        7 => value["id"] = json!(uuid::Uuid::new_v4()),
        8 => value["padding"] = json!("x".repeat(256 * 1024)),
        _ => {}
    }
    Json(value).into_response()
}
pub(super) async fn head(
    State(state): State<Arc<Provider>>,
    headers: HeaderMap,
) -> axum::response::Response {
    authenticated(&headers);
    let mode = state.attachment_mode.load(Ordering::SeqCst);
    axum::response::Response::builder()
        .status(if mode == 20 {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::OK
        })
        .header("etag", format!("\"sha256:{}\"", "0".repeat(64)))
        .header("content-type", "application/octet-stream")
        .header("content-length", "10")
        .body(axum::body::Body::empty())
        .unwrap()
}
pub(super) async fn read(
    State(state): State<Arc<Provider>>,
    headers: HeaderMap,
) -> axum::response::Response {
    authenticated(&headers);
    if state.attachment_mode.load(Ordering::SeqCst) >= 30 {
        return super::downloads::read(state, headers).await;
    }
    assert_eq!(headers["range"], "bytes=2-4");
    assert_eq!(headers["accept-encoding"], "identity");
    assert_eq!(
        headers["if-match"],
        format!("\"sha256:{}\"", "0".repeat(64))
    );
    let mode = state.attachment_mode.load(Ordering::SeqCst);
    if mode == 11 {
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    let mut response = axum::response::Response::builder()
        .status(StatusCode::PARTIAL_CONTENT)
        .header("etag", format!("\"sha256:{}\"", "0".repeat(64)))
        .header("content-range", "bytes 2-4/10")
        .header("content-length", "3")
        .header("content-type", "application/octet-stream")
        .body(axum::body::Body::from("234"))
        .unwrap();
    match mode {
        21 => {
            response
                .headers_mut()
                .insert("content-range", "bytes 3-5/10".parse().unwrap());
        }
        22 => {
            response
                .headers_mut()
                .insert("etag", "wrong".parse().unwrap());
        }
        23 => {
            response
                .headers_mut()
                .insert("content-type", "image/png".parse().unwrap());
        }
        24 => {
            *response.status_mut() = StatusCode::OK;
        }
        25 => {
            *response.body_mut() =
                axum::body::Body::from_stream(futures_util::stream::iter([Ok::<
                    _,
                    std::convert::Infallible,
                >(
                    b"234".to_vec()
                )]));
            response.headers_mut().remove("content-length");
        }
        26 => {
            *response.status_mut() = StatusCode::PRECONDITION_FAILED;
        }
        _ => {}
    }
    response
}
pub(super) async fn close(headers: HeaderMap) -> StatusCode {
    authenticated(&headers);
    StatusCode::NO_CONTENT
}

async fn capable(fixture: &Fixture) -> Arc<RemoteClient> {
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .extend([
            "attachment_leases".into(),
            "attachment_info".into(),
            "attachment_metadata".into(),
            "attachment_remove".into(),
        ]);
    client
}

#[tokio::test]
async fn typed_attachment_lifecycle_handles_escaped_names_and_remote_windows_aliases_over_https() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = capable(&fixture).await;
    fixture.login(&client).await;
    for name in ["chart + # ü.bin", "CON.txt"] {
        let id = uuid::Uuid::new_v4().to_string();
        let opened = client
            .open_attachment("test", "note", name, &id, CancellationToken::new())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(opened, lease(&id, name));
        assert_eq!(
            client
                .open_attachment("test", "note", name, &id, CancellationToken::new())
                .await
                .unwrap(),
            Some(opened.clone())
        );
        client
            .head_attachment(&opened, CancellationToken::new())
            .await
            .unwrap();
        assert_eq!(
            client
                .read_attachment_chunk(&opened, 2, 3, CancellationToken::new())
                .await
                .unwrap(),
            b"234"
        );
        client
            .close_attachment("test", &id, CancellationToken::new())
            .await
            .unwrap();
    }
    fixture.state.attachment_mode.store(10, Ordering::SeqCst);
    assert!(
        client
            .open_attachment(
                "test",
                "note",
                "missing.bin",
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn attachment_capability_and_portable_validation_run_before_credentials() {
    let fixture = Fixture::new().await;
    let client = fixture.client().await;
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        client
            .open_attachment("test", "note", "valid.bin", &id, CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::IncompatibleVersion
    );
    assert_eq!(
        client
            .close_attachment("test", &id, CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::IncompatibleVersion
    );
    let client = capable(&fixture).await;
    for name in ["", ".private", "../note.md", "nested/a", "a\\b", "a\0b"] {
        assert_eq!(
            client
                .open_attachment("test", "note", name, &id, CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
    let opened = lease(&id, "valid.bin");
    for (offset, length) in [
        (0, 0),
        (10, 1),
        (0, RECORDING_CHUNK_LIMIT + 1),
        (u64::MAX, 1),
    ] {
        assert_eq!(
            client
                .read_attachment_chunk(&opened, offset, length, CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.refreshes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn attachment_responses_enforce_identity_headers_and_bounded_representation() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = capable(&fixture).await;
    fixture.login(&client).await;
    let id = uuid::Uuid::new_v4().to_string();
    for mode in 1..=8 {
        fixture.state.attachment_mode.store(mode, Ordering::SeqCst);
        let expected = if mode == 8 {
            ErrorCode::ResourceLimit
        } else {
            ErrorCode::IncompatibleVersion
        };
        assert_eq!(
            client
                .open_attachment("test", "note", "valid.bin", &id, CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            expected,
            "mode {mode}"
        );
    }
    let opened = lease(&id, "valid.bin");
    for mode in 21..=25 {
        fixture.state.attachment_mode.store(mode, Ordering::SeqCst);
        assert_eq!(
            client
                .read_attachment_chunk(&opened, 2, 3, CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            ErrorCode::IncompatibleVersion,
            "mode {mode}"
        );
    }
    fixture.state.attachment_mode.store(26, Ordering::SeqCst);
    assert_eq!(
        client
            .read_attachment_chunk(&opened, 2, 3, CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    fixture.state.attachment_mode.store(20, Ordering::SeqCst);
    assert_eq!(
        client
            .head_attachment(&opened, CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn attachment_open_and_chunk_cancellation_do_not_wait_for_the_network_timeout() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(150))).await;
    let client = capable(&fixture).await;
    fixture.login(&client).await;
    fixture.state.attachment_mode.store(11, Ordering::SeqCst);
    let id = uuid::Uuid::new_v4().to_string();
    let opened = lease(&id, "valid.bin");
    for open in [true, false] {
        let cancel = CancellationToken::new();
        let caller = client.clone();
        let cancelled = cancel.clone();
        let data = opened.clone();
        let started = std::time::Instant::now();
        let request = tokio::spawn(async move {
            if open {
                caller
                    .open_attachment("test", "note", "valid.bin", &data.id, cancelled)
                    .await
                    .map(|_| ())
            } else {
                caller
                    .read_attachment_chunk(&data, 2, 3, cancelled)
                    .await
                    .map(|_| ())
            }
        });
        tokio::time::sleep(Duration::from_millis(200)).await;
        cancel.cancel();
        assert_eq!(
            request.await.unwrap().unwrap_err().code,
            ErrorCode::Cancelled
        );
        assert!(started.elapsed() < Duration::from_secs(1));
    }
}

fn info_operation(name: &str) -> RemoteOperation {
    RemoteOperation::AttachmentInfo {
        vault_id: "test".into(),
        session_id: "note".into(),
        attachment_id: name.into(),
    }
}

#[tokio::test]
async fn attachment_information_requires_capability_and_valid_names_before_credentials() {
    let fixture = Fixture::new().await;
    let request = uuid::Uuid::new_v4().to_string();
    let unsupported = fixture.client().await;
    let cause = unsupported
        .execute(
            info_operation("valid.bin"),
            &request,
            CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(cause.code, ErrorCode::IncompatibleVersion);
    assert_eq!(cause.request_id.as_deref(), Some(request.as_str()));
    let client = capable(&fixture).await;
    for name in ["", ".private", "../note.md", "nested/a", "a\\b", "a\0b"] {
        assert_eq!(
            client
                .execute(info_operation(name), &request, CancellationToken::new())
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        client
            .execute(info_operation("valid.bin"), &request, cancel)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
    assert!(fixture.credentials.values.lock().await.is_empty());
}

#[tokio::test]
async fn attachment_mutations_and_metadata_validate_capabilities_and_inputs_before_credentials() {
    let fixture = Fixture::new().await;
    let unsupported = fixture.client().await;
    let request = uuid::Uuid::new_v4().to_string();
    let metadata = |name: &str| RemoteOperation::AttachmentMetadata {
        vault_id: "test".into(),
        session_id: "note".into(),
        attachment_id: name.into(),
    };
    let remove = |name: &str, hash: &str| RemoteOperation::RemoveAttachment {
        vault_id: "test".into(),
        session_id: "note".into(),
        attachment_id: name.into(),
        input: hypr_loofah_protocol::RemoveAttachmentRequest {
            expected_sha256: hash.into(),
        },
    };
    for operation in [metadata("file.pdf"), remove("file.pdf", &"0".repeat(64))] {
        let cause = unsupported
            .execute(operation, &request, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(cause.code, ErrorCode::IncompatibleVersion);
        assert_eq!(cause.request_id.as_deref(), Some(request.as_str()));
    }
    let client = capable(&fixture).await;
    for operation in [
        metadata("../private"),
        remove(".private", &"0".repeat(64)),
        remove("file.pdf", ""),
        remove("file.pdf", &"A".repeat(64)),
    ] {
        let cause = client
            .execute(operation, &request, CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(cause.code, ErrorCode::Validation);
        assert_eq!(cause.request_id.as_deref(), Some(request.as_str()));
    }
    assert_eq!(fixture.state.issued.load(Ordering::SeqCst), 0);
    assert!(fixture.credentials.values.lock().await.is_empty());
    assert!(fixture.state.attachment_removals.lock().await.is_empty());
}

#[tokio::test]
async fn checked_attachment_requests_validate_https_metadata_and_never_repeat_a_cancelled_mutation()
{
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = capable(&fixture).await;
    fixture.login(&client).await;
    let name = "chart + # ü.pdf";
    for mode in [0, 1, 2, 3, 10] {
        fixture.state.attachment_mode.store(mode, Ordering::SeqCst);
        let result = client
            .execute(
                RemoteOperation::AttachmentMetadata {
                    vault_id: "test".into(),
                    session_id: "note".into(),
                    attachment_id: name.into(),
                },
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new(),
            )
            .await;
        match mode {
            0 => assert!(
                matches!(result.unwrap(), RemoteReply::AttachmentMetadata(Some(value)) if value == metadata(name))
            ),
            10 => assert!(matches!(
                result.unwrap(),
                RemoteReply::AttachmentMetadata(None)
            )),
            _ => assert_eq!(result.unwrap_err().code, ErrorCode::IncompatibleVersion),
        }
    }
    let operation = RemoteOperation::RemoveAttachment {
        vault_id: "test".into(),
        session_id: "note".into(),
        attachment_id: name.into(),
        input: hypr_loofah_protocol::RemoveAttachmentRequest {
            expected_sha256: "0".repeat(64),
        },
    };
    let id = uuid::Uuid::new_v4().to_string();
    assert!(matches!(
        client
            .execute(operation.clone(), &id, CancellationToken::new())
            .await
            .unwrap(),
        RemoteReply::AttachmentRemoved(true)
    ));
    assert_eq!(*fixture.state.attachment_removals.lock().await, vec![id]);
    fixture.state.attachment_mode.store(21, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let id = uuid::Uuid::new_v4().to_string();
    let pending = tokio::spawn({
        let client = client.clone();
        let cancel = cancel.clone();
        let id = id.clone();
        async move { client.execute(operation, &id, cancel).await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.state.attachment_removals.lock().await.len() < 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    let cause = pending.await.unwrap().unwrap_err();
    assert_eq!(
        cause.mutation_outcome,
        Some(hypr_loofah_protocol::MutationOutcome::Unknown)
    );
    assert_eq!(cause.request_id.as_deref(), Some(id.as_str()));
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(fixture.state.attachment_removals.lock().await.len(), 2);
}

#[tokio::test]
async fn attachment_information_validates_https_replies_and_preserves_opaque_names() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(50))).await;
    let client = capable(&fixture).await;
    fixture.login(&client).await;
    let request = uuid::Uuid::new_v4().to_string();
    for name in ["chart + # ü.png", "CON.txt"] {
        let RemoteReply::AttachmentInfo(Some(info)) = client
            .execute(info_operation(name), &request, CancellationToken::new())
            .await
            .unwrap()
        else {
            panic!("missing information")
        };
        assert_eq!(info.filename, name);
        assert_eq!(info.size_bytes, 10);
        assert_eq!(
            serde_json::to_value(info)
                .unwrap()
                .as_object()
                .unwrap()
                .len(),
            3
        );
    }
    fixture.state.attachment_mode.store(10, Ordering::SeqCst);
    assert!(matches!(
        client
            .execute(
                info_operation("missing.bin"),
                &request,
                CancellationToken::new()
            )
            .await
            .unwrap(),
        RemoteReply::AttachmentInfo(None)
    ));
    for mode in [1, 3, 12, 13, 14] {
        fixture.state.attachment_mode.store(mode, Ordering::SeqCst);
        let cause = client
            .execute(
                info_operation("valid.bin"),
                &request,
                CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            cause.code,
            if mode == 12 {
                ErrorCode::ResourceLimit
            } else {
                ErrorCode::IncompatibleVersion
            },
            "mode {mode}"
        );
    }
}

#[tokio::test]
async fn attachment_information_cancellation_interrupts_a_pending_https_reply() {
    let fixture = Fixture::with_tls(Some(Duration::from_millis(150))).await;
    let client = capable(&fixture).await;
    fixture.login(&client).await;
    fixture.state.attachment_mode.store(11, Ordering::SeqCst);
    let cancel = CancellationToken::new();
    let stopped = cancel.clone();
    let started = std::time::Instant::now();
    let request = tokio::spawn(async move {
        client
            .execute(
                info_operation("valid.bin"),
                &uuid::Uuid::new_v4().to_string(),
                stopped,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    cancel.cancel();
    assert_eq!(
        request.await.unwrap().unwrap_err().code,
        ErrorCode::Cancelled
    );
    assert!(started.elapsed() < Duration::from_secs(1));
}
