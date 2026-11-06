use super::*;
use futures_util::StreamExt;

async fn request(
    fixture: &Fixture,
    subject: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
) -> axum::response::Response {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(
            "authorization",
            format!("Bearer {}", fixture.token(subject)),
        )
        .header("x-loofah-api-major", "1");
    for (key, value) in headers {
        request = request.header(*key, *value);
    }
    fixture
        .app
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}

#[tokio::test]
async fn authenticated_recording_ranges_keep_a_generation_and_never_expose_paths_or_unknown_files()
{
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    fixture.login("bob").await;
    let vault = &alice.personal_vault_id;
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &format!("/api/v1/vaults/{vault}/sessions"),
            Some(json!({"title":"Recording","markdown":"Keep this note"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let lease = uuid::Uuid::new_v4();
    let open = format!("/api/v1/vaults/{vault}/sessions/{session}/audio/leases/{lease}");
    let path = format!("/api/v1/vaults/{vault}/audio/leases/{lease}");
    assert_eq!(
        fixture.call("alice", "PUT", &open, None).await,
        (StatusCode::OK, Value::Null)
    );
    let context = fixture.service.vault(vault).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(session);
    let meta = std::fs::read(directory.join("_meta.json")).unwrap();
    std::fs::write(directory.join("audio.mp3"), b"0123456789").unwrap();
    std::fs::write(directory.join("private-user.mp3"), b"Never content").unwrap();
    let (status, info) = fixture.call("alice", "PUT", &open, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(info["id"], lease.to_string());
    assert_eq!(info["session_id"], session);
    assert_eq!(info["metadata"]["sizeBytes"], 10);
    assert_eq!(info["idle_timeout_secs"], 600);
    assert!(
        !info
            .to_string()
            .contains(&directory.to_string_lossy().to_string())
    );
    assert_eq!(fixture.call("alice", "PUT", &open, None).await.1, info);
    let etag = format!(
        "\"sha256:{}\"",
        info["metadata"]["sha256"].as_str().unwrap()
    );
    for (range, status, content_range, expected) in [
        (None, StatusCode::OK, None, "0123456789"),
        (
            Some("bytes=2-4"),
            StatusCode::PARTIAL_CONTENT,
            Some("bytes 2-4/10"),
            "234",
        ),
        (
            Some("bytes=-3"),
            StatusCode::PARTIAL_CONTENT,
            Some("bytes 7-9/10"),
            "789",
        ),
        (
            Some("bytes=8-99"),
            StatusCode::PARTIAL_CONTENT,
            Some("bytes 8-9/10"),
            "89",
        ),
    ] {
        let headers = range
            .map(|range| vec![("range", range)])
            .unwrap_or_default();
        let response = request(&fixture, "alice", "GET", &path, &headers).await;
        assert_eq!(response.status(), status);
        assert_eq!(response.headers()["etag"], etag);
        assert_eq!(response.headers()["content-type"], "audio/mpeg");
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(
            response
                .headers()
                .get("content-range")
                .map(|value| value.to_str().unwrap()),
            content_range
        );
        assert_eq!(
            to_bytes(response.into_body(), 1024).await.unwrap(),
            expected
        );
    }
    let response = request(&fixture, "alice", "HEAD", &path, &[("range", "bytes=2-4")]).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-length"], "10");
    assert!(
        to_bytes(response.into_body(), 1024)
            .await
            .unwrap()
            .is_empty()
    );
    for range in ["bytes=10-", "bytes=1-0", "bytes=0-1,3-4", "bytes=-0"] {
        let response = request(&fixture, "alice", "GET", &path, &[("range", range)]).await;
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(response.headers()["content-range"], "bytes */10");
    }
    assert_eq!(
        request(
            &fixture,
            "alice",
            "GET",
            &path,
            &[("if-match", "\"other\"")]
        )
        .await
        .status(),
        StatusCode::PRECONDITION_FAILED
    );
    let response = request(
        &fixture,
        "alice",
        "GET",
        &path,
        &[("range", "bytes=2-4"), ("if-range", "\"other\"")],
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        to_bytes(response.into_body(), 1024).await.unwrap(),
        "0123456789"
    );
    std::fs::rename(
        directory.join("audio.mp3"),
        directory.join("retired-user.mp3"),
    )
    .unwrap();
    std::fs::write(directory.join("audio.mp3"), b"Replacement recording").unwrap();
    let response = request(
        &fixture,
        "alice",
        "GET",
        &path,
        &[("range", "bytes=4-6"), ("if-match", &etag)],
    )
    .await;
    assert_eq!(to_bytes(response.into_body(), 1024).await.unwrap(), "456");
    assert_eq!(fixture.call("alice", "PUT", &open, None).await.1, info);
    for method in ["GET", "HEAD", "DELETE"] {
        assert_eq!(
            request(&fixture, "bob", method, &path, &[]).await.status(),
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        fixture.call("alice", "DELETE", &path, None).await.0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        request(&fixture, "alice", "GET", &path, &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(std::fs::read(directory.join("_meta.json")).unwrap(), meta);
    assert_eq!(
        std::fs::read_to_string(directory.join("notes.md")).unwrap(),
        "Keep this note"
    );
    assert_eq!(
        std::fs::read(directory.join("private-user.mp3")).unwrap(),
        b"Never content"
    );
}

#[tokio::test]
async fn reader_streams_release_catalog_locks_and_stop_after_revocation_without_stalling_notes() {
    let fixture = Fixture::new().await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Audio"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    let (status, shared) = fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    let context = fixture.service.vault(vault).await.unwrap();
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Audio","created_at":"2026-11-06T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Unchanged")
        .await
        .unwrap();
    let bytes: Vec<u8> = (0..200_000).map(|index| (index % 251) as u8).collect();
    std::fs::write(
        context.store.vault_base().join("sessions/note/audio.wav"),
        &bytes,
    )
    .unwrap();
    let lease = uuid::Uuid::new_v4();
    let open = format!("/api/v1/vaults/{vault}/sessions/note/audio/leases/{lease}");
    let path = format!("/api/v1/vaults/{vault}/audio/leases/{lease}");
    assert_eq!(
        fixture.call("bob", "PUT", &open, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        fixture.call("alice", "PUT", &open, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        request(&fixture, "alice", "GET", &path, &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    let capacity = fixture
        .service
        .recording_streams
        .clone()
        .try_acquire_many_owned(8)
        .unwrap();
    assert_eq!(
        request(&fixture, "bob", "GET", &path, &[]).await.status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        request(&fixture, "bob", "HEAD", &path, &[]).await.status(),
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .call(
                "bob",
                "GET",
                &format!("/api/v1/vaults/{vault}/sessions/note/note"),
                None
            )
            .await
            .0,
        StatusCode::OK
    );
    drop(capacity);
    let response = request(&fixture, "bob", "GET", &path, &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap(), &bytes[..64 * 1024]);
    let remove_path = format!(
        "/api/v1/vaults/{vault}/members/{}?expected_revision={}",
        bob.member.id,
        shared["revision"].as_str().unwrap()
    );
    let removal = fixture.call("alice", "DELETE", &remove_path, None);
    let (status, _) = tokio::time::timeout(std::time::Duration::from_secs(2), removal)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert!(stream.next().await.unwrap().is_err());
    drop(stream);
    assert_eq!(fixture.service.recording_streams.available_permits(), 8);
    assert_eq!(
        request(&fixture, "bob", "GET", &path, &[]).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn unpolled_recording_bodies_release_stream_slots_on_idle_timeout_and_shutdown() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let vault = &alice.personal_vault_id;
    let context = fixture.service.vault(vault).await.unwrap();
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Audio","created_at":"2026-11-06T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Unchanged")
        .await
        .unwrap();
    std::fs::write(
        context.store.vault_base().join("sessions/note/audio.wav"),
        vec![42u8; 200_000],
    )
    .unwrap();
    let id = uuid::Uuid::new_v4();
    let open = format!("/api/v1/vaults/{vault}/sessions/note/audio/leases/{id}");
    let path = format!("/api/v1/vaults/{vault}/audio/leases/{id}");
    assert_eq!(
        fixture.call("alice", "PUT", &open, None).await.0,
        StatusCode::OK
    );
    let response = request(&fixture, "alice", "GET", &path, &[]).await;
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap().len(), 64 * 1024);
    assert_eq!(fixture.service.recording_streams.available_permits(), 7);
    tokio::time::pause();
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    for _ in 0..20 {
        if fixture.service.recording_streams.available_permits() == 8 {
            break;
        }
        tokio::task::yield_now().await;
    }
    assert_eq!(fixture.service.recording_streams.available_permits(), 8);
    assert!(stream.next().await.unwrap().is_err());
    tokio::time::resume();
    let response = request(&fixture, "alice", "GET", &path, &[]).await;
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap().len(), 64 * 1024);
    fixture.service.stop.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while fixture.service.recording_streams.available_permits() != 8 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(stream.next().await.unwrap().is_err());
}
