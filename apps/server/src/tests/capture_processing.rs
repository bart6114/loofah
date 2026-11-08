use super::*;

#[tokio::test]
async fn notes_only_processing_refusal_preserves_raw_capture_and_creates_no_note() {
    let fixture = Fixture::new().await;
    let (_, capabilities) = fixture
        .call("alice", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        !capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("capture_processing"))
    );
    let me = fixture.login("alice").await;
    let input = capture::input(&fixture, &me);
    assert_eq!(
        capture::begin(&fixture, "alice", &input).await.0,
        StatusCode::OK
    );
    let path = format!(
        "/api/v1/vaults/{}/captures/{}/processing",
        me.personal_vault_id, input.identity.capture_id
    );
    let request = json!({"capture_fingerprint":capture_fingerprint(&input.identity,&input.format),"model":null});
    let (status, value) = fixture
        .call_token(
            &fixture.token("alice"),
            "PUT",
            &path,
            Some(request),
            Some(&input.identity.request_id),
        )
        .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{value}");
    let context = fixture.service.vault(&me.personal_vault_id).await.unwrap();
    let state = context
        .capture_status_authorized(
            me.member.id.clone(),
            input.identity.capture_id.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    assert_eq!(state.identity, input.identity);
    assert_eq!(state.prefix.chunk_count, 0);
    assert_eq!(state.session_id, None);
    assert_eq!(context.store.session_count(), 0);
}

#[tokio::test]
async fn processing_declaration_requires_original_identity_and_current_vault_membership() {
    let fixture = Fixture::new().await;
    let me = fixture.login("alice").await;
    fixture.login("bob").await;
    let input = capture::input(&fixture, &me);
    assert_eq!(
        capture::begin(&fixture, "alice", &input).await.0,
        StatusCode::OK
    );
    let path = format!(
        "/api/v1/vaults/{}/captures/{}/processing",
        me.personal_vault_id, input.identity.capture_id
    );
    let request = json!({"capture_fingerprint":capture_fingerprint(&input.identity,&input.format)});
    let (status, value) = fixture
        .call("alice", "PUT", &path, Some(request.clone()))
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{value}");
    for method in ["GET", "PUT"] {
        assert_eq!(
            fixture
                .call_token(
                    &fixture.token("bob"),
                    method,
                    &path,
                    (method == "PUT").then(|| request.clone()),
                    Some(&input.identity.request_id)
                )
                .await
                .0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        fixture
            .call("bob", "POST", &format!("{path}/cancel"), None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
}
