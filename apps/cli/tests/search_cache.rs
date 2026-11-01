use serde_json::{Value, json};
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn command(vault: &Path, args: &[&str]) -> Command {
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
        .args(args);
    command
}

fn run(vault: &Path, args: &[&str]) -> Output {
    command(vault, args).output().unwrap()
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
fn first_vault_commands_initialize_before_dispatch_and_keep_stdout_clean() {
    for args in [
        vec!["sessions", "list"],
        vec!["sessions", "get", "a"],
        vec!["sessions", "rename", "a", "Changed"],
        vec!["tags", "list"],
        vec!["sessions", "search", "initial"],
    ] {
        let vault = tempfile::tempdir().unwrap();
        seed(vault.path());
        let output = run(vault.path(), &args);
        let value = response(&output);
        assert_ne!(value["command"], "init");
        let events: Vec<Value> = std::str::from_utf8(&output.stderr)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(events.last().unwrap()["phase"], "ready");
        let cache = hypr_search_cache::Cache::for_vault(vault.path()).unwrap();
        assert!(cache.status().ready);
        if args[1] == "rename" {
            assert_eq!(
                cache
                    .search(hypr_search_cache::SearchRequest {
                        query: "Changed".into(),
                        collection: None,
                        filters: Default::default(),
                        options: Default::default(),
                        limit: 10,
                    })
                    .unwrap()
                    .count,
                1
            );
        }
        let repeat = run(vault.path(), &["sessions", "list"]);
        response(&repeat);
        assert!(repeat.stderr.is_empty());
    }
}

#[test]
fn doctor_does_not_initialize_and_explicit_init_streams_progress_separately() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    let doctor = run(vault.path(), &["doctor"]);
    assert!(!doctor.status.success());
    let report: Value = serde_json::from_slice(&doctor.stdout).unwrap();
    assert_eq!(report["data"]["ready"], false);
    let cache = hypr_search_cache::Cache::for_vault(vault.path()).unwrap();
    assert!(!cache.path().exists());
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
fn mcp_first_start_initializes_without_polluting_the_protocol_stream() {
    use std::io::{BufRead, Write};
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    let stderr = tempfile::NamedTempFile::new().unwrap();
    let mut child = command(vault.path(), &["mcp"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr.reopen().unwrap())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout).lines() {
            let result = line.map_err(|error| error.to_string()).and_then(|line| {
                serde_json::from_str::<Value>(&line).map_err(|error| error.to_string())
            });
            if sender.send(result).is_err() {
                break;
            }
        }
    });
    let responses = (|| -> Result<Vec<Value>, String> {
        writeln!(stdin, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"cache-test","version":"1"}}}))
            .map_err(|error| error.to_string())?;
        let initialized = receiver
            .recv_timeout(std::time::Duration::from_secs(15))
            .map_err(|error| error.to_string())??;
        writeln!(
            stdin,
            "{}",
            json!({"jsonrpc":"2.0","method":"notifications/initialized"})
        )
        .map_err(|error| error.to_string())?;
        writeln!(stdin, "{}", json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_meetings","arguments":{"query":"initial"}}})).map_err(|error| error.to_string())?;
        let searched = receiver
            .recv_timeout(std::time::Duration::from_secs(15))
            .map_err(|error| error.to_string())??;
        Ok(vec![initialized, searched])
    })();
    let _ = child.kill();
    child.wait().unwrap();
    reader.join().unwrap();
    let responses = responses.unwrap();
    assert_eq!(responses[0]["id"], 1);
    assert!(responses[0].get("error").is_none());
    assert_eq!(
        responses[1]["result"]["structuredContent"]["hits"][0]["session_id"],
        "a"
    );
    let events: Vec<Value> = std::fs::read_to_string(stderr.path())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(events.last().unwrap()["phase"], "ready");
}

#[test]
fn incompatible_cache_is_rebuilt_automatically_but_corruption_requires_repair() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    response(&run(vault.path(), &["sessions", "list"]));
    let cache = hypr_search_cache::Cache::for_vault(vault.path()).unwrap();
    let pointer = cache.path().join("CURRENT");
    let meta_path = cache
        .path()
        .join("generations")
        .join(std::fs::read_to_string(&pointer).unwrap())
        .join("meta.json");
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    let mut payload: Value = serde_json::from_str(meta["payload"].as_str().unwrap()).unwrap();
    payload["projection_version"] = json!(0);
    meta["payload"] = json!(payload.to_string());
    std::fs::write(&meta_path, meta.to_string()).unwrap();
    let search = run(vault.path(), &["sessions", "search", "initial"]);
    assert_eq!(response(&search)["data"][0]["session_id"], "a");
    assert!(!search.stderr.is_empty());
    std::fs::write(&pointer, "../invalid").unwrap();
    for args in [vec!["sessions", "list"], vec!["mcp"]] {
        let output = run(vault.path(), &args);
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], "cache_corrupt");
        assert!(
            error["error"]["message"]
                .as_str()
                .unwrap()
                .contains("loof init")
        );
    }
    response(&run(vault.path(), &["init"]));
    assert!(cache.status().ready);
}

#[test]
fn failed_first_initialization_never_dispatches_a_write() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    std::fs::write(vault.path().join("sessions/a/transcript.json"), "malformed").unwrap();
    let output = run(vault.path(), &["sessions", "rename", "a", "Changed"]);
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let meta: Value =
        serde_json::from_slice(&std::fs::read(vault.path().join("sessions/a/_meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["title"], "Original");
    assert!(
        !hypr_search_cache::Cache::for_vault(vault.path())
            .unwrap()
            .status()
            .ready
    );
}

#[test]
fn concurrent_first_commands_share_one_complete_cache() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    let first = command(vault.path(), &["sessions", "search", "initial"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let second = run(vault.path(), &["sessions", "search", "initial"]);
    let first = first.wait_with_output().unwrap();
    for output in [&first, &second] {
        assert_eq!(response(output)["data"][0]["session_id"], "a");
    }
    assert!(
        hypr_search_cache::Cache::for_vault(vault.path())
            .unwrap()
            .status()
            .ready
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
fn initialization_never_creates_a_missing_vault() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    for args in [vec!["init"], vec!["sessions", "list"], vec!["mcp"]] {
        let output = run(&missing, &args);
        assert!(!output.status.success());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], "vault_not_found");
        assert!(!missing.exists());
    }
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
