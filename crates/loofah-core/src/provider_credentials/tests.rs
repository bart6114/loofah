use super::*;
use crate::VaultRuntime;

fn policy(endpoint: &str, key: Option<&str>) -> ProviderCredentialPolicy {
    ProviderCredentialPolicy {
        provider_id: "openai".into(),
        endpoint: endpoint.into(),
        operator_key: key.map(str::to_owned),
    }
}

async fn runtime(root: &Path) -> VaultRuntime {
    VaultRuntime::open_with_changes(
        "vault".into(),
        &root.join("vault"),
        &root.join("cache"),
        &root.join("operations"),
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap()
}

async fn save(
    owner: &Arc<ProviderCredentials>,
    revision: &str,
    key: Option<&str>,
) -> ProviderCredentialStatus {
    match owner
        .save(
            "openai".into(),
            SaveProviderCredentialRequest {
                expected_revision: revision.into(),
                api_key: key.map(str::to_owned),
            },
        )
        .await
        .unwrap()
    {
        ProviderCredentialSaveOutcome::Saved(status) => status,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn owner_credentials_are_private_cas_guarded_and_clear_restores_operator_fallback() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path()).await;
    let owner = runtime
        .context
        .configure_provider_credentials(vec![policy(
            "https://api.openai.com/v1",
            Some("operator-secret"),
        )])
        .await
        .unwrap();
    let initial = owner.status("openai".into()).await.unwrap();
    assert_eq!(initial.source, ProviderCredentialSource::Operator);
    uuid::Uuid::parse_str(&initial.revision).unwrap();
    let saved = save(&owner, &initial.revision, Some("vault-secret")).await;
    assert_eq!(saved.source, ProviderCredentialSource::Vault);
    assert_ne!(saved.revision, initial.revision);
    assert_eq!(
        owner.resolve("openai".into()).await.unwrap().as_deref(),
        Some("vault-secret")
    );
    let conflict = owner
        .save(
            "openai".into(),
            SaveProviderCredentialRequest {
                expected_revision: initial.revision,
                api_key: Some("stale-secret".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        conflict,
        ProviderCredentialSaveOutcome::Conflict(saved.clone())
    );
    assert!(!serde_json::to_string(&conflict).unwrap().contains("secret"));
    let cleared = save(&owner, &saved.revision, None).await;
    assert_eq!(cleared.source, ProviderCredentialSource::Operator);
    assert_eq!(
        owner.resolve("openai".into()).await.unwrap().as_deref(),
        Some("operator-secret")
    );
    assert!(
        !std::fs::read_to_string(owner.path())
            .unwrap()
            .contains("secret")
    );
    assert!(!root.path().join("vault/provider-credentials").exists());
    let request = SaveProviderCredentialRequest {
        expected_revision: cleared.revision,
        api_key: Some("debug-secret".into()),
    };
    assert!(!format!("{request:?}").contains("debug-secret"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&owner.root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(owner.path())
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn stored_overrides_survive_restart_but_never_follow_changed_operator_endpoints() {
    let root = tempfile::tempdir().unwrap();
    let first = runtime(root.path()).await;
    let owner = first
        .context
        .configure_provider_credentials(vec![policy("https://api.openai.com/v1/", None)])
        .await
        .unwrap();
    let initial = owner.status("openai".into()).await.unwrap();
    assert_eq!(initial.source, ProviderCredentialSource::Missing);
    let saved = save(&owner, &initial.revision, Some("vault-secret")).await;
    drop(owner);
    first.shutdown().await.unwrap();
    let second = runtime(root.path()).await;
    let owner = second
        .context
        .configure_provider_credentials(vec![policy("https://API.OPENAI.COM:443/v1", None)])
        .await
        .unwrap();
    assert_eq!(owner.status("openai".into()).await.unwrap(), saved);
    assert_eq!(
        owner.resolve("openai".into()).await.unwrap().as_deref(),
        Some("vault-secret")
    );
    drop(owner);
    second.shutdown().await.unwrap();
    let third = runtime(root.path()).await;
    let owner = third
        .context
        .configure_provider_credentials(vec![policy(
            "https://other.example/v1",
            Some("new-operator-secret"),
        )])
        .await
        .unwrap();
    let status = owner.status("openai".into()).await.unwrap();
    assert!(!status.endpoint_matches);
    assert_eq!(status.source, ProviderCredentialSource::Operator);
    assert_ne!(status.revision, saved.revision);
    assert!(matches!(
        owner
            .save(
                "openai".into(),
                SaveProviderCredentialRequest {
                    expected_revision: saved.revision,
                    api_key: Some("stale-endpoint-secret".into())
                }
            )
            .await
            .unwrap(),
        ProviderCredentialSaveOutcome::Conflict(_)
    ));
    assert_eq!(
        owner.resolve("openai".into()).await.unwrap().as_deref(),
        Some("new-operator-secret")
    );
    let rebound = save(&owner, &status.revision, Some("new-vault-secret")).await;
    assert!(rebound.endpoint_matches);
    assert_eq!(
        owner.resolve("openai".into()).await.unwrap().as_deref(),
        Some("new-vault-secret")
    );
    third.shutdown().await.unwrap();
}

#[tokio::test]
async fn concurrent_saves_have_one_winner_and_invalid_input_never_changes_the_revision() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path()).await;
    let owner = runtime
        .context
        .configure_provider_credentials(vec![policy("https://api.openai.com/v1", None)])
        .await
        .unwrap();
    let initial = owner.status("openai".into()).await.unwrap();
    for key in ["", "bad\r\nkey", " whitespace", "non-ascii-ä"] {
        let error = owner
            .save(
                "openai".into(),
                SaveProviderCredentialRequest {
                    expected_revision: initial.revision.clone(),
                    api_key: Some(key.into()),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert!(!error.message.contains(key) || key.is_empty());
    }
    assert_eq!(owner.status("openai".into()).await.unwrap(), initial);
    let a = owner.save(
        "openai".into(),
        SaveProviderCredentialRequest {
            expected_revision: initial.revision.clone(),
            api_key: Some("first".into()),
        },
    );
    let b = owner.save(
        "openai".into(),
        SaveProviderCredentialRequest {
            expected_revision: initial.revision,
            api_key: Some("second".into()),
        },
    );
    let (a, b) = tokio::join!(a, b);
    assert!(matches!(
        (a.unwrap(), b.unwrap()),
        (
            ProviderCredentialSaveOutcome::Saved(_),
            ProviderCredentialSaveOutcome::Conflict(_)
        ) | (
            ProviderCredentialSaveOutcome::Conflict(_),
            ProviderCredentialSaveOutcome::Saved(_)
        )
    ));
    assert_eq!(
        owner.status("unknown".into()).await.unwrap_err().code,
        ErrorCode::Validation
    );
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn future_corrupt_or_other_vault_storage_fails_closed_without_falling_back() {
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path()).await;
    let owner = runtime
        .context
        .configure_provider_credentials(vec![policy(
            "https://api.openai.com/v1",
            Some("operator-secret"),
        )])
        .await
        .unwrap();
    let original = std::fs::read_to_string(owner.path()).unwrap();
    for invalid in [
        "{broken".to_owned(),
        original.replace("\"version\":1", "\"version\":2"),
        original.replace("\"vault_id\":\"vault\"", "\"vault_id\":\"other\""),
    ] {
        hypr_storage::fs::atomic_write(&owner.path(), &invalid).unwrap();
        let error = owner.resolve("openai".into()).await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Unavailable);
        assert!(!error.message.contains("operator-secret"));
    }
    runtime.shutdown().await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn symlinks_shared_files_and_public_permissions_are_rejected_before_secret_reads() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let runtime = runtime(root.path()).await;
    let owner = runtime
        .context
        .configure_provider_credentials(vec![policy(
            "https://api.openai.com/v1",
            Some("operator-secret"),
        )])
        .await
        .unwrap();
    std::fs::set_permissions(owner.path(), std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(owner.resolve("openai".into()).await.is_err());
    std::fs::set_permissions(owner.path(), std::fs::Permissions::from_mode(0o600)).unwrap();
    let outside = root.path().join("outside.json");
    std::fs::hard_link(owner.path(), &outside).unwrap();
    assert!(owner.resolve("openai".into()).await.is_err());
    std::fs::remove_file(owner.path()).unwrap();
    symlink(&outside, owner.path()).unwrap();
    assert!(owner.resolve("openai".into()).await.is_err());
    assert!(owner.status("openai".into()).await.is_err());
    runtime.shutdown().await.unwrap();
}

#[test]
fn missing_secret_field_is_not_an_implicit_clear() {
    assert!(
        serde_json::from_value::<SaveProviderCredentialRequest>(
            serde_json::json!({"expected_revision":uuid::Uuid::new_v4().to_string()})
        )
        .is_err()
    );
    assert!(
        serde_json::from_value::<SaveProviderCredentialRequest>(
            serde_json::json!({"expected_revision":uuid::Uuid::new_v4().to_string(),"api_key":null})
        )
        .is_ok()
    );
}

#[tokio::test]
async fn cancelled_waiter_keeps_original_vault_lease_until_real_credential_io_finishes() {
    use hypr_storage::vault::{LeaseAttempt, VaultLease};
    let root = tempfile::tempdir().unwrap();
    let opened = runtime(root.path()).await;
    let owner = opened
        .context
        .configure_provider_credentials(vec![policy("https://api.openai.com/v1", None)])
        .await
        .unwrap();
    let initial = owner.status("openai".into()).await.unwrap();
    let (entered, entering) = tokio::sync::oneshot::channel();
    let (resume, resumed) = std::sync::mpsc::channel();
    let (done, finished) = tokio::sync::oneshot::channel();
    let work = owner.clone();
    let waiter = tokio::spawn(async move {
        work.run(move |owner| {
            entered.send(()).unwrap();
            resumed.recv().unwrap();
            let mut stored = owner.read()?;
            let record = stored.records.get_mut("openai").unwrap();
            assert_eq!(record.revision, initial.revision);
            record.revision = uuid::Uuid::new_v4().to_string();
            record.api_key = Some("owned-secret".into());
            owner.persist(&stored)?;
            done.send(()).unwrap();
            Ok(())
        })
        .await
    });
    entering.await.unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    let lease = Arc::downgrade(&owner._lease);
    drop(owner);
    opened.shutdown().await.unwrap();
    assert!(matches!(
        VaultLease::try_acquire_existing(&root.path().join("vault")).unwrap(),
        LeaseAttempt::Busy
    ));
    resume.send(()).unwrap();
    finished.await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while lease.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let opened = runtime(root.path()).await;
    let owner = opened
        .context
        .configure_provider_credentials(vec![policy("https://api.openai.com/v1", None)])
        .await
        .unwrap();
    assert_eq!(
        owner.resolve("openai".into()).await.unwrap().as_deref(),
        Some("owned-secret")
    );
    opened.shutdown().await.unwrap();
}
