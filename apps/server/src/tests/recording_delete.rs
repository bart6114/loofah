use super::*;
use std::time::Duration;

#[tokio::test]
async fn pending_deletion_does_not_block_revocation_and_rechecks_access_before_commit() {
    for attachment in [false, true] {
        let fixture = Fixture::new().await;
        fixture.login("alice").await;
        let bob = fixture.login("bob").await;
        let (_, shared) = fixture
            .call(
                "alice",
                "POST",
                "/api/v1/vaults",
                Some(json!({"name":"Revocation"})),
            )
            .await;
        let vault = shared["id"].as_str().unwrap();
        let membership = format!("/api/v1/vaults/{vault}/members/{}", bob.member.id);
        assert_eq!(
            fixture
                .call(
                    "alice",
                    "PUT",
                    &membership,
                    Some(json!({"role":"editor","expected_revision":shared["revision"]}))
                )
                .await
                .0,
            StatusCode::OK
        );
        let context = fixture.service.vault(vault).await.unwrap();
        let meta = serde_json::from_value(
            json!({"id":"note","title":"Audio","created_at":"2026-11-06T10:00:00Z","tags":[]}),
        )
        .unwrap();
        context
            .store
            .create_note_session(&meta, "Keep this note")
            .await
            .unwrap();
        let directory = context.store.vault_base().join("sessions/note");
        if attachment {
            std::fs::create_dir(directory.join("attachments")).unwrap();
        }
        let path = directory.join(if attachment {
            "attachments/file.pdf"
        } else {
            "audio.wav"
        });
        std::fs::write(&path, b"Keep this recording").unwrap();
        let sha = if attachment {
            context
                .attachment_metadata("note", "file.pdf")
                .await
                .unwrap()
                .unwrap()
                .sha256
        } else {
            context
                .audio_metadata("note")
                .await
                .unwrap()
                .unwrap()
                .sha256
        };
        let target = if attachment {
            "attachments/file.pdf"
        } else {
            "audio"
        };
        let snapshot = context.store.lock_snapshot().await;
        let request = Request::builder()
            .method("DELETE")
            .uri(format!(
                "/api/v1/vaults/{vault}/sessions/note/{target}?expected_sha256={sha}"
            ))
            .header("authorization", format!("Bearer {}", fixture.token("bob")))
            .header("x-loofah-api-major", "1")
            .header("x-request-id", uuid::Uuid::new_v4().to_string())
            .body(Body::empty())
            .unwrap();
        let app = fixture.app.clone();
        let before = fixture.service.writes.available_permits();
        let deleting = tokio::spawn(async move { app.oneshot(request).await.unwrap() });
        tokio::time::timeout(Duration::from_secs(2), async {
            while fixture.service.writes.available_permits() == before {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let (_, current) = fixture
            .call("alice", "GET", &format!("/api/v1/vaults/{vault}"), None)
            .await;
        let removed = tokio::time::timeout(
            Duration::from_secs(1),
            fixture.call(
                "alice",
                "DELETE",
                &format!(
                    "{membership}?expected_revision={}",
                    current["revision"].as_str().unwrap()
                ),
                None,
            ),
        )
        .await
        .unwrap();
        assert_eq!(removed.0, StatusCode::OK);
        drop(snapshot);
        let response = deleting.await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let error: ApplicationError = serde_json::from_slice(&body).unwrap();
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
        assert_eq!(std::fs::read(path).unwrap(), b"Keep this recording");
    }
}

#[tokio::test]
async fn checked_recording_deletion_enforces_roles_isolation_preconditions_and_invalidates_open_handles()
 {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Recordings"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
                Some(json!({"role":"reader","expected_revision":shared["revision"]}))
            )
            .await
            .0,
        StatusCode::OK
    );
    let context = fixture.service.vault(vault).await.unwrap();
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Audio","created_at":"2026-11-06T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Keep this note")
        .await
        .unwrap();
    let directory = context.store.vault_base().join("sessions/note");
    std::fs::write(directory.join("audio.wav"), b"Original recording").unwrap();
    std::fs::write(directory.join("audio.peaks.json"), b"Cached waveform").unwrap();
    std::fs::write(directory.join("unknown.mp3"), b"Protected user attachment").unwrap();
    std::fs::write(directory.join(".private"), b"Protected private attachment").unwrap();
    let protected = ["notes.md", "_meta.json", "unknown.mp3", ".private"];
    let before: Vec<_> = protected
        .iter()
        .map(|name| std::fs::read(directory.join(name)).unwrap())
        .collect();
    let sha = context
        .audio_metadata("note")
        .await
        .unwrap()
        .unwrap()
        .sha256;
    let endpoint = format!("/api/v1/vaults/{vault}/sessions/note/audio?expected_sha256={sha}");
    assert_eq!(
        fixture.call("bob", "DELETE", &endpoint, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture.call("charlie", "DELETE", &endpoint, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .call(
                "bob",
                "DELETE",
                &format!(
                    "/api/v1/vaults/{}/sessions/note/audio?expected_sha256={sha}",
                    alice.personal_vault_id
                ),
                None
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let invalid = format!("/api/v1/vaults/{vault}/sessions/note/audio?expected_sha256=invalid");
    assert_eq!(
        fixture.call("alice", "DELETE", &invalid, None).await.0,
        StatusCode::BAD_REQUEST
    );
    context.store.note_recording_active("note");
    let (status, error) = fixture.call("alice", "DELETE", &endpoint, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["mutation_outcome"], "not_committed");
    context
        .store
        .release_recording_prepare("note")
        .await
        .unwrap();
    std::fs::write(directory.join("audio.wav"), b"Replaced recording").unwrap();
    let (status, error) = fixture.call("alice", "DELETE", &endpoint, None).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(error["mutation_outcome"], "not_committed");
    assert_eq!(
        std::fs::read(directory.join("audio.peaks.json")).unwrap(),
        b"Cached waveform"
    );
    let sha = context
        .audio_metadata("note")
        .await
        .unwrap()
        .unwrap()
        .sha256;
    let endpoint = format!("/api/v1/vaults/{vault}/sessions/note/audio?expected_sha256={sha}");
    let lease = uuid::Uuid::new_v4();
    let open = format!("/api/v1/vaults/{vault}/sessions/note/audio/leases/{lease}");
    assert_eq!(
        fixture.call("bob", "PUT", &open, None).await.0,
        StatusCode::OK
    );
    let reader = fixture
        .service
        .media_leases
        .reader(&bob.member.id, vault, &lease.to_string())
        .unwrap();
    assert_eq!(
        fixture.call("alice", "DELETE", &endpoint, None).await,
        (StatusCode::OK, json!(true))
    );
    assert_eq!(
        reader.read_chunk(0, 1).await.unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(
        fixture.call("alice", "DELETE", &endpoint, None).await,
        (StatusCode::OK, json!(false))
    );
    assert!(!directory.join("audio.wav").exists());
    assert!(!directory.join("audio.peaks.json").exists());
    for (name, bytes) in protected.iter().zip(before) {
        assert_eq!(std::fs::read(directory.join(name)).unwrap(), bytes);
    }
}
