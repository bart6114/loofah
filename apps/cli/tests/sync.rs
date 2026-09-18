use std::process::Command;

fn cli(home: &std::path::Path) -> Command {
    let home = home.canonicalize().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_loof"));
    command
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join("data"));
    command
        .env_remove("LOOFAH_BASE")
        .env_remove("LOOFAH_VAULT_PATH");
    command
}

#[test]
fn recovery_metadata_is_required_before_normal_reads_and_mcp() {
    let temporary = tempfile::tempdir().unwrap();
    let vault = temporary.path().join("vault");
    std::fs::create_dir_all(vault.join("sessions")).unwrap();
    std::fs::write(vault.join(".loofah-sync-pending"), b"missing journal").unwrap();
    for arguments in [vec!["sessions", "list"], vec!["mcp"]] {
        let output = cli(temporary.path())
            .arg("--json")
            .arg("--vault-path")
            .arg(&vault)
            .args(arguments)
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("recovery")
        );
    }
    assert_eq!(
        std::fs::read(vault.join(".loofah-sync-pending")).unwrap(),
        b"missing journal"
    );
}

#[cfg(feature = "staging")]
#[test]
fn headless_status_and_setup_validation_do_not_need_desktop_or_keyring() {
    let temporary = tempfile::tempdir().unwrap();
    let vault = temporary.path().join("vault");
    std::fs::create_dir_all(vault.join("sessions")).unwrap();
    let output = cli(temporary.path())
        .arg("--json")
        .arg("--vault-path")
        .arg(&vault)
        .args(["sync", "status"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(value.to_string().contains("disconnected"));
    let missing = cli(temporary.path())
        .args(["--json", "sync", "connect"])
        .output()
        .unwrap();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("explicit --vault-path"));
    let unrelated = temporary.path().join("unrelated");
    std::fs::create_dir(&unrelated).unwrap();
    std::fs::write(unrelated.join("keep.txt"), "untouched").unwrap();
    let invalid = cli(temporary.path())
        .arg("--json")
        .arg("--vault-path")
        .arg(&unrelated)
        .args(["sync", "connect"])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert_eq!(
        std::fs::read_to_string(unrelated.join("keep.txt")).unwrap(),
        "untouched"
    );
    assert!(!unrelated.join("sessions").exists());
}

#[cfg(not(feature = "staging"))]
#[test]
fn stable_cli_cannot_enable_sync() {
    let temporary = tempfile::tempdir().unwrap();
    let output = cli(temporary.path())
        .args(["--json", "sync", "connect"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("invitation-only staging"));
}

#[cfg(feature = "staging")]
#[tokio::test(flavor = "multi_thread")]
#[ignore = "requires a fresh disposable account in LOOFAH_SYNC_STAGING_FIXTURE"]
async fn disposable_staging_cli_enrollment_upload_and_history() {
    use vault_sync::{
        credentials::{self, Backend},
        engine::{Binding, atomic_json},
        owner,
        remote::{Environment, RemoteClient},
    };
    use zeroize::Zeroizing;
    let fixture_path = std::env::var_os("LOOFAH_SYNC_STAGING_FIXTURE")
        .expect("protected disposable fixture required");
    let bytes = credentials::file::read_secret(std::path::Path::new(&fixture_path)).unwrap();
    let mut fixture: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        fixture["disposable"], true,
        "fixture must explicitly identify a disposable account"
    );
    let token = Zeroizing::new(fixture["token"].take().as_str().unwrap().to_owned());
    let remote = RemoteClient::new(Environment::Staging, &token, None).unwrap();
    let account: serde_json::Value = remote.get("/account").await.unwrap();
    assert!(
        account["account"]["enrollment_authority"].is_null(),
        "use a fresh disposable account for each platform and run"
    );
    let vault_id = account["account"]["vault_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(account["identity"]["user"], fixture["user"]);
    assert_eq!(account["account"]["vault_id"], fixture["vault"]);
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().canonicalize().unwrap();
    let vault = home.join("vault");
    std::fs::create_dir_all(vault.join("sessions")).unwrap();
    let data = if cfg!(target_os = "macos") {
        home.join("Library/Application Support")
    } else {
        home.join("data")
    };
    let directory = owner::directory(&data.join("io.loofah.sync.staging"), &vault).unwrap();
    let store = credentials::open(
        &directory,
        Environment::Staging,
        Some(Backend::EncryptedFile),
        Some(b"disposable-local-unlock"),
    )
    .unwrap();
    store.save_session(vault_id, token.as_bytes()).unwrap();
    atomic_json(&directory.join("connection.json"), &serde_json::json!({
        "version": 1, "paused": true, "has_started": false,
        "binding": Binding { account: account["identity"]["user"].as_str().unwrap().into(), vault: vault_id, generation: account["account"]["recovery_generation"].as_str().unwrap().parse().unwrap(), local: vault.clone() }
    })).unwrap();
    let unlock = home.join("unlock");
    {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&unlock)
            .unwrap();
        file.write_all(b"disposable-local-unlock").unwrap();
    }
    let run_sync = |arguments: &[&str]| {
        let output = cli(&home)
            .arg("--json")
            .arg("--vault-path")
            .arg(&vault)
            .args(["sync", "--unlock-file"])
            .arg(&unlock)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let kit = home.join("recovery.loofah-key");
    run_sync(&["recovery", "create", kit.to_str().unwrap()]);
    let status = run_sync(&["recovery", "import", kit.to_str().unwrap()]);
    assert_eq!(status["data"]["status"]["hasStarted"], false);
    assert_eq!(status["data"]["status"]["phase"], "paused");
    let note = cli(&home)
        .arg("--json")
        .arg("--vault-path")
        .arg(&vault)
        .args([
            "sessions",
            "new",
            "--title",
            "Disposable CLI staging acceptance",
        ])
        .output()
        .unwrap();
    assert!(note.status.success());
    run_sync(&["once", "--timeout", "300"]);
    let ids: Vec<_> = std::fs::read_dir(vault.join("sessions"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(ids.len(), 1);
    let entity = format!("session:{}", ids[0]);
    let history = run_sync(&["history", "list", &entity]);
    let revision = history["data"]["content"]["versions"][0]["id"]
        .as_str()
        .unwrap();
    run_sync(&["history", "show", &entity, revision]);
    let export = home.join("export");
    run_sync(&[
        "history",
        "export",
        &entity,
        revision,
        export.to_str().unwrap(),
    ]);
    assert!(export.join("_meta.json").exists());
    run_sync(&["disconnect", "--yes"]);
    assert!(
        vault
            .join("sessions")
            .join(&ids[0])
            .join("_meta.json")
            .exists()
    );
    assert!(kit.exists());
}
