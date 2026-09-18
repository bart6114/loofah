use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

fn run(vault: &Path, args: &[&str]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_loof"));
    for key in [
        "LOOFAH_BASE",
        "LOOFAH_VAULT_PATH",
        "FMTR_BASE",
        "FMTR_VAULT_PATH",
    ] {
        cmd.env_remove(key);
    }
    cmd.env("TZ", "UTC")
        .arg("--vault-path")
        .arg(vault)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn error(output: Output, exit: i32, code: &str) {
    assert_eq!(output.status.code(), Some(exit), "{output:?}");
    assert!(output.stdout.is_empty());
    let value: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(value["error"]["code"], code);
}

fn seed(vault: &Path) -> PathBuf {
    let session = vault.join("sessions/demo");
    std::fs::create_dir_all(session.join("enhanced")).unwrap();
    std::fs::write(
        session.join("_meta.json"),
        json!({"id":"demo", "title":"Démo 👋", "created_at":"2026-09-18T00:00:00Z", "tags": []})
            .to_string(),
    )
    .unwrap();
    std::fs::write(
        session.join("notes.md"),
        "## Café\n\n- **naïve** [link](https://example.com)\n- 你好 👋",
    )
    .unwrap();
    std::fs::write(
        session.join("enhanced/00000000-0000-4000-8000-000000000001.md"),
        "---\nkind: summary\n---\n\n**Décision**: ship `v2`.",
    )
    .unwrap();
    std::fs::write(
        vault.join("people.json"),
        json!({"people":[{"id":"zoe", "name":"Zoë"}]}).to_string(),
    )
    .unwrap();
    std::fs::write(session.join("transcript.json"), json!({"transcripts":[
        {"id":"later", "session_id":"demo", "started_at":60000, "ended_at":120000,
         "words":[{"id":"w2", "text":"Unknown speaker.", "channel":1, "start_ms":0,"end_ms":1000}]},
        {"id":"first", "session_id":"demo", "started_at":0, "ended_at":60000,
         "words":[{"id":"w1", "text":"Bonjour — 你好.", "channel":0, "start_ms":0,"end_ms":1000}],
         "speaker_hints":[{"word_id":"w1", "type":"speaker_label", "value":"zoe"}]}
    ]}).to_string()).unwrap();
    std::fs::write(session.join("unknown.md"), "DO NOT EXPORT").unwrap();
    std::fs::write(session.join(".hidden"), "DO NOT EXPORT").unwrap();
    session
}

fn snapshot(root: &Path) -> BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)> {
    fn visit(
        root: &Path,
        dir: &Path,
        result: &mut BTreeMap<PathBuf, (Vec<u8>, std::time::SystemTime)>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    (
                        std::fs::read(&path).unwrap(),
                        path.metadata().unwrap().modified().unwrap(),
                    ),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

#[test]
fn headless_formats_match_desktop_fixture_and_leave_vault_unchanged() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    let before = snapshot(vault.path());
    let mut input: hypr_export_core::ExportInput = serde_json::from_str(include_str!(
        "../../../crates/export-core/tests/fixtures/desktop-export.json"
    ))
    .unwrap();
    input.metadata.as_mut().unwrap().participants.clear();
    input.transcript.as_mut().unwrap().items.pop();
    for (name, format) in [
        ("md", hypr_export_core::TextFormat::Md),
        ("txt", hypr_export_core::TextFormat::Txt),
        ("org", hypr_export_core::TextFormat::Org),
    ] {
        let expected = hypr_export_core::render_text(&input, format, &Default::default());
        let actual = success(run(
            vault.path(),
            &[
                "sessions",
                "export",
                "demo",
                "--format",
                name,
                "--include",
                "note,summary,transcript",
            ],
        ));
        assert_eq!(actual.trim_end(), expected.trim_end(), "{name}");
    }
    assert_eq!(snapshot(vault.path()), before);
}

#[test]
fn selection_defaults_json_reporting_and_legacy_compatibility() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    let txt = success(run(
        vault.path(),
        &["sessions", "export", "demo", "--format", "txt"],
    ));
    assert!(txt.contains("Décision"));
    assert!(!txt.contains("Bonjour"));
    let transcript = success(run(
        vault.path(),
        &[
            "--json",
            "sessions",
            "export",
            "demo",
            "--format",
            "txt",
            "--include",
            "transcript",
        ],
    ));
    let report: Value = serde_json::from_str(&transcript).unwrap();
    assert_eq!(report["command"], "meetings.export");
    let text = report["data"]["content"].as_str().unwrap();
    assert!(text.contains("Zoë: Bonjour"));
    assert!(text.contains("Speaker 1: Unknown"));
    assert!(!text.contains("Café"));
    let summary = success(run(
        vault.path(),
        &["sessions", "export", "demo", "--include", "summary"],
    ));
    assert!(summary.contains("Décision"));
    assert!(!summary.contains("Café"));
    assert!(!summary.contains("Bonjour"));
    let legacy = success(run(vault.path(), &["sessions", "export", "demo"]));
    assert!(legacy.contains("Décision"));
    assert!(legacy.contains("Bonjour"));
    let raw = success(run(
        vault.path(),
        &["sessions", "export", "demo", "--format", "json"],
    ));
    let raw: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(raw["summaries"].as_array().unwrap().len(), 1);
    assert_eq!(raw["transcripts"].as_array().unwrap().len(), 2);
    assert!(raw.get("schema_version").is_none());
    let selected = success(run(
        vault.path(),
        &[
            "sessions",
            "export",
            "demo",
            "--format",
            "json",
            "--include",
            "transcript",
        ],
    ));
    let selected: Value = serde_json::from_str(&selected).unwrap();
    assert!(selected["note"].is_null());
    assert_eq!(selected["summaries"], json!([]));
    assert_eq!(selected["transcripts"].as_array().unwrap().len(), 2);
}

