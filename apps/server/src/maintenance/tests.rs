use super::*;
use serde_json::json;
use std::fs;

fn source(root: &Path, private: bool) -> (PathBuf, String) {
    let storage = root.join("storage");
    fs::create_dir(&storage).unwrap();
    fs::write(
        storage.join("catalog.json"),
        json!({"schema_version":1,"members":{},"vaults":{},"receipts":{}}).to_string(),
    )
    .unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    let vault = storage.join("vaults").join(&id);
    fs::create_dir_all(vault.join("sessions/example/empty-unknown-directory")).unwrap();
    fs::write(
        vault.join("sessions/example/.hidden"),
        b"PRIVATE USER FILE\0\xff",
    )
    .unwrap();
    fs::write(
        vault.join("sessions/example/unrecognized.pdf"),
        b"user attachment",
    )
    .unwrap();
    fs::create_dir_all(storage.join("runtime").join(&id).join("creates/captures")).unwrap();
    fs::write(
        storage
            .join("runtime")
            .join(&id)
            .join("creates/captures/receipt"),
        b"required runtime receipt",
    )
    .unwrap();
    if private {
        let path = storage
            .join("runtime")
            .join(&id)
            .join("creates/provider-credentials");
        fs::create_dir(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        copy::write_private(&path.join("credentials.json"), b"SECRET-CANARY").unwrap();
    }
    (storage, id)
}

use std::path::PathBuf;

#[tokio::test]
async fn backup_requires_both_service_and_vault_ownership_and_does_not_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let (storage, id) = source(root.path(), false);
    let output = root.path().join("backup");
    let secrets = root.path().join("secrets");
    let catalog = Catalog::open(&storage).await.unwrap();
    assert!(
        backup(&storage, &output, &secrets)
            .await
            .unwrap_err()
            .to_string()
            .contains("stop the service")
    );
    assert!(!output.exists());
    drop(catalog);
    let lease = match VaultLease::try_acquire_existing(&storage.join("vaults").join(id)).unwrap() {
        LeaseAttempt::Acquired(lease) => lease,
        LeaseAttempt::Busy => panic!("unexpected owner"),
    };
    assert!(
        backup(&storage, &output, &secrets)
            .await
            .unwrap_err()
            .to_string()
            .contains("stop every vault owner")
    );
    assert!(!output.exists());
    drop(lease);
    let report = backup(&storage, &output, &secrets).await.unwrap();
    assert!(!report.secrets_required);
    verify(&output, None).await.unwrap();
    let before = copy::inventory(&output).unwrap();
    assert!(
        backup(&storage, &output, &root.path().join("other-secret"))
            .await
            .is_err()
    );
    assert_eq!(copy::inventory(&output).unwrap(), before);
    assert!(restore(&output, None, &storage).await.is_err());
}

#[tokio::test]
async fn paired_credentials_are_mandatory_private_and_never_enter_main_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let (storage, id) = source(root.path(), true);
    let output = root.path().join("backup");
    let secrets = root.path().join("secrets");
    backup(&storage, &output, &secrets).await.unwrap();
    let credential = format!("runtime/{id}/creates/provider-credentials/credentials.json");
    assert!(!output.join("data").join(&credential).exists());
    for (name, entry) in copy::inventory(&output).unwrap() {
        if matches!(entry, Entry::File { .. }) {
            assert!(
                !String::from_utf8_lossy(&fs::read(output.join(name)).unwrap())
                    .contains("SECRET-CANARY")
            );
        }
    }
    assert!(
        !String::from_utf8_lossy(&fs::read(secrets.join("manifest.json")).unwrap())
            .contains("SECRET-CANARY")
    );
    assert!(verify(&output, None).await.is_err());
    let target = root.path().join("restored");
    assert!(restore(&output, None, &target).await.is_err());
    assert!(!target.exists());
    let other_output = root.path().join("other-backup");
    let other_secrets = root.path().join("other-secrets");
    backup(&storage, &other_output, &other_secrets)
        .await
        .unwrap();
    assert!(
        restore(&output, Some(&other_secrets), &target)
            .await
            .is_err()
    );
    assert!(!target.exists());
    restore(&output, Some(&secrets), &target).await.unwrap();
    assert_eq!(
        fs::read(target.join(&credential)).unwrap(),
        b"SECRET-CANARY"
    );
    assert_eq!(
        fs::read(target.join(format!("vaults/{id}/sessions/example/.hidden"))).unwrap(),
        b"PRIVATE USER FILE\0\xff"
    );
    assert!(
        target
            .join(format!(
                "vaults/{id}/sessions/example/empty-unknown-directory"
            ))
            .is_dir()
    );
    assert_eq!(
        fs::read(target.join(format!("runtime/{id}/creates/captures/receipt"))).unwrap(),
        b"required runtime receipt"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(target.join(credential))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        fs::set_permissions(&secrets, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(verify(&output, Some(&secrets)).await.is_err());
    }
}

