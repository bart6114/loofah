use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

fn run(vault: &Path, args: &[&str], json: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_loof"));
    for name in [
        "LOOFAH_BASE",
        "LOOFAH_VAULT_PATH",
        "FMTR_BASE",
        "FMTR_VAULT_PATH",
    ] {
        command.env_remove(name);
    }
    command.stdin(Stdio::null()).arg("--vault-path").arg(vault);
    if json {
        command.arg("--json");
    }
    command.args(args).output().unwrap()
}

fn response(output: Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schema_version"], "1");
    value
}

fn seed(vault: &Path) -> PathBuf {
    let path = vault.join("sessions/session-1");
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        path.join("_meta.json"),
        json!({
            "id": "session-1", "title": "Original", "tags": ["work"],
            "created_at": "2026-09-21T10:00:00Z",
            "started_at": "2026-09-21T10:00:00Z", "ended_at": null,
            "folder": "team", "author": "agent", "skill": "notes",
            "tracking_id": "tracking-1", "future_field": {"preserve": true}
        })
        .to_string(),
    )
    .unwrap();
    path
}

#[test]
fn rename_preserves_metadata_and_files_and_is_visible_to_read_commands() {
    let vault = tempfile::tempdir().unwrap();
    let session = seed(vault.path());
    let meta_path = session.join("_meta.json");
    let mut expected: Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
    let files = [
        "notes.md",
        "summary.md",
        "transcript.json",
        "tasks.json",
        "audio.mp3",
        "attachments/photo.png",
        "enhanced/output.md",
        "unknown.bin",
        ".hidden",
    ];
    for name in files {
        let path = session.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let body = match name {
            "transcript.json" => "{\"words\":[],\"speaker_hints\":[]}",
            "tasks.json" => "[]",
            _ => "untouched",
        };
        std::fs::write(path, body).unwrap();
    }
    let before: Vec<_> = files
        .iter()
        .map(|name| std::fs::read(session.join(name)).unwrap())
        .collect();
    let title = "Résumé \"Planning\" 🦆";
    let result = response(run(
        vault.path(),
        &["sessions", "rename", "session-1", title],
        true,
    ));
    assert_eq!(result["command"], "sessions.rename");
    assert_eq!(result["data"], json!({"id": "session-1", "title": title}));
    expected["title"] = json!(title);
    let after: Value = serde_json::from_slice(&std::fs::read(meta_path).unwrap()).unwrap();
    assert_eq!(after, expected);
    for (name, bytes) in files.iter().zip(before) {
        assert_eq!(std::fs::read(session.join(name)).unwrap(), bytes);
    }

    let get = response(run(vault.path(), &["sessions", "get", "session-1"], true));
    assert_eq!(get["data"]["title"], title);
    let list = response(run(
        vault.path(),
        &["sessions", "list", "--query", "Résumé"],
        true,
    ));
    assert_eq!(list["data"][0]["title"], title);
    let search = response(run(
        vault.path(),
        &["sessions", "search", "Résumé", "--kind", "title"],
        true,
    ));
    assert!(!search["data"].as_array().unwrap().is_empty());
    let old = response(run(
        vault.path(),
        &["sessions", "list", "--query", "Original"],
        true,
    ));
    assert!(old["data"].as_array().unwrap().is_empty());
}

#[test]
fn aliases_and_repeated_verbatim_titles_work_in_text_and_json_modes() {
    let vault = tempfile::tempdir().unwrap();
    let session = seed(vault.path());
    for title in ["  Spaced title  ", "", "Same title", "Same title"] {
        let output = run(
            vault.path(),
            &["meetings", "rename", "session-1", title],
            false,
        );
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stderr).contains("`loof meetings` is deprecated"));
        assert_eq!(output.stdout, b"Renamed session session-1.\n");
        let meta: Value =
            serde_json::from_slice(&std::fs::read(session.join("_meta.json")).unwrap()).unwrap();
        assert_eq!(meta["title"], title);
    }
    let result = response(run(
        vault.path(),
        &["meetings", "rename", "session-1", "Alias"],
        true,
    ));
    assert_eq!(result["command"], "sessions.rename");
    let output = run(
        vault.path(),
        &["sessions", "rename", "session-1", "Canonical"],
        false,
    );
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    assert_eq!(output.stdout, b"Renamed session session-1.\n");
}

#[test]
fn missing_invalid_and_corrupt_sessions_fail_without_writing() {
    let vault = tempfile::tempdir().unwrap();
    let session = seed(vault.path());
    let meta = session.join("_meta.json");
    std::fs::write(&meta, "invalid json").unwrap();
    for (id, exit, code) in [
        ("missing", 2, "not_found"),
        ("../outside", 1, "operation_failed"),
        ("session-1", 1, "operation_failed"),
    ] {
        let output = run(vault.path(), &["sessions", "rename", id, "New"], true);
        assert_eq!(output.status.code(), Some(exit), "{output:?}");
        assert!(output.stdout.is_empty());
        let error: Value = serde_json::from_slice(&output.stderr).unwrap();
        assert_eq!(error["error"]["code"], code);
    }
    assert_eq!(std::fs::read_to_string(meta).unwrap(), "invalid json");
    assert!(!vault.path().join("sessions/missing").exists());
}

#[test]
fn rename_requires_both_id_and_title() {
    let vault = tempfile::tempdir().unwrap();
    for args in [
        vec!["sessions", "rename"],
        vec!["sessions", "rename", "session-1"],
    ] {
        let output = run(vault.path(), &args, false);
        assert_eq!(output.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&output.stderr).contains("required arguments"));
    }
}