#[test]
fn file_output_is_atomic_protected_and_reporting_is_separate() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("session.txt");
    let path = file.to_str().unwrap();
    let report = success(run(
        vault.path(),
        &[
            "--json", "sessions", "export", "demo", "--format", "txt", "--output", path,
        ],
    ));
    let report: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["data"]["output"], path);
    assert!(std::fs::read_to_string(&file).unwrap().starts_with("Démo"));
    error(
        run(
            vault.path(),
            &[
                "--json", "sessions", "export", "demo", "--format", "txt", "--output", path,
            ],
        ),
        4,
        "output_exists",
    );
    success(run(
        vault.path(),
        &[
            "sessions",
            "export",
            "demo",
            "--format",
            "txt",
            "--include",
            "transcript",
            "--output",
            path,
            "--force",
        ],
    ));
    let bytes = std::fs::read(&file).unwrap();
    assert!(!String::from_utf8_lossy(&bytes).contains("Café"));
    error(
        run(
            vault.path(),
            &[
                "--json",
                "sessions",
                "export",
                "missing",
                "--include",
                "summary",
                "--output",
                path,
                "--force",
            ],
        ),
        2,
        "not_found",
    );
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    success(run(
        vault.path(),
        &[
            "--json", "sessions", "export", "demo", "--format", "json", "--output", path, "--force",
        ],
    ));
    let legacy: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(legacy["schema_version"], "1");
    assert_eq!(legacy["data"]["id"], "demo");
    assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 1);
    let note = vault.path().join("sessions/demo/notes.md");
    let before = snapshot(vault.path());
    error(
        run(
            vault.path(),
            &[
                "--json",
                "sessions",
                "export",
                "demo",
                "--output",
                note.to_str().unwrap(),
                "--force",
            ],
        ),
        1,
        "operation_failed",
    );
    assert_eq!(snapshot(vault.path()), before);
}

#[test]
fn selected_text_and_json_artifacts_stay_separate_from_reports() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path());
    let out = tempfile::tempdir().unwrap();
    for format in ["markdown", "org", "txt", "json"] {
        let file = out.path().join(format);
        let report = success(run(
            vault.path(),
            &[
                "--json",
                "sessions",
                "export",
                "demo",
                "--format",
                format,
                "--include",
                "note",
                "--output",
                file.to_str().unwrap(),
            ],
        ));
        let report: Value = serde_json::from_str(&report).unwrap();
        let bytes = std::fs::read(&file).unwrap();
        assert_eq!(report["data"]["format"], format);
        assert_eq!(report["data"]["bytes"], bytes.len());
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.contains("Café"));
        assert!(!text.contains("schema_version"));
        assert!(!text.contains("Bonjour"));
        assert!(!text.contains("Décision"));
        let stdout = success(run(
            vault.path(),
            &[
                "--json",
                "sessions",
                "export",
                "demo",
                "--format",
                format,
                "--include",
                "note",
            ],
        ));
        let stdout: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(stdout["data"]["content"], text);
        if format == "json" {
            let artifact: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(artifact["id"], "demo");
        }
    }
}

