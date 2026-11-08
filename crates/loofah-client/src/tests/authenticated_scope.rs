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
