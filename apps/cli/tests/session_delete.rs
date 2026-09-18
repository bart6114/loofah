use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

fn command() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_loof"));
    for name in [
        "LOOFAH_BASE",
        "LOOFAH_VAULT_PATH",
        "FMTR_BASE",
        "FMTR_VAULT_PATH",
    ] {
        cmd.env_remove(name);
    }
    cmd.stdin(Stdio::null());
    cmd
}

fn run(vault: &Path, args: &[&str]) -> Output {
    command()
        .arg("--vault-path")
        .arg(vault)
        .arg("--json")
        .args(args)
        .output()
        .unwrap()
}

fn data(output: &Output) -> Value {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    let response: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(response["schema_version"], "1");
    response["data"].clone()
}

fn error(output: &Output, status: i32, code: &str) {
    assert_eq!(output.status.code(), Some(status), "{output:?}");
    let response: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(response["error"]["code"], code);
}

fn seed(vault: &Path, id: &str) -> PathBuf {
    let path = vault.join("sessions").join(id);
    std::fs::create_dir_all(&path).unwrap();
    std::fs::write(
        path.join("_meta.json"),
        json!({
            "id": id, "title": "Keep every file", "created_at": "2026-09-18T00:00:00Z", "tags": []
        })
        .to_string(),
    )
    .unwrap();
    path
}

fn file_body(name: &str) -> String {
    match name {
        "transcript.json" | "tasks.json" => "{}".to_string(),
        _ => format!("original {name}"),
    }
}

#[test]
fn deletes_complete_session_and_supports_manual_recovery_and_read_back() {
    let vault = tempfile::tempdir().unwrap();
    let created = data(&run(
        vault.path(),
        &["sessions", "new", "--title", "Keep every file"],
    ));
    let id = created["id"].as_str().unwrap();
    let source = vault
        .path()
        .canonicalize()
        .unwrap()
        .join("sessions")
        .join(id);
    let files = [
        "notes.md",
        "_memo.md",
        "transcript.json",
        "tasks.json",
        "audio.mp3",
        "audio.wav",
        "audio.ogg",
        "audio.peaks.json",
        "audio_mic.wav",
        "audio_spk.wav",
        "audio.tmp",
        "audio/legacy.wav",
        "enhanced/summary.md",
        "attachments/image.png",
        "unknown-user-file.bin",
        "user-folder/nested.txt",
        ".hidden",
    ];
    for name in files {
        let path = source.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, file_body(name)).unwrap();
    }
    let meta = std::fs::read(source.join("_meta.json")).unwrap();
    let output = run(vault.path(), &["sessions", "delete", id]);
    let envelope: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(envelope["command"], "sessions.delete");
    let result = data(&output);
    assert_eq!(result["id"], id);
    assert_eq!(result["status"], "deleted");
    assert_eq!(result["mode"], "soft");
    assert_eq!(Path::new(result["path"].as_str().unwrap()), source);
    chrono::DateTime::parse_from_rfc3339(result["deleted_at"].as_str().unwrap()).unwrap();
    let trash = Path::new(result["trash_path"].as_str().unwrap());
    assert!(trash.starts_with(vault.path().canonicalize().unwrap().join(".trash")));
    assert!(!source.exists());
    for name in files {
        assert_eq!(
            std::fs::read(trash.join(name)).unwrap(),
            file_body(name).as_bytes()
        );
    }
    assert_eq!(std::fs::read(trash.join("_meta.json")).unwrap(), meta);
    error(&run(vault.path(), &["sessions", "get", id]), 2, "not_found");
    assert_eq!(data(&run(vault.path(), &["sessions", "list"])), json!([]));
    error(
        &run(vault.path(), &["sessions", "delete", id]),
        2,
        "not_found",
    );
    assert!(trash.is_dir());
    std::fs::rename(trash, &source).unwrap();
    let restored = data(&run(vault.path(), &["sessions", "get", id]));
    assert_eq!(restored["note"]["markdown"], "original notes.md");
    let path = data(&run(vault.path(), &["sessions", "path", id]));
    assert_eq!(path["id"], id);
    assert_eq!(std::fs::read(source.join("_meta.json")).unwrap(), meta);
}

