use super::*;

#[tokio::test]
async fn transcription_policy_is_reader_scoped_disabled_without_host_and_never_opens_vault() {
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    fixture.login("charlie").await;
    let (_, vault) = fixture
        .call(
            "alice",
            "POST",
            "/api/v1/vaults",
            Some(json!({"name":"Policy"})),
        )
        .await;
    let id = vault["id"].as_str().unwrap();
    assert_eq!(
        fixture
            .call(
                "alice",
                "PUT",
                &format!("/api/v1/vaults/{id}/members/{}", bob.member.id),
                Some(json!({"role":"reader","expected_revision":vault["revision"]}))
            )
            .await
            .0,
        StatusCode::OK
    );
    let (started, observed) = std::sync::mpsc::channel();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    *fixture.service.startup_pause.lock().unwrap() = Some((started, barrier.clone()));
    let path = format!("/api/v1/vaults/{id}/transcription/policy");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        fixture.call("bob", "GET", &path, None),
    )
    .await;
    let opened = observed.try_recv().is_ok();
    fixture.service.startup_pause.lock().unwrap().take();
    if opened {
        tokio::task::spawn_blocking(move || barrier.wait())
            .await
            .unwrap();
    }
    let (status, policy) = response.expect("Policy must not wait for vault startup");
    assert!(!opened, "Policy opened a cold vault");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        policy,
        json!({"model":null,"availability":"disabled","batch_enabled":false,"live_enabled":false})
    );
    assert_eq!(
        fixture.call("charlie", "GET", &path, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        fixture
            .call(
                "bob",
                "GET",
                &format!(
                    "/api/v1/vaults/{}/transcription/policy",
                    alice.personal_vault_id
                ),
                None
            )
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, capabilities) = fixture
        .call("bob", "GET", "/api/v1/capabilities", None)
        .await;
    assert!(
        capabilities["features"]
            .as_array()
            .unwrap()
            .contains(&json!("transcription_policy"))
    );
    fixture.service.startup_pause.lock().unwrap().take();
    fixture.service.shutdown().await.unwrap();
}
