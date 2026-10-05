use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output};

fn run(vault: &Path, args: &[&str]) -> Output {
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
        .arg("--json")
        .arg("--vault-path")
        .arg(vault)
        .args(args)
        .output()
        .unwrap()
}

fn seed(vault: &Path) {
    let dir = vault.join("sessions/a");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("_meta.json"), json!({"id":"a","title":"Original","created_at":"2026-10-01T12:00:00Z","started_at":null,"ended_at":null,"tags":[]}).to_string()).unwrap();
    std::fs::write(dir.join("notes.md"), "initial content").unwrap();
}

fn response(output: &Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn all_vault_commands_are_gated_and_init_streams_progress_separately() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    for args in [
        vec!["sessions", "list"],
        vec!["sessions", "get", "a"],
        vec!["sessions", "rename", "a", "Changed"],
        vec!["tags", "list"],
        vec!["mcp"],
    ] {
        let output = run(vault.path(), &args);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], "cache_not_ready");
    }
    let doctor = run(vault.path(), &["doctor"]);
    assert!(!doctor.status.success());
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(report["data"]["ready"], false);
    let init = run(vault.path(), &["init"]);
    let value = response(&init);
    assert_eq!(value["schema_version"], "2");
    assert_eq!(value["data"]["ready"], true);
    let events: Vec<Value> = std::str::from_utf8(&init.stderr)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert!(events.iter().any(|event| event["phase"] == "index"));
    assert_eq!(events.last().unwrap()["phase"], "ready");
    let repeat = run(vault.path(), &["init"]);
    assert_eq!(response(&repeat)["data"]["updated"], 0);
    assert_eq!(
        response(&run(vault.path(), &["doctor"]))["data"]["ready"],
        true
    );
}

#[test]
fn cli_reuses_desktop_created_cache_and_reconciles_external_edits() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    let desktop = hypr_search_cache::Cache::for_vault(vault.path()).unwrap();
    desktop.initialize(&mut |_| {}).unwrap();
    std::fs::write(vault.path().join("sessions/a/notes.md"), "external zebra").unwrap();
    let output = run(vault.path(), &["sessions", "search", "zebra"]);
    let hits = response(&output);
    assert_eq!(hits["data"][0]["session_id"], "a");
    assert!(hits["data"][0]["score"].as_f64().unwrap() > 0.0);
    assert!(hits["data"][0].get("kind").is_none());
    assert_eq!(
        response(&run(
            vault.path(),
            &["sessions", "list", "--query", "zebra"]
        ))["data"][0]["id"],
        "a"
    );
    let write = run(vault.path(), &["sessions", "rename", "a", "Newtitle"]);
    assert_eq!(response(&write)["data"]["title"], "Newtitle");
    let hits = desktop
        .search(hypr_search_cache::SearchRequest {
            query: "Newtitle".into(),
            collection: None,
            filters: Default::default(),
            options: Default::default(),
            limit: 10,
        })
        .unwrap();
    assert_eq!(hits.count, 1);
}

#[test]
fn reconciliation_failure_reports_persisted_write_without_success_output() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    response(&run(vault.path(), &["init"]));
    std::fs::write(vault.path().join("sessions/a/transcript.json"), "malformed").unwrap();
    let write = run(vault.path(), &["sessions", "rename", "a", "Persisted"]);
    assert!(!write.status.success());
    assert!(write.stdout.is_empty());
    let error: Value = serde_json::from_slice(&write.stderr).unwrap();
    assert_eq!(error["error"]["write_persisted"], true);
    assert_eq!(error["error"]["session_ids"], json!(["a"]));
    let meta: Value =
        serde_json::from_slice(&std::fs::read(vault.path().join("sessions/a/_meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["title"], "Persisted");
    assert!(
        !run(vault.path(), &["sessions", "search", "Persisted"])
            .status
            .success()
    );
    std::fs::remove_file(vault.path().join("sessions/a/transcript.json")).unwrap();
    response(&run(vault.path(), &["init"]));
    assert_eq!(
        response(&run(vault.path(), &["sessions", "search", "Persisted"]))["data"][0]["session_id"],
        "a"
    );
}

#[test]
fn init_never_creates_a_missing_vault() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    let output = run(&missing, &["init"]);
    assert!(!output.status.success());
    let error: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(error["error"]["code"], "vault_not_found");
    assert!(!missing.exists());
}

#[test]
fn direct_writes_reconcile_only_the_affected_sessions() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    response(&run(vault.path(), &["init"]));
    let broken = vault.path().join("sessions/broken");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("_meta.json"), "invalid JSON").unwrap();
    response(&run(vault.path(), &["sessions", "rename", "a", "Renamed"]));
    response(&run(vault.path(), &["sessions", "delete", "a"]));
    assert!(broken.join("_meta.json").exists());
    assert!(!run(vault.path(), &["sessions", "list"]).status.success());
}
