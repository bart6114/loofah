use super::*;
#[tokio::test]
async fn initial_metadata_creation_is_one_checked_receipt_with_strict_access_and_input() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Initial metadata"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    let path = format!("/api/v1/vaults/{vault}/session-creations");
    let input = json!({"title":"Created together","markdown":"Initial user note","metadata":{"created_at":"2026-11-08T10:00:00+01:00","started_at":"2026-11-08T09:00:00Z","ended_at":null,"tags":["original"],"author":"cli","skill":"meeting"}});
    for (subject, target, expected) in [
        ("bob", vault, StatusCode::FORBIDDEN),
        ("charlie", vault, StatusCode::NOT_FOUND),
        (
            "bob",
            alice.personal_vault_id.as_str(),
            StatusCode::NOT_FOUND,
        ),
    ] {
        assert_eq!(
            fixture
                .call(
                    subject,
                    "POST",
                    &format!("/api/v1/vaults/{target}/session-creations"),
                    Some(input.clone())
                )
                .await
                .0,
            expected
        );
    }
    let request = uuid::Uuid::new_v4().to_string();
    let (status, meta) = fixture
        .call_token(
            &fixture.token("alice"),
            "POST",
            &path,
            Some(input.clone()),
            Some(&request),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(meta["created_at"], "2026-11-08T09:00:00.000Z");
    assert_eq!(meta["author"], "cli");
    assert_eq!(meta["skill"], "meeting");
    let session = meta["id"].as_str().unwrap();
    let context = fixture.service.vault(vault).await.unwrap();
    assert_eq!(
        context
            .read_note(session)
            .await
            .unwrap()
            .markdown
            .as_deref(),
        Some("Initial user note")
    );
    assert_eq!(
        fixture
            .call_token(
                &fixture.token("alice"),
                "POST",
                &path,
                Some(input.clone()),
                Some(&request)
            )
            .await,
        (StatusCode::OK, meta.clone())
    );
    let mut changed = input.clone();
    changed["metadata"]["author"] = "different".into();
    assert_eq!(
        fixture
            .call_token(
                &fixture.token("alice"),
                "POST",
                &path,
                Some(changed),
                Some(&request)
            )
            .await
            .0,
        StatusCode::CONFLICT
    );
    let count = context.store.session_list().len();
    for invalid in [
        json!({"title":"Invalid","markdown":"x","metadata":{"tags":[],"author":null,"skill":null,"created_at":null,"started_at":null,"ended_at":null,"id":"forged"}}),
        json!({"title":"Invalid","markdown":"x","metadata":{"tags":[],"author":null,"skill":"requires-author","created_at":null,"started_at":null,"ended_at":null}}),
    ] {
        assert_eq!(
            fixture.call("alice", "POST", &path, Some(invalid)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(context.store.session_list().len(), count);
}
