use super::*;

#[tokio::test]
async fn attachment_removal_checks_roles_revision_receipts_and_exact_resource_invalidation() {
    let fixture = Fixture::new().await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Files"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    let members = format!("/api/v1/vaults/{vault}/members/{}", bob.member.id);
    let (_, shared) = fixture
        .call(
            "alice",
            "PUT",
            &members,
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    let (_, created) = fixture
        .call(
            "alice",
            "POST",
            &format!("/api/v1/vaults/{vault}/sessions"),
            Some(json!({"title":"Files","markdown":"Keep canonical notes"})),
        )
        .await;
    let session = created["meta"]["id"].as_str().unwrap();
    let context = fixture.service.vault(vault).await.unwrap();
    let directory = context.store.vault_base().join("sessions").join(session);
    let note = std::fs::read(directory.join("notes.md")).unwrap();
    std::fs::write(directory.join("unknown.pdf"), b"User owned").unwrap();
    for name in ["file.pdf", "other.pdf"] {
        context
            .store
            .save_attachment(session, name, b"Original bytes".to_vec())
            .await
            .unwrap();
    }
    let prefix = format!("/api/v1/vaults/{vault}/sessions/{session}/attachments");
    let metadata_path = format!("{prefix}/file.pdf/metadata");
    let (status, metadata) = fixture.call("bob", "GET", &metadata_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(metadata["filename"], "file.pdf");
    assert_eq!(metadata["contentType"], "application/pdf");
    assert_eq!(metadata["sizeBytes"], 14);
    assert_eq!(metadata.as_object().unwrap().len(), 4);
    let sha = metadata["sha256"].as_str().unwrap();
    let path = format!("{prefix}/file.pdf?expected_sha256={sha}");
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        fixture
            .call_token(&fixture.token("bob"), "DELETE", &path, None, Some(&id))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        fixture.call("charlie", "GET", &metadata_path, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture.call("charlie", "DELETE", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .call_token(&fixture.token("alice"), "DELETE", &path, None, None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (_, shared) = fixture
        .call(
            "alice",
            "PUT",
            &members,
            Some(json!({"role":"editor","expected_revision":shared["revision"]})),
        )
        .await;
    let (status, stale) = fixture
        .call(
            "bob",
            "DELETE",
            &format!("{prefix}/file.pdf?expected_sha256={}", "0".repeat(64)),
            None,
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(stale["mutation_outcome"], "not_committed");
    let first = uuid::Uuid::new_v4().to_string();
    let other = uuid::Uuid::new_v4().to_string();
    for (name, lease) in [("file.pdf", &first), ("other.pdf", &other)] {
        assert_eq!(
            fixture
                .call(
                    "bob",
                    "PUT",
                    &format!("{prefix}/{name}/leases/{lease}"),
                    None
                )
                .await
                .0,
            StatusCode::OK
        );
    }
    let held = fixture
        .service
        .media_leases
        .attachment_reader(&bob.member.id, vault, &first)
        .unwrap();
    assert_eq!(
        fixture
            .call_token(&fixture.token("bob"), "DELETE", &path, None, Some(&id))
            .await,
        (StatusCode::OK, json!(true))
    );
    assert!(!directory.join("attachments/file.pdf").exists());
    assert!(
        fixture
            .service
            .media_leases
            .attachment_reader(&bob.member.id, vault, &first)
            .is_err()
    );
    assert!(held.read_chunk(0, 1).await.is_err());
    assert!(
        fixture
            .service
            .media_leases
            .attachment_reader(&bob.member.id, vault, &other)
            .is_ok()
    );
    context
        .store
        .save_attachment(session, "file.pdf", b"Restored file".to_vec())
        .await
        .unwrap();
    let restored = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        fixture
            .call(
                "bob",
                "PUT",
                &format!("{prefix}/file.pdf/leases/{restored}"),
                None
            )
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        fixture
            .call_token(&fixture.token("bob"), "DELETE", &path, None, Some(&id))
            .await,
        (StatusCode::OK, json!(true))
    );
    assert_eq!(
        std::fs::read(directory.join("attachments/file.pdf")).unwrap(),
        b"Restored file"
    );
    assert!(
        fixture
            .service
            .media_leases
            .attachment_reader(&bob.member.id, vault, &restored)
            .is_ok()
    );
    let mismatch = format!("{prefix}/file.pdf?expected_sha256={}", "1".repeat(64));
    assert_eq!(
        fixture
            .call_token(&fixture.token("bob"), "DELETE", &mismatch, None, Some(&id))
            .await
            .0,
        StatusCode::CONFLICT
    );
    let revoked = fixture
        .call(
            "alice",
            "DELETE",
            &format!(
                "{members}?expected_revision={}",
                shared["revision"].as_str().unwrap()
            ),
            None,
        )
        .await;
    assert_eq!(revoked.0, StatusCode::OK);
    assert_eq!(
        fixture
            .call_token(&fixture.token("bob"), "DELETE", &path, None, Some(&id))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(std::fs::read(directory.join("notes.md")).unwrap(), note);
    assert_eq!(
        std::fs::read(directory.join("unknown.pdf")).unwrap(),
        b"User owned"
    );
    assert_eq!(
        std::fs::read(directory.join("attachments/other.pdf")).unwrap(),
        b"Original bytes"
    );
    fixture.service.shutdown().await.unwrap();
}
