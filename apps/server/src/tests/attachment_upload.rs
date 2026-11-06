use super::*;
use axum::body::Bytes;
use sha2::{Digest, Sha256};
use std::time::Duration;

fn input(bytes: &[u8]) -> UploadAttachmentRequest {
    UploadAttachmentRequest {
        filename: "chart + ü.pdf".into(),
        size_bytes: bytes.len() as u64,
        sha256: Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    }
}
fn query(input: &UploadAttachmentRequest) -> String {
    let mut url = url::Url::parse("https://example.invalid").unwrap();
    url.query_pairs_mut()
        .append_pair("filename", &input.filename)
        .append_pair("size_bytes", &input.size_bytes.to_string())
        .append_pair("sha256", &input.sha256);
    url.query().unwrap().to_owned()
}
fn upload_request(
    f: &Fixture,
    who: &str,
    vault: &str,
    session: &str,
    id: &str,
    input: &UploadAttachmentRequest,
    body: Body,
) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(format!(
            "/api/v1/vaults/{vault}/sessions/{session}/attachments?{}",
            query(input)
        ))
        .header("authorization", format!("Bearer {}", f.token(who)))
        .header("x-loofah-api-major", "1")
        .header("x-request-id", id)
        .header("content-type", "application/octet-stream")
        .header("content-length", input.size_bytes.to_string())
        .body(body)
        .unwrap()
}
async fn response(response: axum::response::Response) -> (StatusCode, Value) {
    let status = response.status();
    assert!(response.headers().get("x-request-id").is_some());
    let bytes = to_bytes(response.into_body(), 256 * 1024).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap())
}
async fn session(f: &Fixture, who: &str, vault: &str) -> String {
    let (status, created) = f
        .call(
            who,
            "POST",
            &format!("/api/v1/vaults/{vault}/sessions"),
            Some(json!({"title":"Uploads","markdown":"Canonical note"})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    created["meta"]["id"].as_str().unwrap().to_owned()
}
async fn settled(f: &Fixture) {
    tokio::time::timeout(Duration::from_secs(3), async {
        while f.service.attachment_uploads.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn uploads_and_status_enforce_roles_member_namespaces_vault_isolation_and_safe_replay() {
    let f = Fixture::new().await;
    let alice = f.login("alice").await;
    let bob = f.login("bob").await;
    f.login("charlie").await;
    let (_, shared) = f
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Uploads"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    let member = format!("/api/v1/vaults/{vault}/members/{}", bob.member.id);
    let (_, shared) = f
        .call(
            "alice",
            "PUT",
            &member,
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    let session = session(&f, "alice", vault).await;
    let request = input(b"payload");
    let id = uuid::Uuid::new_v4().to_string();
    let status_path = format!(
        "/api/v1/vaults/{vault}/sessions/{session}/attachment-uploads/{id}?{}",
        query(&request)
    );
    assert_eq!(
        f.call("alice", "GET", &status_path, None).await.1["kind"],
        "missing"
    );
    assert_eq!(
        response(
            f.app
                .clone()
                .oneshot(upload_request(
                    &f,
                    "bob",
                    vault,
                    &session,
                    &id,
                    &request,
                    Body::from("payload")
                ))
                .await
                .unwrap()
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, saved) = response(
        f.app
            .clone()
            .oneshot(upload_request(
                &f,
                "alice",
                vault,
                &session,
                &id,
                &request,
                Body::from("payload"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["filename"], request.filename);
    assert_eq!(saved["sha256"], request.sha256);
    assert_eq!(saved.as_object().unwrap().len(), 4);
    assert_eq!(
        f.call("alice", "GET", &status_path, None).await.1,
        json!({"kind":"committed","value":saved})
    );
    assert_eq!(
        f.call("bob", "GET", &status_path, None).await.1["kind"],
        "missing"
    );
    assert_eq!(
        f.call("charlie", "GET", &status_path, None).await.0,
        StatusCode::NOT_FOUND
    );
    let private = format!(
        "/api/v1/vaults/{}/sessions/{session}/attachment-uploads/{id}?{}",
        alice.personal_vault_id,
        query(&request)
    );
    assert_eq!(
        f.call("alice", "GET", &private, None).await.1["kind"],
        "missing"
    );
    assert_eq!(
        f.call(
            "alice",
            "PUT",
            &member,
            Some(json!({"role":"editor","expected_revision":shared["revision"]}))
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, second) = response(
        f.app
            .clone()
            .oneshot(upload_request(
                &f,
                "bob",
                vault,
                &session,
                &id,
                &request,
                Body::from("payload"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(second["filename"], saved["filename"]);
    let context = f.service.vault(vault).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(&session);
    std::fs::write(directory.join("unknown-user.pdf"), b"Preserve unknown").unwrap();
    let target = directory
        .join("attachments")
        .join(saved["filename"].as_str().unwrap());
    std::fs::remove_file(&target).unwrap();
    let (status, replay) = response(
        f.app
            .clone()
            .oneshot(upload_request(
                &f,
                "alice",
                vault,
                &session,
                &id,
                &request,
                Body::from("payload"),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!((status, replay), (StatusCode::OK, saved));
    assert!(!target.exists());
    assert_eq!(
        std::fs::read_dir(directory.join("attachments"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read(directory.join("notes.md")).unwrap(),
        b"Canonical note"
    );
    assert_eq!(
        std::fs::read(directory.join("unknown-user.pdf")).unwrap(),
        b"Preserve unknown"
    );
    f.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn wrong_checksums_lengths_paths_and_oversized_declarations_never_publish_partial_content() {
    let f = Fixture::new().await;
    let alice = f.login("alice").await;
    let vault = &alice.personal_vault_id;
    let session = session(&f, "alice", vault).await;
    let original = input(b"payload");
    for mode in ["hash", "short", "long", "path", "maximum"] {
        let mut input = original.clone();
        let mut data = b"payload".to_vec();
        match mode {
            "hash" => input.sha256 = "0".repeat(64),
            "short" => data.truncate(3),
            "long" => data.extend_from_slice(b"extra"),
            "path" => input.filename = "../notes.md".into(),
            _ => input.size_bytes = crate::uploads::MAX_ATTACHMENT_BYTES + 1,
        }
        let req = upload_request(
            &f,
            "alice",
            vault,
            &session,
            &uuid::Uuid::new_v4().to_string(),
            &input,
            Body::from(data),
        );
        let (status, error) = response(f.app.clone().oneshot(req).await.unwrap()).await;
        assert_eq!(
            status,
            match mode {
                "hash" => StatusCode::CONFLICT,
                "maximum" => StatusCode::TOO_MANY_REQUESTS,
                _ => StatusCode::BAD_REQUEST,
            },
            "{mode}: {error}"
        );
        assert_eq!(error["mutation_outcome"], "not_committed");
        settled(&f).await;
    }
    let context = f.service.vault(vault).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(&session);
    if directory.join("attachments").exists() {
        assert!(
            std::fs::read_dir(directory.join("attachments"))
                .unwrap()
                .next()
                .is_none()
        );
    }
    assert_eq!(
        std::fs::read(directory.join("notes.md")).unwrap(),
        b"Canonical note"
    );
    f.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn binary_uploads_larger_than_the_json_body_limit_stream_successfully() {
    let f = Fixture::new().await;
    let alice = f.login("alice").await;
    let vault = &alice.personal_vault_id;
    let session = session(&f, "alice", vault).await;
    let data = vec![42_u8; 6 * 1024 * 1024];
    let input = input(&data);
    let (status, metadata) = response(
        f.app
            .clone()
            .oneshot(upload_request(
                &f,
                "alice",
                vault,
                &session,
                &uuid::Uuid::new_v4().to_string(),
                &input,
                Body::from(data),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(metadata["sha256"], input.sha256);
    assert_eq!(metadata["sizeBytes"], input.size_bytes);
    let context = f.service.vault(vault).await.unwrap();
    assert_eq!(
        std::fs::metadata(
            context
                .store
                .vault_base()
                .join("sessions")
                .join(session)
                .join("attachments")
                .join(input.filename)
        )
        .unwrap()
        .len(),
        input.size_bytes
    );
    f.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn downgrade_and_revocation_stop_receiving_without_blocking_notes_or_retaining_catalog_grants()
 {
    for downgrade in [true, false] {
        let f = Fixture::new().await;
        f.login("alice").await;
        let bob = f.login("bob").await;
        let (_, shared) = f
            .call(
                "alice",
                "POST",
                "/api/v1/vaults",
                Some(json!({"name":"Uploads"})),
            )
            .await;
        let vault = shared["id"].as_str().unwrap();
        let member = format!("/api/v1/vaults/{vault}/members/{}", bob.member.id);
        let (_, shared) = f
            .call(
                "alice",
                "PUT",
                &member,
                Some(json!({"role":"editor","expected_revision":shared["revision"]})),
            )
            .await;
        let session = session(&f, "alice", vault).await;
        let (send, receive) = tokio::sync::mpsc::channel::<Bytes>(2);
        let body = Body::from_stream(futures_util::stream::unfold(
            receive,
            |mut receive| async move {
                receive
                    .recv()
                    .await
                    .map(|bytes| (Ok::<_, std::io::Error>(bytes), receive))
            },
        ));
        let req = upload_request(
            &f,
            "bob",
            vault,
            &session,
            &uuid::Uuid::new_v4().to_string(),
            &input(&[b'x'; 100]),
            body,
        );
        let app = f.app.clone();
        let task = tokio::spawn(async move { app.oneshot(req).await.unwrap() });
        send.send(Bytes::from_static(b"xxx")).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while f.service.attachment_uploads.available_permits() == 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            f.call(
                "alice",
                "GET",
                &format!("/api/v1/vaults/{vault}/sessions/{session}/note"),
                None
            )
            .await
            .0,
            StatusCode::OK
        );
        let update = tokio::time::timeout(Duration::from_secs(2), async {
            if downgrade {
                f.call(
                    "alice",
                    "PUT",
                    &member,
                    Some(json!({"role":"reader","expected_revision":shared["revision"]})),
                )
                .await
            } else {
                f.call(
                    "alice",
                    "DELETE",
                    &format!(
                        "{member}?expected_revision={}",
                        shared["revision"].as_str().unwrap()
                    ),
                    None,
                )
                .await
            }
        })
        .await
        .unwrap();
        assert_eq!(update.0, StatusCode::OK);
        let (status, _) = response(
            tokio::time::timeout(Duration::from_secs(2), task)
                .await
                .unwrap()
                .unwrap(),
        )
        .await;
        assert_eq!(
            status,
            if downgrade {
                StatusCode::FORBIDDEN
            } else {
                StatusCode::NOT_FOUND
            }
        );
        drop(send);
        settled(&f).await;
        let context = f.service.vault(vault).await.unwrap();
        let dir = context
            .store
            .vault_base()
            .join("sessions")
            .join(&session)
            .join("attachments");
        if dir.exists() {
            assert!(std::fs::read_dir(dir).unwrap().next().is_none());
        }
        assert!(context.store.freeze_for_vault_move().await.is_ok());
        assert_eq!(
            f.call("bob", "GET", "/api/v1/me", None).await.0,
            StatusCode::OK
        );
        f.service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn dropped_request_futures_release_transfer_admission_only_after_real_cleanup() {
    let f = Fixture::new().await;
    let alice = f.login("alice").await;
    let vault = &alice.personal_vault_id;
    let session = session(&f, "alice", vault).await;
    let (send, receive) = tokio::sync::mpsc::channel::<Bytes>(2);
    let body = Body::from_stream(futures_util::stream::unfold(
        receive,
        |mut receive| async move {
            receive
                .recv()
                .await
                .map(|bytes| (Ok::<_, std::io::Error>(bytes), receive))
        },
    ));
    let request = upload_request(
        &f,
        "alice",
        vault,
        &session,
        &uuid::Uuid::new_v4().to_string(),
        &input(&[b'x'; 100]),
        body,
    );
    let app = f.app.clone();
    let task = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    send.send(Bytes::from_static(b"xxx")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while f.service.attachment_uploads.available_permits() == 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    drop(send);
    settled(&f).await;
    let context = f.service.vault(vault).await.unwrap();
    let dir = context
        .store
        .vault_base()
        .join("sessions")
        .join(session)
        .join("attachments");
    if dir.exists() {
        assert!(std::fs::read_dir(dir).unwrap().next().is_none());
    }
    assert!(context.store.freeze_for_vault_move().await.is_ok());
    f.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn lost_tcp_acknowledgements_reconcile_without_publishing_a_second_attachment() {
    let f = Fixture::new().await;
    let alice = f.login("alice").await;
    let vault = &alice.personal_vault_id;
    let session = session(&f, "alice", vault).await;
    let committed = std::sync::Arc::new(tokio::sync::Notify::new());
    let release = tokio_util::sync::CancellationToken::new();
    let signal = committed.clone();
    let gate = release.clone();
    let app = f.app.clone().layer(axum::middleware::from_fn(
        move |request: Request<Body>, next: axum::middleware::Next| {
            let signal = signal.clone();
            let gate = gate.clone();
            async move {
                let upload = request.method() == axum::http::Method::POST
                    && request.uri().path().ends_with("/attachments");
                let response = next.run(request).await;
                if upload && response.status().is_success() {
                    signal.notify_one();
                    gate.cancelled().await;
                }
                response
            }
        },
    ));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let stopped = cancel.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
            .unwrap()
    });
    let client = reqwest::Client::new();
    let input = input(b"payload");
    let id = uuid::Uuid::new_v4().to_string();
    let upload = client
        .post(format!(
            "http://{address}/api/v1/vaults/{vault}/sessions/{session}/attachments?{}",
            query(&input)
        ))
        .bearer_auth(f.token("alice"))
        .header("x-loofah-api-major", "1")
        .header("x-request-id", &id)
        .header("content-type", "application/octet-stream")
        .body("payload");
    let retry = upload.try_clone().unwrap();
    let request = tokio::spawn(async move { upload.send().await });
    tokio::time::timeout(Duration::from_secs(3), committed.notified())
        .await
        .unwrap();
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    release.cancel();
    let status: AttachmentUploadStatus = client
        .get(format!(
            "http://{address}/api/v1/vaults/{vault}/sessions/{session}/attachment-uploads/{id}?{}",
            query(&input)
        ))
        .bearer_auth(f.token("alice"))
        .header("x-loofah-api-major", "1")
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let AttachmentUploadStatus::Committed(metadata) = status else {
        panic!("committed upload was not recoverable after its reply was lost");
    };
    let replay: FileMetadata = retry
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(replay, metadata);
    let context = f.service.vault(vault).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(&session);
    assert_eq!(
        std::fs::read_dir(directory.join("attachments"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read(directory.join("notes.md")).unwrap(),
        b"Canonical note"
    );
    cancel.cancel();
    server.await.unwrap();
    f.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn healthy_tcp_uploads_continue_beyond_the_ordinary_thirty_second_request_deadline() {
    let f = Fixture::new().await;
    let alice = f.login("alice").await;
    let vault = &alice.personal_vault_id;
    let session = session(&f, "alice", vault).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let cancel = tokio_util::sync::CancellationToken::new();
    let stopped = cancel.clone();
    let app = f.app.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
            .unwrap()
    });
    let data = vec![b'x'; 16 * 1024];
    let input = input(&data);
    let body = reqwest::Body::wrap_stream(futures_util::stream::unfold(0_u8, |index| async move {
        if index == 4 {
            return None;
        }
        tokio::time::sleep(Duration::from_secs(8)).await;
        Some((
            Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 4096])),
            index + 1,
        ))
    }));
    let result = reqwest::Client::new()
        .post(format!(
            "http://{address}/api/v1/vaults/{vault}/sessions/{session}/attachments?{}",
            query(&input)
        ))
        .bearer_auth(f.token("alice"))
        .header("x-loofah-api-major", "1")
        .header("x-request-id", uuid::Uuid::new_v4().to_string())
        .header("content-type", "application/octet-stream")
        .body(body)
        .timeout(Duration::from_secs(50))
        .send()
        .await
        .unwrap();
    assert_eq!(result.status(), reqwest::StatusCode::OK);
    let metadata: FileMetadata = result.json().await.unwrap();
    assert_eq!(metadata.sha256, input.sha256);
    cancel.cancel();
    server.await.unwrap();
    f.service.shutdown().await.unwrap();
}
