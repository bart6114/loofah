use super::*;

async fn profile(
    fixture: &Fixture,
    client: &Arc<RemoteClient>,
    root: &std::path::Path,
) -> Arc<ProfileLease> {
    let profiles = ConnectionProfiles::new(root.join("profiles")).unwrap();
    let profile = profiles
        .create("window", &fixture.state.origin)
        .await
        .unwrap();
    let mut value = profile.profile().await;
    value.member_id = Some("member".into());
    value.vault_id = Some("personal".into());
    value
        .credential_accounts
        .push(client.credential_account().into());
    profile.save(value).await.unwrap();
    profile
}
fn scope(client: &RemoteClient) -> CaptureScope {
    CaptureScope {
        server: client.address().into(),
        member_id: "member".into(),
        vault_id: "test".into(),
    }
}

#[tokio::test]
async fn window_views_share_capacity_but_stop_and_actual_drain_are_independent() {
    let fixture = Fixture::with_tls(Some(Duration::ZERO)).await;
    let client = fixture.client().await;
    fixture.login(&client).await;
    let root = tempfile::tempdir().unwrap();
    let profile = profile(&fixture, &client, root.path()).await;
    let first = client
        .window_view(profile.clone(), scope(&client), CancellationToken::new())
        .await
        .unwrap();
    let second = client
        .window_view(profile, scope(&client), CancellationToken::new())
        .await
        .unwrap();
    assert!(Arc::ptr_eq(&first.upload_writers, &client.upload_writers));
    assert!(Arc::ptr_eq(
        &first.media_source_readers,
        &second.media_source_readers
    ));
    let first_work = first.begin_window_work().unwrap();
    let second_work = second.begin_window_work().unwrap();
    first.disconnect();
    assert!(!client.is_closed());
    assert!(!second.is_closed());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), first.drain_window())
            .await
            .is_err()
    );
    drop(first_work);
    tokio::time::timeout(Duration::from_millis(100), first.drain_window())
        .await
        .unwrap();
    assert!(!second.is_closed());
    drop(second_work);
    client.disconnect();
    tokio::time::timeout(
        Duration::from_secs(1),
        second.window_view_lifetime().unwrap().cancelled(),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn window_view_rejects_wrong_vault_login_provision_and_profile_or_login_aba() {
    for change in ["profile", "login", "lifetime"] {
        let fixture = Fixture::with_tls(Some(Duration::ZERO)).await;
        let client = fixture.client().await;
        fixture.login(&client).await;
        let root = tempfile::tempdir().unwrap();
        let profile = profile(&fixture, &client, root.path()).await;
        let lifetime = CancellationToken::new();
        let view = client
            .window_view(profile.clone(), scope(&client), lifetime.clone())
            .await
            .unwrap();
        assert!(view.endpoint(&["vaults", "test", "sessions"]).is_ok());
        assert_eq!(
            view.endpoint(&["vaults", "personal", "sessions"])
                .unwrap_err()
                .code,
            ErrorCode::Forbidden
        );
        assert_eq!(
            view.endpoint(&["vaults"]).unwrap_err().code,
            ErrorCode::Forbidden
        );
        assert!(view.begin_login().await.is_err());
        assert!(view.logout().await.is_err());
        assert!(
            view.execute(
                RemoteOperation::Provision,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .await
            .is_err()
        );
        let (send, mut receive) = mpsc::unbounded_channel();
        let subscription = view
            .subscribe("test".into(), move |event| send.send(event).is_ok())
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !matches!(
                receive.recv().await,
                Some(RemoteEvent::Connection {
                    state: ConnectionState::Connected,
                    ..
                })
            ) {}
        })
        .await
        .unwrap();
        match change {
            "profile" => {
                let old = profile.profile().await;
                let mut other = old.clone();
                other.member_id = Some("other".into());
                profile.save(other).await.unwrap();
                profile.save(old).await.unwrap();
            }
            "login" => fixture.login(&client).await,
            _ => lifetime.cancel(),
        }
        tokio::time::timeout(
            Duration::from_secs(1),
            view.window_view_lifetime().unwrap().cancelled(),
        )
        .await
        .unwrap();
        assert!(view.is_closed());
        assert!(view.window_dispatch_guard().await.is_err());
        assert!(
            view.execute(
                RemoteOperation::Me,
                &uuid::Uuid::new_v4().to_string(),
                CancellationToken::new()
            )
            .await
            .is_err()
        );
        subscription.close().await;
        tokio::time::timeout(Duration::from_secs(1), view.drain_window())
            .await
            .unwrap();
        assert!(!client.is_closed());
    }
}
