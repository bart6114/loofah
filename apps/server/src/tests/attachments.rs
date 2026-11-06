use super::recording::request;
use super::*;
use std::time::Duration;

#[tokio::test]
async fn attachment_information_is_scoped_pathless_and_does_not_consume_download_leases() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    fixture.login("bob").await;
    let vault = &alice.personal_vault_id;
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &format!("/api/v1/vaults/{vault}/sessions"),
            Some(json!({"title":"File information","markdown":"Keep note"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let context = fixture.service.vault(vault).await.unwrap();
    context
        .store
        .save_attachment(session, "chart + # ü.png", b"image bytes".to_vec())
        .await
        .unwrap();
    let prefix = format!("/api/v1/vaults/{vault}/sessions/{session}/attachments");
    let asset = format!("{prefix}/chart%20%2B%20%23%20%C3%BC.png");
    let info_path = format!("{asset}/info");
    assert_eq!(
        fixture.call("bob", "GET", &info_path, None).await.0,
        StatusCode::NOT_FOUND
    );
    let (status, info) = fixture.call("alice", "GET", &info_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        info,
        json!({"filename":"chart + # ü.png","contentType":"image/png","sizeBytes":11})
    );
    for _ in 0..8 {
        assert_eq!(
            fixture
                .call(
                    "alice",
                    "PUT",
                    &format!("{asset}/leases/{}", uuid::Uuid::new_v4()),
                    None
                )
                .await
                .0,
            StatusCode::OK
        );
    }
    let (_, limit) = fixture
        .call(
            "alice",
            "PUT",
            &format!("{asset}/leases/{}", uuid::Uuid::new_v4()),
            None,
        )
        .await;
    assert_eq!(limit["code"], "resource_limit");
    assert_eq!(
        fixture.call("alice", "GET", &info_path, None).await,
        (StatusCode::OK, info)
    );
    for id in ["missing.png", "notes.md", "unknown-user.png"] {
        let (status, missing) = fixture
            .call("alice", "GET", &format!("{prefix}/{id}/info"), None)
            .await;
        assert_eq!(status, StatusCode::OK);
        assert!(missing.is_null());
    }
    assert_eq!(
        fixture
            .call("alice", "GET", &format!("{prefix}/.private/info"), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        std::fs::read_to_string(
            context
                .store
                .vault_base()
                .join("sessions")
                .join(session)
                .join("notes.md")
        )
        .unwrap(),
        "Keep note"
    );
}

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
async fn media_info_and_open_requests_release_catalog_grants_then_recheck_revocation() {
    for kind in ["attachment", "recording", "metadata", "info"] {
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
        if kind == "info" {
            let (status, info) = fixture
                .call(
                    "bob",
                    "GET",
                    &format!("/api/v1/vaults/{vault}/sessions/note/attachments/a.png/info"),
                    None,
                )
                .await;
            assert_eq!(status, StatusCode::OK);
            assert_eq!(info["sizeBytes"], 10);
        }
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
            "info" => (
                "GET",
                format!("/api/v1/vaults/{vault}/sessions/note/attachments/a.png/info"),
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

#[tokio::test]
async fn attachment_listing_is_paginated_pathless_isolated_and_available_to_readers() {
    let fixture = Fixture::new().await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Attachment pages"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &format!("/api/v1/vaults/{vault}/sessions"),
            Some(json!({"title":"Attachment pages","markdown":"Canonical note"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let context = fixture.service.vault(vault).await.unwrap();
    for name in ["z.pdf", "b + ü.png", "a.txt"] {
        context
            .store
            .save_attachment(session, name, b"owned".to_vec())
            .await
            .unwrap();
    }
    let directory = fixture
        ._root
        .path()
        .join("storage/vaults")
        .join(vault)
        .join("sessions")
        .join(session);
    let note = std::fs::read(directory.join("notes.md")).unwrap();
    std::fs::write(directory.join("unknown-user.pdf"), "User owned, never list").unwrap();
    std::fs::write(directory.join("attachments/.hidden"), "Private").unwrap();
    std::fs::create_dir(directory.join("attachments/directory")).unwrap();
    let path = format!("/api/v1/vaults/{vault}/sessions/{session}/attachments");
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    let membership = format!("/api/v1/vaults/{vault}/members/{}", bob.member.id);
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &membership,
                Some(json!({"role":"reader","expected_revision":shared["revision"]}))
            )
            .await
            .0,
        StatusCode::OK
    );
    let (status, first) = fixture
        .call("alice", "GET", &format!("{path}?limit=2"), None)
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        first,
        json!({"files":[{"filename":"a.txt","contentType":"text/plain","sizeBytes":5},{"filename":"b + ü.png","contentType":"image/png","sizeBytes":5}],"next_cursor":"b + ü.png"})
    );
    let (status, second) = fixture
        .call(
            "alice",
            "GET",
            &format!("{path}?limit=2&after=b%20%2B%20%C3%BC.png"),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        second,
        json!({"files":[{"filename":"z.pdf","contentType":"application/pdf","sizeBytes":5}],"next_cursor":null})
    );
    assert_eq!(
        fixture
            .call("bob", "GET", &format!("{path}?limit=2"), None)
            .await,
        (StatusCode::OK, first.clone())
    );
    for query in [
        "limit=0",
        "limit=201",
        "after=..%2Fprivate",
        "after=.hidden",
        "limit=bad",
    ] {
        assert_eq!(
            fixture
                .call("alice", "GET", &format!("{path}?{query}"), None)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        fixture
            .call(
                "alice",
                "GET",
                &format!("/api/v1/vaults/{vault}/sessions/missing/attachments"),
                None
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(std::fs::read(directory.join("notes.md")).unwrap(), note);
    assert_eq!(
        std::fs::read_to_string(directory.join("unknown-user.pdf")).unwrap(),
        "User owned, never list"
    );
}
