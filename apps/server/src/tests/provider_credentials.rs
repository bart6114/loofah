use super::*;

#[tokio::test]
async fn provider_credential_routes_require_owner_and_preserve_secret_vault_isolation() {
    let processing =
        serde_json::from_value(json!({"providers":[{"id":"openai","models":["approved"]}]}))
            .unwrap();
    let fixture = Fixture::with_processing(Some(processing)).await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Credentials"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    let (_, membership) = fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"editor","expected_revision":shared["revision"]})),
        )
        .await;
    let path = format!("/api/v1/vaults/{vault}/processing/providers/openai/credential");
    let (status, initial) = fixture.call("alice", "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(initial["source"], "missing");
    let input = json!({"expected_revision":initial["revision"],"api_key":"PRIVATE-CANARY"});
    for method in ["GET", "PUT"] {
        let body = (method == "PUT").then(|| input.clone());
        assert_eq!(
            fixture.call("bob", method, &path, body.clone()).await.0,
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            fixture.call("charlie", method, &path, body).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let (status, saved) = fixture
        .call("alice", "PUT", &path, Some(input.clone()))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["kind"], "saved");
    assert_eq!(saved["status"]["source"], "vault");
    assert!(!saved.to_string().contains("PRIVATE-CANARY"));
    assert!(!saved.to_string().contains("api_key"));
    let (_, conflict) = fixture.call("alice", "PUT", &path, Some(input)).await;
    assert_eq!(conflict["kind"], "conflict");
    assert_eq!(conflict["status"], saved["status"]);
    let context = fixture.service.vault(vault).await.unwrap();
    assert_eq!(
        context
            .provider_credentials()
            .unwrap()
            .resolve("openai".into())
            .await
            .unwrap()
            .as_deref(),
        Some("PRIVATE-CANARY")
    );
    let (_, own) = fixture
        .call(
            "alice",
            "GET",
            &format!(
                "/api/v1/vaults/{}/processing/providers/openai/credential",
                alice.personal_vault_id
            ),
            None,
        )
        .await;
    assert_eq!(own["source"], "missing");
    let (_, policy) = fixture
        .call(
            "bob",
            "GET",
            &format!("/api/v1/vaults/{vault}/processing/policy"),
            None,
        )
        .await;
    let (_, settings) = fixture
        .call(
            "bob",
            "GET",
            &format!("/api/v1/vaults/{vault}/processing/settings"),
            None,
        )
        .await;
    assert!(!format!("{policy}{settings}").contains("PRIVATE-CANARY"));
    for invalid in [
        json!({"expected_revision":saved["status"]["revision"]}),
        json!({"expected_revision":saved["status"]["revision"],"api_key":"HEADER\r\nCANARY"}),
        json!({"expected_revision":saved["status"]["revision"],"api_key":"key","endpoint":"https://evil.example"}),
    ] {
        let (status, reply) = fixture.call("alice", "PUT", &path, Some(invalid)).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(!reply.to_string().contains("CANARY"));
        assert!(!reply.to_string().contains("evil.example"));
    }
    assert_eq!(
        fixture
            .call(
                "alice",
                "GET",
                &path.replace("/openai/", "/unapproved/"),
                None
            )
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let (status, cleared) = fixture
        .call(
            "alice",
            "PUT",
            &path,
            Some(json!({"expected_revision":saved["status"]["revision"],"api_key":null})),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cleared["status"]["source"], "missing");
    let (_, owner_membership) = fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"owner","expected_revision":membership["revision"]})),
        )
        .await;
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.0,
        StatusCode::OK
    );
    fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"reader","expected_revision":owner_membership["revision"]})),
        )
        .await;
    assert_eq!(
        fixture.call("bob", "GET", &path, None).await.0,
        StatusCode::FORBIDDEN
    );
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn disabled_processing_does_not_advertise_or_initialize_credentials() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let (_, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        !capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("provider_credentials"))
    );
    assert_eq!(
        fixture
            .call(
                "alice",
                "GET",
                &format!(
                    "/api/v1/vaults/{}/processing/providers/openai/credential",
                    me.personal_vault_id
                ),
                None
            )
            .await
            .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    fixture.service.shutdown().await.unwrap();
}