#[test]
fn rejects_invalid_missing_and_non_session_targets() {
    let vault = tempfile::tempdir().unwrap();
    let safe = seed(vault.path(), "safe");
    for id in [
        "",
        ".",
        "..",
        "../safe",
        "/tmp/outside",
        "nested/safe",
        "nested\\safe",
        ".hidden",
    ] {
        error(
            &run(vault.path(), &["sessions", "delete", id]),
            1,
            "operation_failed",
        );
        assert!(safe.join("_meta.json").is_file());
    }
    error(
        &run(vault.path(), &["sessions", "delete", "missing"]),
        2,
        "not_found",
    );
    let plain = vault.path().join("sessions/plain");
    std::fs::create_dir_all(&plain).unwrap();
    std::fs::write(plain.join("mine.txt"), b"keep").unwrap();
    error(
        &run(vault.path(), &["sessions", "delete", "plain"]),
        2,
        "not_found",
    );
    assert_eq!(std::fs::read(plain.join("mine.txt")).unwrap(), b"keep");
    let wrong = seed(vault.path(), "wrong");
    std::fs::rename(&wrong, vault.path().join("sessions/mismatch")).unwrap();
    error(
        &run(vault.path(), &["sessions", "delete", "mismatch"]),
        1,
        "operation_failed",
    );
    let corrupt = seed(vault.path(), "corrupt");
    std::fs::write(corrupt.join("_meta.json"), b"invalid JSON").unwrap();
    error(
        &run(vault.path(), &["sessions", "delete", "corrupt"]),
        1,
        "operation_failed",
    );
    assert!(corrupt.is_dir());
    assert!(vault.path().join("sessions/mismatch/_meta.json").is_file());
    assert!(!vault.path().join(".trash").exists());
}

#[test]
fn collisions_preserve_existing_trash_and_failed_moves_leave_source_readable() {
    let vault = tempfile::tempdir().unwrap();
    let source = seed(vault.path(), "session");
    let old = vault
        .path()
        .join(".trash")
        .join(chrono::Utc::now().format("%Y-%m-%d").to_string())
        .join("sessions/session");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(old.join("keep.txt"), b"older deletion").unwrap();
    let result = data(&run(vault.path(), &["sessions", "delete", "session"]));
    assert_ne!(
        Path::new(result["trash_path"].as_str().unwrap()),
        old.canonicalize().unwrap()
    );
    assert_eq!(
        std::fs::read(old.join("keep.txt")).unwrap(),
        b"older deletion"
    );
    assert!(!source.exists());

    let blocked = tempfile::tempdir().unwrap();
    let source = seed(blocked.path(), "session");
    std::fs::write(blocked.path().join(".trash"), b"obstruction").unwrap();
    error(
        &run(blocked.path(), &["sessions", "delete", "session"]),
        1,
        "operation_failed",
    );
    assert!(source.join("_meta.json").is_file());
    data(&run(blocked.path(), &["sessions", "get", "session"]));
    std::fs::remove_file(blocked.path().join(".trash")).unwrap();
    data(&run(blocked.path(), &["sessions", "delete", "session"]));
}

#[test]
fn respects_environment_vault_and_plain_output_never_prompts() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path(), "session");
    let output = command()
        .env("LOOFAH_VAULT_PATH", vault.path())
        .args(["sessions", "delete", "session"])
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("Deleted session session; recover from"));
    assert!(!vault.path().join("sessions/session").exists());
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_sessions_metadata_and_trash_parents() {
    use std::os::unix::fs::symlink;
    for component in [
        "sessions",
        "sessions/session",
        "sessions/session/_meta.json",
        ".trash",
    ] {
        let vault = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source = seed(vault.path(), "session");
        let path = vault.path().join(component);
        let target = outside.path().join("target");
        if path.exists() {
            std::fs::rename(&path, &target).unwrap();
        } else {
            std::fs::create_dir(&target).unwrap();
        }
        symlink(&target, &path).unwrap();
        error(
            &run(vault.path(), &["sessions", "delete", "session"]),
            1,
            "operation_failed",
        );
        assert!(source.join("_meta.json").is_file());
        assert!(target.exists());
    }
}

#[cfg(unix)]
#[test]
fn exact_id_does_not_require_permission_to_enumerate_sessions() {
    use std::os::unix::fs::PermissionsExt;
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path(), "session");
    let sessions = vault.path().join("sessions");
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o300)).unwrap();
    let deleted = run(vault.path(), &["sessions", "delete", "session"]);
    let missing = run(vault.path(), &["sessions", "delete", "absent"]);
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
    data(&deleted);
    error(&missing, 2, "not_found");
}

#[cfg(unix)]
#[test]
fn failed_rename_keeps_the_original_session_recoverable() {
    use std::os::unix::fs::PermissionsExt;
    let vault = tempfile::tempdir().unwrap();
    let source = seed(vault.path(), "session");
    std::fs::write(source.join("notes.md"), b"keep this note").unwrap();
    let sessions = vault.path().join("sessions");
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o500)).unwrap();
    let output = run(vault.path(), &["sessions", "delete", "session"]);
    std::fs::set_permissions(&sessions, std::fs::Permissions::from_mode(0o700)).unwrap();
    error(&output, 1, "operation_failed");
    let original = data(&run(vault.path(), &["sessions", "get", "session"]));
    assert_eq!(original["note"]["markdown"], "keep this note");
    data(&run(vault.path(), &["sessions", "delete", "session"]));
}
