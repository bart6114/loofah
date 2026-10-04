use std::path::Path;
use std::process::{Command, Stdio};

fn run(vault: &Path, args: &[&str]) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_loof"))
        .args(["--json", "--vault-path"])
        .arg(vault)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn refresh_verifies_without_reindexing_and_repairs_only_changed_sessions() {
    let vault = tempfile::tempdir().unwrap();
    for id in ["a", "b"] {
        let directory = vault.path().join("sessions").join(id);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(
            directory.join("_meta.json"),
            serde_json::json!({
                "id": id, "title": id, "created_at": "2026-01-01T00:00:00Z", "tags": []
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(directory.join("notes.md"), "original").unwrap();
    }
    let built = run(vault.path(), &["cache", "refresh"]);
    assert_eq!(built["command"], "cache.refresh");
    assert_eq!(built["data"]["updated"].as_array().unwrap().len(), 2);
    let verified = run(vault.path(), &["cache", "refresh", "--full"]);
    assert!(verified["data"]["updated"].as_array().unwrap().is_empty());
    std::fs::write(vault.path().join("sessions/b/notes.md"), "changed").unwrap();
    std::fs::write(vault.path().join("sessions/b/private.md"), "unsearchable").unwrap();
    let repaired = run(vault.path(), &["cache", "refresh", "--full"]);
    assert_eq!(repaired["data"]["updated"], serde_json::json!(["b"]));
    let found = run(vault.path(), &["sessions", "search", "changed"]);
    assert_eq!(found["data"][0]["meeting_id"], "b");
    let private = run(vault.path(), &["sessions", "search", "unsearchable"]);
    assert!(private["data"].as_array().unwrap().is_empty());
}