#[test]
fn missing_empty_and_failure_contracts() {
    let vault = tempfile::tempdir().unwrap();
    let session = seed(vault.path());
    error(
        run(
            vault.path(),
            &["--json", "sessions", "export", "missing", "--format", "txt"],
        ),
        2,
        "not_found",
    );
    error(
        run(
            vault.path(),
            &["--json", "sessions", "export", "demo", "--format", "pdf"],
        ),
        1,
        "operation_failed",
    );
    error(
        run(
            vault.path(),
            &[
                "--json", "sessions", "export", "demo", "--format", "pdf", "--output", "-",
            ],
        ),
        1,
        "operation_failed",
    );
    error(
        run(
            vault.path(),
            &[
                "--json",
                "sessions",
                "export",
                "demo",
                "--include",
                "invalid",
            ],
        ),
        2,
        "invalid_arguments",
    );
    std::fs::remove_file(session.join("notes.md")).unwrap();
    std::fs::remove_file(session.join("transcript.json")).unwrap();
    std::fs::remove_dir_all(session.join("enhanced")).unwrap();
    let txt = success(run(
        vault.path(),
        &[
            "sessions",
            "export",
            "demo",
            "--format",
            "txt",
            "--include",
            "note,summary,transcript",
        ],
    ));
    assert!(txt.starts_with("Démo"));
    assert!(!txt.contains("Note"));
    assert!(!txt.contains("Transcript"));
    std::fs::write(session.join("transcript.json"), "{broken").unwrap();
    error(
        run(
            vault.path(),
            &["--json", "sessions", "export", "demo", "--format", "txt"],
        ),
        1,
        "operation_failed",
    );
}

#[test]
fn pdf_embeds_portable_images_and_protects_existing_output() {
    let vault = tempfile::tempdir().unwrap();
    let session = seed(vault.path());
    std::fs::create_dir(session.join("attachments")).unwrap();
    std::fs::write(session.join("attachments/photo é.svg"), r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><rect width="20" height="20" fill="#ff0000"/></svg>"##).unwrap();
    std::fs::write(session.join("notes.md"), "![Photo](attachments/photo%20%C3%A9.svg)\n\n![missing](attachments/missing.png)\n\n[Document](attachments/doc.pdf)").unwrap();
    let before = snapshot(vault.path());
    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("session.pdf");
    let path = file.to_str().unwrap();
    let report = success(run(
        vault.path(),
        &[
            "--json",
            "sessions",
            "export",
            "demo",
            "--format",
            "pdf",
            "--include",
            "note,summary,transcript",
            "--output",
            path,
        ],
    ));
    let report: Value = serde_json::from_str(&report).unwrap();
    assert_eq!(report["data"]["format"], "pdf");
    let bytes = std::fs::read(&file).unwrap();
    assert!(bytes.starts_with(b"%PDF"));
    assert!(bytes.len() > 1000);
    error(
        run(
            vault.path(),
            &[
                "--json", "sessions", "export", "demo", "--format", "pdf", "--output", path,
            ],
        ),
        4,
        "output_exists",
    );
    assert_eq!(std::fs::read(&file).unwrap(), bytes);
    success(run(
        vault.path(),
        &[
            "sessions",
            "export",
            "demo",
            "--format",
            "pdf",
            "--include",
            "transcript",
            "--output",
            path,
            "--force",
        ],
    ));
    assert!(std::fs::read(&file).unwrap().starts_with(b"%PDF"));
    assert_eq!(snapshot(vault.path()), before);
    let intact = std::fs::read(&file).unwrap();
    std::fs::write(session.join("attachments/photo é.svg"), "broken SVG").unwrap();
    error(
        run(
            vault.path(),
            &[
                "--json", "sessions", "export", "demo", "--format", "pdf", "--output", path,
                "--force",
            ],
        ),
        1,
        "operation_failed",
    );
    assert_eq!(std::fs::read(&file).unwrap(), intact);
}
