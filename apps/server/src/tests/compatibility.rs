use super::*;

#[tokio::test]
async fn older_api_one_wire_fixture_survives_additive_capabilities_and_future_major_cannot_mutate()
{
    #[derive(serde::Deserialize)]
    struct OlderCapabilities {
        api_major: u32,
        features: Vec<String>,
    }
    #[derive(serde::Deserialize)]
    struct OlderNote {
        markdown: String,
        revision: String,
    }

    let processing =
        serde_json::from_value(json!({"providers":[{"id":"openai","models":["approved"]}]}))
            .unwrap();
    let fixture = Fixture::with_processing(Some(processing)).await;
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/capabilities")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let capabilities: OlderCapabilities =
        serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap()).unwrap();
    assert_eq!(capabilities.api_major, 1);
    assert!(capabilities.features.iter().any(|name| name == "notes"));
    assert!(
        capabilities
            .features
            .iter()
            .any(|name| name == "provider_credentials")
    );
    let alice = fixture.login("alice").await;
    let path = format!("/api/v1/vaults/{}/sessions", alice.personal_vault_id);
    let input: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/api1-minimal-note.json")).unwrap();
    let (status, created) = fixture
        .call("alice", "POST", &path, Some(input.clone()))
        .await;
    assert_eq!(status, StatusCode::OK);
    let note: OlderNote = serde_json::from_value(created["note"].clone()).unwrap();
    assert_eq!(note.markdown, input["markdown"]);
    assert!(!note.revision.is_empty());
    let before = fixture.call("alice", "GET", &path, None).await;
    for major in [None, Some("0"), Some("2")] {
        let mut request = Request::builder()
            .method("POST")
            .uri(&path)
            .header(
                "authorization",
                format!("Bearer {}", fixture.token("alice")),
            )
            .header("content-type", "application/json")
            .header("x-request-id", uuid::Uuid::new_v4().to_string());
        if let Some(major) = major {
            request = request.header("x-loofah-api-major", major);
        }
        let response = fixture
            .app
            .clone()
            .oneshot(request.body(Body::from(input.to_string())).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UPGRADE_REQUIRED);
        let error: ApplicationError =
            serde_json::from_slice(&to_bytes(response.into_body(), 64 * 1024).await.unwrap())
                .unwrap();
        assert_eq!(error.code, ErrorCode::IncompatibleVersion);
    }
    assert_eq!(fixture.call("alice", "GET", &path, None).await, before);
    fixture.service.shutdown().await.unwrap();
}
