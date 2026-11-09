use super::*;

#[tokio::test]
async fn append_dispatch_releases_profile_while_concurrent_refresh_waits_for_credentials() {
    let fixture = Fixture::with_tls(Some(Duration::ZERO)).await;
    let mut client = fixture.client().await;
    Arc::get_mut(&mut client)
        .unwrap()
        .capabilities
        .features
        .push("capture_ingestion".into());
    fixture.login(&client).await;
    let root = tempfile::tempdir().unwrap();
    let profiles = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
    let profile = profiles.create("append", client.address()).await.unwrap();
    let mut selected = profile.profile().await;
    selected.member_id = Some("member".into());
    selected.vault_id = Some("vault".into());
    selected
        .credential_accounts
        .push(client.credential_account().into());
    profile.save(selected).await.unwrap();
    let request = CaptureAppendRequest {
        scope: CaptureScope {
            server: client.address().into(),
            member_id: "member".into(),
            vault_id: "vault".into(),
        },
        session_id: "session".into(),
        capture_id: uuid::Uuid::new_v4().to_string(),
        request_id: uuid::Uuid::new_v4().to_string(),
        admission_id: uuid::Uuid::new_v4().to_string(),
        processing: None,
    };
    let authority = client
        .bind_capture_append(profile.clone(), request.clone(), CancellationToken::new())
        .await
        .unwrap();
    let generation = profile.generation().await.unwrap();
    let original = generation.profile().clone();
    let gate = profile.guard_generation(&generation).await.unwrap();
    let dispatch = async { authority.guard_dispatch().await.map(|_| ()) };
    tokio::pin!(dispatch);
    assert!(futures_util::poll!(&mut dispatch).is_pending());
    let access = client.access_token().await.unwrap();
    client.expire_access(&access).await;
    let saves = fixture.credentials.saves.load(Ordering::SeqCst);
    let credentials = fixture.credentials.values.lock().await;
    let refresh = tokio::spawn({
        let client = client.clone();
        async move { client.access_token().await }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.credentials.saves.load(Ordering::SeqCst) == saves {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(client.tokens.try_lock().is_err());
    drop(gate);
    assert!(futures_util::poll!(&mut dispatch).is_pending());
    let update = async {
        // Refresh cannot finish until after this save; no timing race releases it.
        tokio::time::timeout(Duration::from_secs(2), profile.save(original))
            .await
            .unwrap()
            .unwrap();
        drop(credentials);
        tokio::time::timeout(Duration::from_secs(2), refresh)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    };
    let (dispatch, ()) = tokio::join!(
        tokio::time::timeout(Duration::from_secs(5), dispatch),
        update
    );
    assert_eq!(dispatch.unwrap().unwrap_err().code, ErrorCode::Conflict);
    assert_eq!(authority.request(), &request);
    assert_eq!(fixture.state.refreshes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn append_authority_rejects_profile_and_login_aba_disconnect_and_window_cancellation() {
    for change in ["profile", "login", "disconnect", "cancel"] {
        let fixture = Fixture::with_tls(Some(Duration::ZERO)).await;
        let mut client = fixture.client().await;
        Arc::get_mut(&mut client)
            .unwrap()
            .capabilities
            .features
            .push("capture_ingestion".into());
        fixture.login(&client).await;
        let root = tempfile::tempdir().unwrap();
        let profiles = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
        let profile = profiles.create("append", client.address()).await.unwrap();
        let mut selected = profile.profile().await;
        selected.member_id = Some("member".into());
        selected.vault_id = Some("vault".into());
        selected
            .credential_accounts
            .push(client.credential_account().into());
        profile.save(selected).await.unwrap();
        let request = CaptureAppendRequest {
            scope: CaptureScope {
                server: client.address().into(),
                member_id: "member".into(),
                vault_id: "vault".into(),
            },
            session_id: "session".into(),
            capture_id: uuid::Uuid::new_v4().to_string(),
            request_id: uuid::Uuid::new_v4().to_string(),
            admission_id: uuid::Uuid::new_v4().to_string(),
            processing: None,
        };
        let cancel = CancellationToken::new();
        let authority = client
            .bind_capture_append(profile.clone(), request.clone(), cancel.clone())
            .await
            .unwrap();
        assert_eq!(authority.request(), &request);
        authority
            .guard_dispatch()
            .await
            .unwrap()
            .check_current()
            .unwrap();
        match change {
            "profile" => {
                let original = profile.profile().await;
                let mut changed = original.clone();
                changed.member_id = Some("other".into());
                profile.save(changed).await.unwrap();
                profile.save(original).await.unwrap();
            }
            "login" => fixture.login(&client).await,
            "cancel" => cancel.cancel(),
            _ => client.disconnect(),
        }
        assert!(authority.guard_dispatch().await.is_err());
        assert!(authority.check_current().await.is_err());
    }
}
