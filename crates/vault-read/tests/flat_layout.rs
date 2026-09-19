use std::path::Path;

use vault_read::{enhanced, meta, tasks, transcript};

const LEGACY_ID: &str = "550e8400-e29b-41d4-a716-446655440000";
const CANONICAL_ID: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
const CANONICAL_DIR: &str = "sessions/6ba7b810-9dad-11d1-80b4-00c04fd430c8";

fn seed_full_session(vault: &Path, relative_dir: &str, id: &str, title: &str) {
    let dir = vault.join(relative_dir);
    std::fs::create_dir_all(dir.join("enhanced")).unwrap();
    std::fs::write(
        dir.join("_meta.json"),
        serde_json::json!({
            "id": id,
            "title": title,
            "started_at": "2026-03-20T09:00:00Z",
            "ended_at": null,
            "created_at": "2026-03-20T08:00:00Z",
            "tags": [],
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(dir.join("notes.md"), format!("note for {title}")).unwrap();
    // A destination collision keeps the legacy summary readable without claiming this file.
    std::fs::write(dir.join("summary.md"), format!("attachment for {title}")).unwrap();
    std::fs::write(
        dir.join("enhanced/doc-1.md"),
        format!("---\nkind: summary\ntitle: Recap\nsort_order: 1\n---\n\nenhanced for {title}"),
    )
    .unwrap();
    std::fs::write(
        dir.join("transcript.json"),
        serde_json::json!({
            "transcripts": [{
                "id": "t1",
                "session_id": id,
                "started_at": 0.0,
                "words": [
                    {"text": "hello", "start_ms": 0.0, "end_ms": 10.0, "channel": 0.0}
                ],
            }],
        })
        .to_string(),
    )
    .unwrap();
    std::fs::write(
        dir.join("tasks.json"),
        serde_json::json!({
            "tasks": [{
                "id": "task-1",
                "source_type": "session_raw_note",
                "source_id": id,
                "source_order": 1,
                "status": "todo",
                "text": format!("task for {title}"),
                "body": [],
                "created_at": "2026-03-20T08:00:00Z",
                "updated_at": "2026-03-20T08:00:00Z",
            }],
        })
        .to_string(),
    )
    .unwrap();
}

#[test]
fn all_readers_resolve_canonical_sessions_by_full_id() {
    let vault = tempfile::tempdir().unwrap();
    seed_full_session(
        vault.path(),
        &format!("sessions/{LEGACY_ID}"),
        LEGACY_ID,
        "Legacy",
    );
    seed_full_session(vault.path(), CANONICAL_DIR, CANONICAL_ID, "Readable");

    let mut ids: Vec<String> = meta::list_session_metas(vault.path())
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect();
    ids.sort();
    let mut expected = vec![LEGACY_ID.to_string(), CANONICAL_ID.to_string()];
    expected.sort();
    assert_eq!(ids, expected);

    for (id, title) in [(LEGACY_ID, "Legacy"), (CANONICAL_ID, "Readable")] {
        let session_meta = meta::read_session_meta(vault.path(), id).unwrap().unwrap();
        assert_eq!(session_meta.id, id);
        assert_eq!(session_meta.title, title);

        assert_eq!(
            meta::read_note(vault.path(), id).unwrap().unwrap(),
            format!("note for {title}")
        );

        let docs = enhanced::list_enhanced_docs(vault.path(), id).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].session_id, id);
        assert_eq!(docs[0].markdown, format!("enhanced for {title}"));

        let transcripts = transcript::read_transcript_json(vault.path(), id)
            .unwrap()
            .transcripts;
        assert_eq!(transcripts.len(), 1);
        assert_eq!(transcripts[0].words[0].text, "hello");

        let task_items = tasks::read_session_tasks(vault.path(), id).unwrap();
        assert_eq!(task_items.len(), 1);
        assert_eq!(task_items[0].text, format!("task for {title}"));
    }

    assert!(
        meta::read_session_meta(vault.path(), "2026-03-20 — Product planning — 6ba7b8")
            .unwrap()
            .is_none()
    );
}

#[test]
fn note_fallback_reads_pre_rename_memo_file() {
    let vault = tempfile::tempdir().unwrap();
    for (relative_dir, id, title) in [
        (format!("sessions/{LEGACY_ID}"), LEGACY_ID, "Legacy"),
        (CANONICAL_DIR.to_string(), CANONICAL_ID, "Readable"),
    ] {
        let dir = vault.path().join(&relative_dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::json!({
                "id": id,
                "title": title,
                "started_at": null,
                "ended_at": null,
                "created_at": "2026-03-20T08:00:00Z",
                "tags": [],
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(dir.join("_memo.md"), format!("legacy note for {title}")).unwrap();

        assert_eq!(
            meta::read_note(vault.path(), id).unwrap().unwrap(),
            format!("legacy note for {title}")
        );
    }
}

#[test]
fn safe_legacy_ids_are_preserved_and_unsafe_components_are_rejected() {
    for id in ["legacy-1", "note 2024", "会議", LEGACY_ID] {
        assert_eq!(
            vault_read::paths::validated_session_dir(id).unwrap(),
            Path::new("sessions").join(id)
        );
    }
    for id in [
        "",
        ".",
        "..",
        ".hidden",
        "../escape",
        "/absolute",
        "a/b",
        "a\\b",
        "C:drive",
        "NUL",
        "com1.txt",
        "COM¹",
        "com².txt",
        "COM³",
        "LPT¹",
        "lpt².txt",
        "LPT³",
        "trailing.",
        "trailing ",
        "a\0b",
        "a\nb",
    ] {
        assert!(
            vault_read::paths::validated_session_dir(id).is_err(),
            "{id:?}"
        );
    }
}

#[test]
fn shallow_discovery_keeps_canonical_copy_and_skips_ignored_sources() {
    let vault = tempfile::tempdir().unwrap();
    seed_full_session(
        vault.path(),
        &format!("sessions/{LEGACY_ID}"),
        LEGACY_ID,
        "canonical",
    );
    seed_full_session(vault.path(), "sessions/Readable copy", LEGACY_ID, "copy");
    seed_full_session(vault.path(), "sessions/Work/nested", CANONICAL_ID, "nested");
    seed_full_session(vault.path(), "sessions/.hidden", "hidden", "hidden");
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        vault.path().join(format!("sessions/{LEGACY_ID}")),
        vault.path().join("sessions/link"),
    )
    .unwrap();
    let scan = vault_read::discover_sessions(vault.path()).unwrap();
    assert_eq!(scan.sessions.len(), 1);
    assert_eq!(scan.sessions[0].1.title, "canonical");
    assert!(
        vault_read::find_session(vault.path(), CANONICAL_ID)
            .unwrap()
            .is_none()
    );
    assert!(
        meta::read_note(vault.path(), CANONICAL_ID)
            .unwrap()
            .is_none()
    );
    assert!(
        enhanced::list_enhanced_docs(vault.path(), CANONICAL_ID)
            .unwrap()
            .is_empty()
    );
    assert!(
        tasks::read_session_tasks(vault.path(), CANONICAL_ID)
            .unwrap()
            .is_empty()
    );
    assert!(
        transcript::read_transcript_json(vault.path(), CANONICAL_ID)
            .unwrap()
            .transcripts
            .is_empty()
    );
    assert!(meta::read_session_meta(vault.path(), "Readable copy").is_err());
}

#[cfg(unix)]
#[test]
fn per_id_reads_and_misses_work_without_permission_to_enumerate_sessions() {
    use std::os::unix::fs::PermissionsExt;
    let vault = tempfile::tempdir().unwrap();
    seed_full_session(
        vault.path(),
        &format!("sessions/{LEGACY_ID}"),
        LEGACY_ID,
        "target",
    );
    seed_full_session(vault.path(), "sessions/unrelated", "unrelated", "unrelated");
    let root = vault.path().join("sessions");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o111)).unwrap();
    // Search permission allows known paths; directory enumeration is denied.
    assert!(std::fs::read_dir(&root).is_err());
    assert!(
        meta::read_session_meta(vault.path(), LEGACY_ID)
            .unwrap()
            .is_some()
    );
    assert!(meta::read_note(vault.path(), LEGACY_ID).unwrap().is_some());
    assert_eq!(
        enhanced::list_enhanced_docs(vault.path(), LEGACY_ID)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        tasks::read_session_tasks(vault.path(), LEGACY_ID)
            .unwrap()
            .len(),
        1
    );
    assert!(
        meta::read_session_meta(vault.path(), "missing")
            .unwrap()
            .is_none()
    );
    assert!(meta::read_note(vault.path(), "missing").unwrap().is_none());
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
}
