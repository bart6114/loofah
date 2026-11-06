use super::recording::request;
use super::*;
use std::time::Duration;

#[tokio::test]
async fn attachment_ranges_preserve_owned_paths_pinned_generations_and_endpoint_isolation() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    fixture.login("bob").await;
    let vault = &alice.personal_vault_id;
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &format!("/api/v1/vaults/{vault}/sessions"),
            Some(json!({"title":"Attachments","markdown":"Keep this note"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let context = fixture.service.vault(vault).await.unwrap();
    let saved = context
        .store
        .save_attachment(session, "chart + # ü.png", b"0123456789".to_vec())
        .await
        .unwrap();
    assert_eq!(saved.attachment_id, "chart + # ü.png");
    let directory = context.store.vault_base().join("sessions").join(session);
    std::fs::write(directory.join("unknown-user.png"), b"unknown file").unwrap();
    std::fs::write(directory.join("attachments/.private"), b"hidden file").unwrap();
    let protected: Vec<_> = [
        "notes.md",
        "_meta.json",
        "unknown-user.png",
        "attachments/.private",
    ]
    .map(|name| {
        (
            directory.join(name),
            std::fs::read(directory.join(name)).unwrap(),
        )
    })
    .into();
    let lease = uuid::Uuid::new_v4();
    let open = format!(
        "/api/v1/vaults/{vault}/sessions/{session}/attachments/chart%20%2B%20%23%20%C3%BC.png/leases/{lease}"
    );
    let endpoint = format!("/api/v1/vaults/{vault}/attachments/leases/{lease}");
    assert_eq!(
        fixture.call("bob", "PUT", &open, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, info) = fixture.call("alice", "PUT", &open, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["attachment_id"], saved.attachment_id);
    assert_eq!(info["metadata"]["filename"], saved.attachment_id);
    assert_eq!(info["metadata"]["sizeBytes"], 10);
    assert!(
        !info
            .to_string()
            .contains(context.store.vault_base().to_string_lossy().as_ref())
    );
    assert_eq!(fixture.call("alice", "PUT", &open, None).await.1, info);
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!("/api/v1/vaults/{vault}/sessions/{session}/audio/leases/{lease}"),
                None
            )
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        request(
            &fixture,
            "alice",
            "GET",
            &format!("/api/v1/vaults/{vault}/audio/leases/{lease}"),
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(
            &fixture,
            "alice",
            "DELETE",
            &format!("/api/v1/vaults/{vault}/audio/leases/{lease}"),
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&fixture, "bob", "GET", &endpoint, &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let etag = format!(
        "\"sha256:{}\"",
        info["metadata"]["sha256"].as_str().unwrap()
    );
    let response = request(
        &fixture,
        "alice",
        "GET",
        &endpoint,
        &[("range", "bytes=2-4"), ("if-match", &etag)],
    )
    .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["etag"], etag);
    assert_eq!(response.headers()["content-range"], "bytes 2-4/10");
    assert_eq!(response.headers()["content-type"], "image/png");
    assert_eq!(response.headers()["x-content-type-options"], "nosniff");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "234");
    let head = request(
        &fixture,
        "alice",
        "HEAD",
        &endpoint,
        &[("range", "bytes=2-4")],
    )
    .await;
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(head.headers()["content-length"], "10");
    assert!(to_bytes(head.into_body(), 1).await.unwrap().is_empty());
    assert_eq!(
        request(
            &fixture,
            "alice",
            "GET",
            &endpoint,
            &[("if-match", "\"stale\"")]
        )
        .await
        .status(),
        StatusCode::PRECONDITION_FAILED
    );
    assert_eq!(
        request(
            &fixture,
            "alice",
            "GET",
            &endpoint,
            &[("range", "bytes=0-1,3-4")]
        )
        .await
        .status(),
        StatusCode::RANGE_NOT_SATISFIABLE
    );
    for filename in ["notes.md", "unknown-user.png", "missing"] {
        assert_eq!(fixture.call("alice", "PUT", &format!("/api/v1/vaults/{vault}/sessions/{session}/attachments/{filename}/leases/{}", uuid::Uuid::new_v4()), None).await, (StatusCode::OK, Value::Null));
    }
    for filename in [".private", "..%2Fnotes.md", "nested%2Ffile.png"] {
        assert_eq!(fixture.call("alice", "PUT", &format!("/api/v1/vaults/{vault}/sessions/{session}/attachments/{filename}/leases/{}", uuid::Uuid::new_v4()), None).await.0, StatusCode::BAD_REQUEST);
    }
    let path = context.store.vault_base().join(saved.relative_path);
    std::fs::rename(&path, path.with_extension("retired")).unwrap();
    std::fs::write(path, b"replacement").unwrap();
    let response = request(&fixture, "alice", "GET", &endpoint, &[]).await;
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "0123456789"
    );
    assert_eq!(
        request(&fixture, "alice", "DELETE", &endpoint, &[])
            .await
            .status(),
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&fixture, "alice", "GET", &endpoint, &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    for (path, bytes) in protected {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn metadata_and_both_media_open_kinds_release_catalog_grants_then_recheck_revocation() {
    for kind in ["attachment", "recording", "metadata"] {
        let fixture = Fixture::new().await;
        fixture.login("alice").await;
        let bob = fixture.login("bob").await;
        let (_, shared) = fixture
            .call(
                "alice",
                "POST",
                "/api/v1/vaults",
                Some(json!({"name":"Media"})),
            )
            .await;
        let vault = shared["id"].as_str().unwrap();
        let membership = format!("/api/v1/vaults/{vault}/members/{}", bob.member.id);
        let (status, shared) = fixture
            .call(
                "alice",
                "PUT",
                &membership,
                Some(json!({"role":"reader", "expected_revision": shared["revision"]})),
            )
            .await;
        assert_eq!(status, StatusCode::OK);
        let context = fixture.service.vault(vault).await.unwrap();
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Media","created_at":"2026-11-06T10:00:00Z","tags":[]}),
        )
        .unwrap();
        context
            .store
            .create_note_session(&meta, "Keep note")
            .await
            .unwrap();
        context
            .store
            .save_attachment("note", "a.png", b"image data".to_vec())
            .await
            .unwrap();
        std::fs::write(
            context.store.vault_base().join("sessions/note/audio.wav"),
            b"audio data",
        )
        .unwrap();
        let snapshot = context.store.lock_snapshot().await;
        let lease = uuid::Uuid::new_v4();
        let (method, path) = match kind {
            "attachment" => (
                "PUT",
                format!("/api/v1/vaults/{vault}/sessions/note/attachments/a.png/leases/{lease}"),
            ),
            "recording" => (
                "PUT",
                format!("/api/v1/vaults/{vault}/sessions/note/audio/leases/{lease}"),
            ),
            _ => (
                "GET",
                format!("/api/v1/vaults/{vault}/sessions/note/audio/metadata"),
            ),
        };
        let request = Request::builder()
            .method(method)
            .uri(path)
            .header("authorization", format!("Bearer {}", fixture.token("bob")))
            .header("x-loofah-api-major", "1")
            .body(Body::empty())
            .unwrap();
        let before = fixture.service.media_reads.available_permits();
        let app = fixture.app.clone();
        let reading = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
        tokio::time::timeout(Duration::from_secs(2), async {
            while fixture.service.media_reads.available_permits() == before {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let removed = tokio::time::timeout(
            Duration::from_secs(1),
            fixture.call(
                "alice",
                "DELETE",
                &format!(
                    "{membership}?expected_revision={}",
                    shared["revision"].as_str().unwrap()
                ),
                None,
            ),
        )
        .await
        .unwrap();
        assert_eq!(removed.0, StatusCode::OK, "{kind}");
        drop(snapshot);
        assert_eq!(
            reading.await.unwrap().status(),
            StatusCode::NOT_FOUND,
            "{kind}"
        );
    }
}

#[tokio::test]
async fn cancelled_media_open_retains_global_admission_until_the_real_read_finishes() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let vault = &alice.personal_vault_id;
    let context = fixture.service.vault(vault).await.unwrap();
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Media","created_at":"2026-11-06T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Keep note")
        .await
        .unwrap();
    context
        .store
        .save_attachment("note", "a.png", b"image data".to_vec())
        .await
        .unwrap();
    let snapshot = context.store.lock_snapshot().await;
    let lease = uuid::Uuid::new_v4();
    let request = Request::builder()
        .method("PUT")
        .uri(format!(
            "/api/v1/vaults/{vault}/sessions/note/attachments/a.png/leases/{lease}"
        ))
        .header(
            "authorization",
            format!("Bearer {}", fixture.token("alice")),
        )
        .header("x-loofah-api-major", "1")
        .body(Body::empty())
        .unwrap();
    let before = fixture.service.media_reads.available_permits();
    let app = fixture.app.clone();
    let reading = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.service.media_reads.available_permits() == before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    reading.abort();
    assert!(reading.await.unwrap_err().is_cancelled());
    assert_eq!(fixture.service.media_reads.available_permits(), before - 1);
    drop(snapshot);
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.service.media_reads.available_permits() != before {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture
        .service
        .media_leases
        .close_attachment(&alice.member.id, vault, &lease.to_string())
        .unwrap();
}

#[tokio::test]
async fn attachment_streams_cannot_consume_all_recording_playback_capacity() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let vault = &alice.personal_vault_id;
    let context = fixture.service.vault(vault).await.unwrap();
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Media","created_at":"2026-11-06T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Keep note")
        .await
        .unwrap();
    context
        .store
        .save_attachment("note", "a.bin", vec![1; 200_000])
        .await
        .unwrap();
    std::fs::write(
        context.store.vault_base().join("sessions/note/audio.wav"),
        b"0123456789",
    )
    .unwrap();
    let attachment = uuid::Uuid::new_v4();
    let recording = uuid::Uuid::new_v4();
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!(
                    "/api/v1/vaults/{vault}/sessions/note/attachments/a.bin/leases/{attachment}"
                ),
                None
            )
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!("/api/v1/vaults/{vault}/sessions/note/audio/leases/{recording}"),
                None
            )
            .await
            .0,
        StatusCode::OK
    );
    let attachment_path = format!("/api/v1/vaults/{vault}/attachments/leases/{attachment}");
    let first = request(&fixture, "alice", "GET", &attachment_path, &[]).await;
    let second = request(&fixture, "alice", "GET", &attachment_path, &[]).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(second.status(), StatusCode::OK);
    assert_eq!(fixture.service.attachment_streams.available_permits(), 0);
    assert_eq!(fixture.service.media_streams.available_permits(), 6);
    assert_eq!(
        request(&fixture, "alice", "GET", &attachment_path, &[])
            .await
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        request(&fixture, "alice", "HEAD", &attachment_path, &[])
            .await
            .status(),
        StatusCode::OK
    );
    let response = request(
        &fixture,
        "alice",
        "GET",
        &format!("/api/v1/vaults/{vault}/audio/leases/{recording}"),
        &[],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "0123456789"
    );
    drop(first);
    drop(second);
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.service.attachment_streams.available_permits() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
