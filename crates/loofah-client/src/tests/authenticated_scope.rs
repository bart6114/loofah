use super::*;

#[tokio::test]
async fn selected_scope_preserves_shared_vault_and_rejects_profile_login_disconnect_and_cancel_aba()
{
    for change in ["profile", "login", "disconnect", "cancel"] {
        let fixture = Fixture::with_tls(Some(Duration::ZERO)).await;
        let client = fixture.client().await;
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
        let scope = CaptureScope {
            server: client.address().into(),
            member_id: "member".into(),
            vault_id: "shared-vault".into(),
        };
        let cancel = CancellationToken::new();
        let authority = client
            .bind_authenticated_scope(profile.clone(), scope.clone(), cancel.clone())
            .await
            .unwrap();
        assert_eq!(authority.scope(), &scope);
        authority
            .guard_commit()
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
        assert!(authority.guard_commit().await.is_err());
        assert!(authority.check_current().await.is_err());
    }
}

#[tokio::test]
async fn selected_scope_rechecks_original_session_authorization_without_personal_fallback() {
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let fixture=Fixture::with_extra_routes(Some(Duration::ZERO),Router::new().route("/api/v1/vaults/shared-vault/sessions/note",get(move|State(provider):State<Arc<Provider>>|{let seen=seen.clone();async move{
        seen.fetch_add(1,Ordering::SeqCst);
        if provider.denied.load(Ordering::SeqCst) {return (StatusCode::FORBIDDEN,Json(failure(ErrorCode::Forbidden,"Membership revoked"))).into_response();}
        Json(json!({"meta":{"id":"note","title":"Original shared session","created_at":"2026-11-08T00:00:00Z","tags":[]},"note":{"markdown":"Original","revision":"0".repeat(64)}})).into_response()
    }}))).await;
    let client = fixture.client().await;
    fixture.login(&client).await;
    let root = tempfile::tempdir().unwrap();
    let profiles = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
    let profile = profiles.create("export", client.address()).await.unwrap();
    let mut original = profile.profile().await;
    original.member_id = Some("member".into());
    original.vault_id = Some("test".into());
    original
        .credential_accounts
        .push(client.credential_account().into());
    profile.save(original).await.unwrap();
    let authority = client
        .bind_authenticated_scope(
            profile,
            CaptureScope {
                server: client.address().into(),
                member_id: "member".into(),
                vault_id: "shared-vault".into(),
            },
            CancellationToken::new(),
        )
        .await
        .unwrap();
    authority.authorize_session("note").await.unwrap();
    fixture.state.denied.store(true, Ordering::SeqCst);
    assert_eq!(
        authority.authorize_session("note").await.unwrap_err().code,
        ErrorCode::Forbidden
    );
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}
