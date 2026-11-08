use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn run(root: &std::path::Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_loof"));
    for name in [
        "LOOFAH_BASE",
        "LOOFAH_VAULT_PATH",
        "FMTR_BASE",
        "FMTR_VAULT_PATH",
    ] {
        command.env_remove(name);
    }
    command
        .env("LOOFAH_CLI_PROFILE_DIR", root.join("profiles"))
        .env("LOOFAH_GLOBAL_BASE", root.join("local-global"))
        .stdin(Stdio::null())
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn profile_management_and_remote_failures_never_initialize_a_local_vault() {
    let root = tempfile::tempdir().unwrap();
    let list = run(root.path(), &["--json", "profiles", "list"]);
    assert!(list.status.success(), "{list:?}");
    assert!(list.stderr.is_empty(), "{list:?}");
    let json: Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(json["schema_version"], "2");
    assert_eq!(json["command"], "profiles.list");
    assert_eq!(json["data"], serde_json::json!([]));
    assert!(!root.path().join("profiles").exists());

    let missing = run(
        root.path(),
        &["--json", "--profile", "missing", "sessions", "list"],
    );
    assert_eq!(missing.status.code(), Some(2), "{missing:?}");
    let json: Value = serde_json::from_slice(&missing.stderr).unwrap();
    assert_eq!(json["error"]["code"], "not_found");
    assert!(!root.path().join("profiles").exists());

    for command in ["doctor", "init"] {
        let args = vec!["--json", "--profile", "missing", command];
        let output = run(root.path(), &args);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        let json: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("No local vault was accessed")
        );
        assert!(!root.path().join("profiles").exists());
    }
    for command in ["import", "transcribe"] {
        let output = run(
            root.path(),
            &["--json", "--profile", "missing", command, "missing"],
        );
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        let json: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(json["error"]["code"], "not_found");
        assert_eq!(json["error"]["message"], "Saved server profile not found");
        assert!(!root.path().join("profiles").exists());
    }
    assert!(!root.path().join("local-global").exists());
    assert!(!root.path().join("local-vault").exists());
}

#[test]
fn creating_a_profile_requires_an_explicit_https_server() {
    let root = tempfile::tempdir().unwrap();
    for args in [
        vec!["--json", "profiles", "login", "company"],
        vec![
            "--json",
            "profiles",
            "login",
            "company",
            "--server",
            "http://company.example",
        ],
    ] {
        let output = run(root.path(), &args);
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(
            serde_json::from_slice::<Value>(&output.stderr)
                .unwrap()
                .get("error")
                .is_some()
        );
        assert!(!root.path().join("profiles").exists());
    }
}
