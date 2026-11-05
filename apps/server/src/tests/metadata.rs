use super::*;

#[tokio::test]
async fn metadata_patches_are_checked_reader_protected_and_keep_canonical_content_intact() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Metadata"})),
        )
        .await;
    let id = shared["id"].as_str().unwrap();
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!("/api/v1/vaults/{id}/members/{}", bob.member.id),
                Some(json!({"role":"reader","expected_revision":shared["revision"]}))
            )
            .await
            .0,
        StatusCode::OK
    );
    let base = format!("/api/v1/vaults/{id}/sessions");
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &base,
            Some(json!({"title":"Original","markdown":"User content"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let path = format!("{base}/{session}/metadata");
    let context = fixture.service.vault(id).await.unwrap();
    let dir = context.store.vault_base().join("sessions").join(session);
    std::fs::write(dir.join("foreign.txt"), "ATTACHMENT CANARY").unwrap();
    let (status, before) = fixture.call("bob", "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        before,
        serde_json::to_value(context.read_metadata(session).await.unwrap()).unwrap()
    );
    let input = json!({"patch":{"title":"Renamed","tags":["project"],"author":"agent","created_at":"2026-11-06T10:00:00Z"},"expected_revision":before["revision"]});
    assert_eq!(
        fixture
            .call("bob", "PATCH", &path, Some(input.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for method in ["GET", "PATCH"] {
        assert_eq!(
            fixture
                .call(
                    "charlie",
                    method,
                    &path,
                    (method == "PATCH").then(|| input.clone())
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        let personal = format!(
            "/api/v1/vaults/{}/sessions/{session}/metadata",
            alice.personal_vault_id
        );
        assert_eq!(
            fixture
                .call(
                    "bob",
                    method,
                    &personal,
                    (method == "PATCH").then(|| input.clone())
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    let (status, saved) = fixture
        .call("alice", "PATCH", &path, Some(input.clone()))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["kind"], "saved");
    assert_eq!(saved["snapshot"]["meta"]["title"], "Renamed");
    let (status, conflict) = fixture.call("alice", "PATCH", &path, Some(input)).await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(conflict["kind"], "conflict");
    assert_eq!(conflict["snapshot"], saved["snapshot"]);
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.1,
        saved["snapshot"]
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.md")).unwrap(),
        "User content"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("foreign.txt")).unwrap(),
        "ATTACHMENT CANARY"
    );
    assert_eq!(
        fixture
            .call(
                "alice",
                "PATCH",
                &path,
                Some(json!({"patch":{"title":"No revision"}}))
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(fixture.call("alice","PATCH",&path,Some(json!({"patch":{"created_at":"bad timestamp"},"expected_revision":saved["snapshot"]["revision"]}))).await.0,StatusCode::BAD_REQUEST);
    let missing = format!("{base}/missing/metadata");
    for method in ["GET", "PATCH"] {
        assert_eq!(
            fixture
                .call(
                    "alice",
                    method,
                    &missing,
                    (method == "PATCH").then(
                        || json!({"patch":{"title":"Do not recreate"},"expected_revision":"none"})
                    )
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert!(!context.store.vault_base().join("sessions/missing").exists());
    assert_eq!(
        context.read_metadata(session).await.unwrap().meta.title,
        "Renamed"
    );
    fixture.service.shutdown().await.unwrap();
}
