use std::{path::Path, process::Command};

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_loofah-server"));
    command.current_dir(root);
    command
}

#[test]
fn maintenance_commands_work_without_config_authentication_or_cache() {
    let root = tempfile::tempdir().unwrap();
    let storage = root.path().join("storage");
    std::fs::create_dir(&storage).unwrap();
    std::fs::write(
        storage.join("catalog.json"),
        br#"{"schema_version":1,"members":{},"vaults":{},"receipts":{}}"#,
    )
    .unwrap();
    let output = command(root.path())
        .args([
            "backup",
            "--storage-root",
            "storage",
            "--output",
            "snapshot",
            "--secrets-output",
            "secrets",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["secrets_required"], false);
    let output = command(root.path())
        .args([
            "verify-backup",
            "--snapshot",
            "snapshot",
            "--secrets",
            "secrets",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = command(root.path())
        .args([
            "restore",
            "--snapshot",
            "snapshot",
            "--storage-root",
            "restored",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        std::fs::read(root.path().join("restored/catalog.json")).unwrap(),
        std::fs::read(storage.join("catalog.json")).unwrap()
    );
    assert!(!root.path().join("cache").exists());
    assert!(
        !command(root.path())
            .args([
                "restore",
                "--snapshot",
                "snapshot",
                "--storage-root",
                "restored"
            ])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn server_start_still_requires_config_and_maintenance_rejects_mixed_startup() {
    let root = tempfile::tempdir().unwrap();
    assert!(!command(root.path()).output().unwrap().status.success());
    assert!(
        !command(root.path())
            .args([
                "--config",
                "missing.json",
                "backup",
                "--storage-root",
                "storage",
                "--output",
                "snapshot",
                "--secrets-output",
                "secrets"
            ])
            .output()
            .unwrap()
            .status
            .success()
    );
    let output = command(root.path())
        .args(["backup", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--secrets-output"));
}

#[cfg(unix)]
#[path = "fixtures/maintenance_acceptance.rs"]
mod acceptance;

#[tokio::test]
async fn adoption_cli_preserves_provisioned_catalog_and_refuses_existing_output() {
    let root = tempfile::tempdir().unwrap();
    let storage = root.path().join("storage");
    let catalog = loofah_server::catalog::Catalog::open(&storage)
        .await
        .unwrap();
    let identity = loofah_server::auth::Identity {
        issuer: "https://company.example".into(),
        subject: "original-owner".into(),
        name: "Owner".into(),
        expires_at: u64::MAX,
    };
    catalog.provision(&identity).await.unwrap();
    let vault = catalog
        .create_shared(&identity, &uuid::Uuid::new_v4().to_string(), "Empty target")
        .await
        .unwrap();
    drop(catalog);
    let source = root.path().join("local");
    std::fs::create_dir_all(source.join("sessions/original")).unwrap();
    std::fs::write(
        source.join("sessions/original/notes.md"),
        b"Original Local note\n",
    )
    .unwrap();
    std::fs::write(
        source.join("sessions/original/unknown.bin"),
        b"unknown bytes",
    )
    .unwrap();
    let run = || {
        command(root.path())
            .args([
                "adopt-vault",
                "--storage-root",
                "storage",
                "--vault-id",
                &vault.id,
                "--source-vault",
                "local",
                "--output",
                "adopted",
            ])
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["vault_id"], vault.id);
    assert_eq!(
        std::fs::read(root.path().join("adopted/catalog.json")).unwrap(),
        std::fs::read(storage.join("catalog.json")).unwrap()
    );
    assert_eq!(
        std::fs::read(
            root.path()
                .join("adopted/vaults")
                .join(&vault.id)
                .join("sessions/original/unknown.bin")
        )
        .unwrap(),
        b"unknown bytes"
    );
    assert!(!run().status.success());
    assert!(!root.path().join("cache").exists());
}
