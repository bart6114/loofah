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
