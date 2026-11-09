use super::*;

#[tokio::test]
async fn transcription_policy_is_reader_scoped_and_does_not_wait_for_cold_vault_startup() {
    struct ReleaseStartup {
        service: Arc<Service>,
        barrier: Option<Arc<std::sync::Barrier>>,
    }
    impl Drop for ReleaseStartup {
        fn drop(&mut self) {
            if let Some(barrier) = self.barrier.take()
                && self.service.startup_pause.lock().unwrap().take().is_none()
            {
                std::thread::spawn(move || barrier.wait());
            }
        }
    }
    let fixture = Fixture::new().await;
    let alice = fixture.login("alice").await;
    let bob = fixture.login("bob").await;
    let charlie = fixture.login("charlie").await;
    // Eager readiness also opens vaults. Drain unrelated account startup before
    // installing the one-shot pause for the newly created target vault.
    for id in [
        &alice.personal_vault_id,
        &bob.personal_vault_id,
        &charlie.personal_vault_id,
    ] {
        fixture.service.vault(id).await.unwrap();
    }
    let (started, observed) = std::sync::mpsc::channel();
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let mut release = ReleaseStartup {
        service: fixture.service.clone(),
        barrier: Some(barrier.clone()),
    };
    *fixture.service.startup_pause.lock().unwrap() = Some((started, barrier));
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
    let startup = {
        let service = fixture.service.clone();
        let id = id.to_owned();
        tokio::spawn(async move { service.vault(&id).await })
    };
    tokio::task::spawn_blocking(move || observed.recv_timeout(std::time::Duration::from_secs(10)))
        .await
        .unwrap()
        .expect("Target startup must reach its physical pause");
    let path = format!("/api/v1/vaults/{id}/transcription/policy");
    let response = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        fixture.call("bob", "GET", &path, None),
    )
    .await;
    let stayed_blocked = !startup.is_finished();
    let barrier = release.barrier.take().unwrap();
    tokio::task::spawn_blocking(move || barrier.wait())
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(10), startup)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let (status, policy) = response.expect("Policy must not wait for vault startup");
    assert!(
        stayed_blocked,
        "Target startup must remain paused until policy returns"
    );
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
