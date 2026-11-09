use super::*;

#[tokio::test]
async fn processing_settings_enforce_roles_vault_isolation_policy_and_revisions() {
    let processing =
        serde_json::from_value(json!({"providers":[{"id":"ollama","models":["approved"]}]}))
            .unwrap();
    let fixture = Fixture::with_processing(Some(processing)).await;
    fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, shared) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Processing settings"})),
        )
        .await;
    let vault = shared["id"].as_str().unwrap();
    let (_, membership) = fixture
        .call(
            "alice",
            "PUT",
            &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
            Some(json!({"role":"reader","expected_revision":shared["revision"]})),
        )
        .await;
    let path = format!("/api/v1/vaults/{vault}/processing/settings");
    let policy_path = format!("/api/v1/vaults/{vault}/processing/policy");
    let (status, policy) = fixture.call("bob", "GET", &policy_path, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(policy["providers"][0]["models"], json!(["approved"]));
    assert_eq!(policy["providers"][0]["allow_custom_endpoint"], false);
    assert!(!policy.to_string().contains("api_key"));
    let (status, initial) = fixture.call("bob", "GET", &path, None).await;
    assert_eq!(status, StatusCode::OK);
    let mut input = json!({"settings":initial["settings"],"expected_revision":initial["revision"]});
    input["settings"]["summary_model"] =
        json!({"provider_id":"ollama","model_id":"approved","base_url":""});
    assert_eq!(
        fixture
            .call("bob", "PUT", &path, Some(input.clone()))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for resource in [&path, &policy_path] {
        assert_eq!(
            fixture.call("charlie", "GET", resource, None).await.0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            fixture
                .call_token("invalid", "GET", resource, None, None)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        fixture
            .call("charlie", "PUT", &path, Some(input.clone()))
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!("/api/v1/vaults/{vault}/members/{}", bob.member.id),
                Some(json!({"role":"editor","expected_revision":membership["revision"]}))
            )
            .await
            .0,
        StatusCode::OK
    );
    let (status, saved) = fixture.call("bob", "PUT", &path, Some(input.clone())).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(saved["kind"], "saved");
    input["settings"]["ai_language"] = json!("nl");
    let (_, conflict) = fixture
        .call("alice", "PUT", &path, Some(input.clone()))
        .await;
    assert_eq!(conflict["kind"], "conflict");
    assert_eq!(conflict["snapshot"], saved["snapshot"]);
    input["expected_revision"] = saved["snapshot"]["revision"].clone();
    for patch in [
        json!({"summary_model":{"provider_id":"ollama","model_id":"unapproved","base_url":""}}),
        json!({"summary_model":{"provider_id":"ollama","model_id":"approved","base_url":"http://another-vault.internal/v1"}}),
        json!({"summary_prompt":"{{ custom }}"}),
    ] {
        let mut invalid = input.clone();
        for (key, value) in patch.as_object().unwrap() {
            invalid["settings"][key] = value.clone();
        }
        assert_eq!(
            fixture.call("alice", "PUT", &path, Some(invalid)).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let mut raw = input.clone();
    raw["settings"]["api_key"] = json!("PRIVATE CANARY");
    assert_eq!(
        fixture.call("alice", "PUT", &path, Some(raw)).await.0,
        StatusCode::BAD_REQUEST
    );
    let context = fixture.service.vault(vault).await.unwrap();
    context
        .settings
        .set_values(std::collections::HashMap::from([(
            "private_unknown".into(),
            json!("PRIVATE CANARY"),
        )]))
        .await
        .unwrap();
    let (_, current) = fixture.call("bob", "GET", &path, None).await;
    assert_eq!(current, saved["snapshot"]);
    assert!(!current.to_string().contains("PRIVATE CANARY"));
    fixture.service.shutdown().await.unwrap();
}

#[tokio::test]
async fn unconfigured_host_does_not_advertise_or_expose_processing_settings() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    let (_, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        !capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("processing_settings"))
    );
    assert_eq!(
        fixture
            .call(
                "alice",
                "GET",
                &format!(
                    "/api/v1/vaults/{}/processing/settings",
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
