use super::*;

#[tokio::test]
async fn recording_metadata_preserves_reader_access_isolation_and_canonical_content() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Media"})),
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
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &format!("/api/v1/vaults/{vault}/sessions"),
            Some(json!({"title":"Recording","markdown":"Keep note text"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let path = format!("/api/v1/vaults/{vault}/sessions/{session}/audio/metadata");
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await,
        (StatusCode::OK, Value::Null)
    );
    let context = fixture.service.vault(vault).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(session);
    let meta = std::fs::read(directory.join("_meta.json")).unwrap();
    std::fs::write(directory.join("audio.wav"), b"Canonical recording bytes").unwrap();
    std::fs::write(directory.join("private-user.wav"), b"Unknown attachment").unwrap();
    std::fs::write(directory.join(".audio.mp3"), b"Hidden attachment").unwrap();
    let (status, metadata) = fixture.call("bob", "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        metadata,
        json!({"filename":"audio.wav","contentType":"audio/wav","sizeBytes":b"Canonical recording bytes".len(),"sha256":"83f548a8344aac5322bc85046d3e54dcab06dd3fd8f6d91801a768dea8c7d188"})
    );
    for (subject, vault_id) in [
        ("charlie", vault),
        ("bob", alice.personal_vault_id.as_str()),
    ] {
        assert_eq!(
            fixture
                .call(
                    subject,
                    "GET",
                    &format!("/api/v1/vaults/{vault_id}/sessions/{session}/audio/metadata"),
                    None
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        fixture
            .call(
                "bob",
                "GET",
                &format!("/api/v1/vaults/{vault}/sessions/missing/audio/metadata"),
                None
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(std::fs::read(directory.join("_meta.json")).unwrap(), meta);
    assert_eq!(
        std::fs::read_to_string(directory.join("notes.md")).unwrap(),
        "Keep note text"
    );
    assert_eq!(
        std::fs::read(directory.join("private-user.wav")).unwrap(),
        b"Unknown attachment"
    );
    assert_eq!(
        std::fs::read(directory.join(".audio.mp3")).unwrap(),
        b"Hidden attachment"
    );
    context.store.begin_audio_import(session).await.unwrap();
    let (status, error) = fixture.call("bob", "GET", &path, None).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(error["code"], "unavailable");
    assert_eq!(error["retryable"], true);
    context.store.finish_audio_import(session).await.unwrap();
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await,
        (StatusCode::OK, metadata)
    );
    let media_slots = fixture
        .service
        .media_reads
        .clone()
        .try_acquire_many_owned(2)
        .unwrap();
    let content_slots = fixture
        .service
        .content_reads
        .clone()
        .try_acquire_many_owned(2)
        .unwrap();
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.0,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        fixture
            .call(
                "bob",
                "GET",
                &format!("/api/v1/vaults/{vault}/sessions/{session}/note"),
                None
            )
            .await
            .0,
        StatusCode::OK
    );
    drop(content_slots);
    drop(media_slots);
}
