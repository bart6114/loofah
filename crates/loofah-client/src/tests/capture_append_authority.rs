use super::*;

#[tokio::test]
async fn append_authority_rejects_profile_and_login_aba_disconnect_and_window_cancellation() {
    for change in ["profile", "login", "disconnect", "cancel"] {
        let fixture = Fixture::new().await;
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