#[tokio::test]
async fn corruption_extra_files_future_formats_and_path_traversal_fail_before_publish() {
    let root = tempfile::tempdir().unwrap();
    let (storage, _) = source(root.path(), false);
    let output = root.path().join("backup");
    let secrets = root.path().join("secrets");
    backup(&storage, &output, &secrets).await.unwrap();
    let target = root.path().join("restored");
    copy::write_private(&output.join("data/extra"), b"extra").unwrap();
    assert!(restore(&output, None, &target).await.is_err());
    fs::remove_file(output.join("data/extra")).unwrap();
    let manifest_path = output.join("manifest.json");
    let original_manifest = fs::read(&manifest_path).unwrap();
    let mut manifest: serde_json::Value = serde_json::from_slice(&original_manifest).unwrap();
    manifest["schema_version"] = json!(999);
    fs::write(&manifest_path, manifest.to_string()).unwrap();
    assert!(
        restore(&output, None, &target)
            .await
            .unwrap_err()
            .to_string()
            .contains("unsupported")
    );
    manifest["schema_version"] = json!(1);
    manifest["entries"]["../escape"] = json!({"kind":"directory","mode":448});
    fs::write(&manifest_path, manifest.to_string()).unwrap();
    assert!(restore(&output, None, &target).await.is_err());
    fs::write(&manifest_path, &original_manifest).unwrap();
    let catalog_path = output.join("data/catalog.json");
    fs::write(
        &catalog_path,
        b"{\"schema_version\":999,\"members\":{},\"vaults\":{},\"receipts\":{}}",
    )
    .unwrap();
    assert!(
        restore(&output, None, &target)
            .await
            .unwrap_err()
            .to_string()
            .contains("digest mismatch")
    );
    // A structurally intact newer backup still cannot downgrade a future catalog.
    let mut manifest: Manifest = serde_json::from_slice(&original_manifest).unwrap();
    manifest.entries = copy::inventory(&output.join("data")).unwrap();
    fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    assert!(
        restore(&output, None, &target)
            .await
            .unwrap_err()
            .to_string()
            .contains("future schema")
    );
    assert!(!target.exists());
    assert!(!root.path().join("escape").exists());
    fs::write(
        storage.join("catalog.json"),
        fs::read(catalog_path).unwrap(),
    )
    .unwrap();
    assert!(
        backup(
            &storage,
            &root.path().join("new"),
            &root.path().join("new-private")
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn interrupted_staging_is_not_live_and_retry_never_replaces_an_existing_tree() {
    let root = tempfile::tempdir().unwrap();
    let (storage, _) = source(root.path(), false);
    let output = root.path().join("backup");
    let secrets = root.path().join("secrets");
    backup(&storage, &output, &secrets).await.unwrap();
    let target = root.path().join("restored");
    let abandoned = copy::stage(&target).unwrap();
    copy::write_private(
        &abandoned.join("catalog.json"),
        b"interrupted partial write",
    )
    .unwrap();
    assert!(!target.exists());
    restore(&output, None, &target).await.unwrap();
    validate_backup_catalog(&target.join("catalog.json")).unwrap();
    assert_eq!(
        fs::read(abandoned.join("catalog.json")).unwrap(),
        b"interrupted partial write"
    );
    let racing = copy::stage(&target).unwrap();
    copy::write_private(&racing.join("canary"), b"must not replace").unwrap();
    assert!(copy::publish(&racing, &target).is_err());
    assert!(!target.join("canary").exists());
    assert!(racing.join("canary").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn linked_or_unsafe_storage_is_rejected_without_dereferencing_or_publishing() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let (storage, _) = source(root.path(), false);
    let output = root.path().join("backup");
    let secrets = root.path().join("secrets");
    let outside = root.path().join("outside");
    fs::write(&outside, b"OUTSIDE-CANARY").unwrap();
    symlink(&outside, storage.join("linked")).unwrap();
    assert!(backup(&storage, &output, &secrets).await.is_err());
    assert!(!output.exists());
    fs::remove_file(storage.join("linked")).unwrap();
    fs::hard_link(&outside, storage.join("linked")).unwrap();
    assert!(backup(&storage, &output, &secrets).await.is_err());
    assert!(!output.exists());
    assert_eq!(fs::read(outside).unwrap(), b"OUTSIDE-CANARY");
    fs::remove_file(storage.join("linked")).unwrap();
    assert!(
        backup(&storage, &storage.join("nested"), &secrets)
            .await
            .is_err()
    );
}
