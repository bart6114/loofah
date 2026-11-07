use super::*;
use crate::{EnhancedDoc, EnhancedDocPatch, TaskInput};
use serde_json::{Value, json};

const PREFIX: &str = "---\r\n# Keep comment\r\nkind: template_output\r\ntitle: Output label\r\ntemplate_id: retired\r\nsort_order: 7\r\nfuture: {keep: true}\r\n---\r\n\r\n";
const SUMMARY: &str = "# Summary section\n- [ ] Summary task";
const OUTPUT: &str = "# Output section\n- [ ] Output task";
const NOTE: &str = "Personal note\n- [ ] Raw task";

async fn fixture() -> (tempfile::TempDir, tempfile::TempDir, SessionStore) {
    let vault = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(vault.path().into());
    let meta=serde_json::from_value(json!({"id":"note","title":"","created_at":"2026-11-07T10:00:00Z","tags":["attached"],"tag_suggestions":{"items":["pending"],"dismissed":["dismissed"]},"future":{"keep":true}})).unwrap();
    store.create_note_session(&meta, NOTE).await.unwrap();
    store.ensure_summary("note").await.unwrap();
    store
        .update_generated_summary("note", SUMMARY, Some(""))
        .await
        .unwrap();
    store
        .write_enhanced_doc(&EnhancedDoc {
            id: "output".into(),
            session_id: "note".into(),
            kind: "template_output".into(),
            title: "Output label".into(),
            template_id: "retired".into(),
            sort_order: 7,
            markdown: OUTPUT.into(),
        })
        .await
        .unwrap();
    store
        .update_enhanced_doc(
            "note",
            "output",
            EnhancedDocPatch {
                markdown: Some(OUTPUT.into()),
                reconcile_tasks: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    for (source, id, text) in [
        ("session_summary", "note", "Summary task"),
        ("enhanced_note", "output", "Output task"),
        ("session_raw_note", "note", "Raw task"),
    ] {
        let task = store
            .session_tasks("note")
            .await
            .unwrap()
            .into_iter()
            .find(|task| task.source_type == source)
            .map(|task| task.id)
            .unwrap_or_else(|| "raw-task".into());
        store
            .replace_tasks(
                source,
                id,
                vec![TaskInput {
                    id: task,
                    source_order: 0,
                    status: "done".into(),
                    text: text.into(),
                    body: json!([]),
                    due_at: "2026-12-01".into(),
                }],
            )
            .await
            .unwrap();
    }
    std::fs::write(
        vault.path().join("sessions/note/enhanced/output.md"),
        format!("{PREFIX}{OUTPUT}"),
    )
    .unwrap();
    std::fs::write(vault.path().join("sessions/note/foreign.md"), [0xff, 0xfe]).unwrap();
    std::fs::write(
        vault.path().join("tags.json"),
        json!({"tags":[{"id":"attached","name":"attached","future":true}],"future":"registry"})
            .to_string(),
    )
    .unwrap();
    (vault, root, store)
}
async fn baseline(store: &SessionStore, root: &Path) -> GeneratedTitleBaseline {
    let GeneratedTitlePreparation::Ready(value) =
        store.prepare_generated_title(root, "note").await.unwrap()
    else {
        panic!("expected blank session");
    };
    value
}
fn intent(root: &Path) -> PathBuf {
    root.join(format!("{}.json", crate::sha256(b"note")))
}
fn files(vault: &Path) -> Vec<Option<Vec<u8>>> {
    [
        "sessions/note/_meta.json",
        "sessions/note/notes.md",
        "sessions/note/summary.md",
        "sessions/note/enhanced/output.md",
        "sessions/note/tasks.json",
        "tags.json",
    ]
    .iter()
    .map(|name| std::fs::read(vault.join(name)).ok())
    .collect()
}
async fn failed(store: &SessionStore, root: &Path, stage: Stage) {
    let prepared = baseline(store, root).await;
    assert!(
        store
            .apply_generated_title_with(
                root,
                prepared,
                " Meeting title ".into(),
                move |at| async move {
                    if at == stage {
                        Err(StoreError::Io("injected title boundary".into()))
                    } else {
                        Ok(())
                    }
                }
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn title_stamps_preserve_frontmatter_metadata_and_exact_task_files_and_rich_task_identity() {
    let (vault, root, store) = fixture().await;
    let before_tasks = std::fs::read(vault.path().join("sessions/note/tasks.json")).unwrap();
    let before = store.session_tasks("note").await.unwrap();
    let registry = std::fs::read(vault.path().join("tags.json")).unwrap();
    let mut events = store.subscribe_index_changes();
    assert_eq!(
        store
            .apply_generated_title(
                root.path(),
                baseline(&store, root.path()).await,
                " Meeting title ".into()
            )
            .await
            .unwrap(),
        GeneratedTitleOutcome::Applied {
            raw_note: RawNoteTitleOutcome::Applied
        }
    );
    assert_eq!(
        std::fs::read_to_string(vault.path().join("sessions/note/summary.md")).unwrap(),
        format!("# Meeting title\n\n{SUMMARY}")
    );
    assert_eq!(
        std::fs::read_to_string(vault.path().join("sessions/note/enhanced/output.md")).unwrap(),
        format!("{PREFIX}# Meeting title\n\n{OUTPUT}")
    );
    let meta = store.read_meta("note").await.unwrap().unwrap();
    assert_eq!(meta.title, "Meeting title");
    assert_eq!(meta.tags, ["attached"]);
    assert_eq!(meta.extra["future"], json!({"keep":true}));
    assert_eq!(meta.tag_suggestions.unwrap().items, ["pending"]);
    assert_eq!(
        std::fs::read(vault.path().join("tags.json")).unwrap(),
        registry
    );
    assert_eq!(
        std::fs::read(vault.path().join("sessions/note/tasks.json")).unwrap(),
        before_tasks
    );
    for (source, id, markdown) in [
        (
            "session_summary",
            "note",
            format!("# Meeting title\n\n{SUMMARY}"),
        ),
        (
            "enhanced_note",
            "output",
            format!("# Meeting title\n\n{OUTPUT}"),
        ),
        (
            "session_raw_note",
            "note",
            format!("# Meeting title\n\n{NOTE}"),
        ),
    ] {
        let content = hypr_tiptap::md_to_tiptap_json(&markdown).unwrap();
        let projected = store
            .prepare_generated_tasks("note", source, id, &content)
            .await
            .unwrap();
        for old in before.iter().filter(|task| task.source_type == source) {
            let new = projected.iter().find(|task| task.id == old.id).unwrap();
            assert_eq!(
                (&new.status, &new.due_at, &new.assignee, &new.created_at),
                (&old.status, &old.due_at, &old.assignee, &old.created_at)
            );
        }
    }
    assert_eq!(
        std::fs::read(vault.path().join("sessions/note/tasks.json")).unwrap(),
        before_tasks
    );
    assert_eq!(
        std::fs::read(vault.path().join("sessions/note/foreign.md")).unwrap(),
        [0xff, 0xfe]
    );
    let mut entities = std::collections::HashSet::new();
    while let Ok(event) = events.try_recv() {
        entities.insert(format!("{:?}", event.entity));
    }
    for entity in ["Sessions", "SessionHeaders", "Docs"] {
        assert!(entities.contains(entity), "{entities:?}");
    }
    assert!(!intent(root.path()).exists());
}

#[tokio::test]
async fn title_empty_existing_and_control_candidates_never_mutate_files() {
    for candidate in [
        "",
        "  ",
        " <EMPTY> ",
        "New\nTitle",
        "bad\0title",
        "bad\ttitle",
    ] {
        let (vault, root, store) = fixture().await;
        let prepared = baseline(&store, root.path()).await;
        let before = files(vault.path());
        let result = store
            .apply_generated_title(root.path(), prepared, candidate.into())
            .await;
        if candidate.trim().is_empty() || candidate.trim() == "<EMPTY>" {
            assert_eq!(result.unwrap(), GeneratedTitleOutcome::Empty);
        } else {
            assert!(matches!(result, Err(StoreError::Serialize(_))));
        }
        assert_eq!(files(vault.path()), before);
        assert!(!intent(root.path()).exists());
    }
    let (vault, root, store) = fixture().await;
    let prepared = baseline(&store, root.path()).await;
    store
        .update_meta(
            "note",
            crate::SessionMetaPatch {
                title: Some("User title".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let before = files(vault.path());
    assert_eq!(
        store
            .apply_generated_title(root.path(), prepared, "Generated".into())
            .await
            .unwrap(),
        GeneratedTitleOutcome::AlreadyNamed
    );
    assert_eq!(files(vault.path()), before);
    assert!(matches!(
        store
            .prepare_generated_title(root.path(), "note")
            .await
            .unwrap(),
        GeneratedTitlePreparation::AlreadyNamed
    ));
}

#[tokio::test]
async fn title_baseline_rejects_newer_body_frontmatter_output_set_and_blank_title_edits() {
    for case in ["body", "frontmatter", "new_output", "blank_title"] {
        let (vault, root, store) = fixture().await;
        let prepared = baseline(&store, root.path()).await;
        match case {
            "body" => {
                std::fs::write(
                    vault.path().join("sessions/note/summary.md"),
                    "Newer user body",
                )
                .unwrap();
            }
            "frontmatter" => {
                let path = vault.path().join("sessions/note/enhanced/output.md");
                let raw = std::fs::read_to_string(&path).unwrap();
                std::fs::write(path, raw.replace("Keep comment", "Newer comment")).unwrap();
            }
            "new_output" => {
                std::fs::write(
                    vault.path().join("sessions/note/enhanced/new.md"),
                    format!("{PREFIX}Newer document"),
                )
                .unwrap();
            }
            _ => {
                store
                    .update_meta(
                        "note",
                        crate::SessionMetaPatch {
                            title: Some(" ".into()),
                            ..Default::default()
                        },
                    )
                    .await
                    .unwrap();
            }
        }
        let before = files(vault.path());
        assert_eq!(
            store
                .apply_generated_title(root.path(), prepared, "Generated".into())
                .await
                .unwrap(),
            GeneratedTitleOutcome::Conflict,
            "{case}"
        );
        assert_eq!(files(vault.path()), before, "{case}");
        assert!(!intent(root.path()).exists());
    }
}

#[tokio::test]
async fn title_raw_note_absent_empty_unreadable_changed_and_legacy_are_independent() {
    for case in ["absent", "empty", "unreadable", "changed", "legacy"] {
        let (vault, root, store) = fixture().await;
        let path = vault.path().join("sessions/note/notes.md");
        if case == "absent" {
            std::fs::remove_file(&path).unwrap();
        }
        if case == "empty" {
            std::fs::write(&path, " \n ").unwrap();
        }
        if case == "unreadable" {
            std::fs::remove_file(&path).unwrap();
            std::fs::create_dir(&path).unwrap();
        }
        if case == "legacy" {
            std::fs::rename(&path, vault.path().join("sessions/note/_memo.md")).unwrap();
        }
        let prepared = baseline(&store, root.path()).await;
        if case == "changed" {
            std::fs::write(&path, "Newer raw edit").unwrap();
        }
        let expected = match case {
            "absent" => RawNoteTitleOutcome::Absent,
            "empty" => RawNoteTitleOutcome::Empty,
            "unreadable" => RawNoteTitleOutcome::Unavailable,
            "changed" => RawNoteTitleOutcome::Changed,
            _ => RawNoteTitleOutcome::Applied,
        };
        assert_eq!(
            store
                .apply_generated_title(root.path(), prepared, "Generated".into())
                .await
                .unwrap(),
            GeneratedTitleOutcome::Applied { raw_note: expected }
        );
        assert_eq!(
            store.read_meta("note").await.unwrap().unwrap().title,
            "Generated"
        );
        assert!(
            std::fs::read_to_string(vault.path().join("sessions/note/summary.md"))
                .unwrap()
                .starts_with("# Generated\n\n")
        );
        if case == "changed" {
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "Newer raw edit");
        }
        if case == "unreadable" {
            assert!(path.is_dir());
        }
        if case == "legacy" {
            assert!(path.is_file());
            assert!(!vault.path().join("sessions/note/_memo.md").exists());
            assert!(vault.path().join(".trash").exists());
        }
    }
}

#[tokio::test]
async fn title_preserves_selected_legacy_summary_and_shadowed_canonical_bytes() {
    for collision in [true, false] {
        let (vault, root, store) = fixture().await;
        let canonical = vault.path().join("sessions/note/summary.md");
        if !collision {
            std::fs::remove_file(&canonical).unwrap();
        }
        let raw = format!(
            "{}Legacy summary",
            PREFIX.replace("kind: template_output", "kind: summary")
        );
        let legacy = vault.path().join("sessions/note/enhanced/legacy.md");
        std::fs::write(&legacy, &raw).unwrap();
        store
            .apply_generated_title(
                root.path(),
                baseline(&store, root.path()).await,
                "Generated".into(),
            )
            .await
            .unwrap();
        assert_eq!(
            std::fs::read_to_string(&legacy).unwrap(),
            raw.replace("Legacy summary", "# Generated\n\nLegacy summary")
        );
        if collision {
            assert_eq!(std::fs::read_to_string(canonical).unwrap(), SUMMARY);
        } else {
            assert!(!canonical.exists());
        }
    }
}

#[tokio::test]
async fn title_recovery_finishes_each_boundary_and_preserves_newer_unrelated_metadata() {
    for stage in [
        Stage::Intent,
        Stage::Document(0),
        Stage::Document(1),
        Stage::Metadata,
        Stage::RawNote,
        Stage::Synced,
    ] {
        let (vault, root, store) = fixture().await;
        let tasks = std::fs::read(vault.path().join("sessions/note/tasks.json")).unwrap();
        failed(&store, root.path(), stage).await;
        let path = vault.path().join("sessions/note/_meta.json");
        let mut meta: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        meta["future"] = json!({"newer":true});
        meta["tags"] = json!(["newer-tag"]);
        std::fs::write(&path, meta.to_string()).unwrap();
        drop(store);
        let store = SessionStore::new(vault.path().into());
        store.recover_document_writes(root.path()).await.unwrap();
        let meta = store.read_meta("note").await.unwrap().unwrap();
        assert_eq!(meta.title, "Meeting title");
        assert_eq!(meta.tags, ["newer-tag"]);
        assert_eq!(meta.extra["future"], json!({"newer":true}));
        assert_eq!(
            std::fs::read(vault.path().join("sessions/note/tasks.json")).unwrap(),
            tasks
        );
        assert!(!intent(root.path()).exists());
        let before = files(vault.path());
        store.recover_document_writes(root.path()).await.unwrap();
        assert_eq!(files(vault.path()), before);
    }
}

#[tokio::test]
async fn title_recovery_keeps_newer_titles_outputs_raw_notes_and_unknown_or_unsafe_intents() {
    for case in [
        "title",
        "output",
        "raw",
        "future",
        "old",
        "unsafe",
        "unknown",
        "recording",
    ] {
        let (vault, root, store) = fixture().await;
        failed(&store, root.path(), Stage::Document(0)).await;
        match case {
            "title" => {
                let path = vault.path().join("sessions/note/_meta.json");
                let mut meta: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                meta["title"] = json!("Newer title");
                std::fs::write(path, meta.to_string()).unwrap();
            }
            "output" => {
                std::fs::write(
                    vault.path().join("sessions/note/summary.md"),
                    "Newer summary body",
                )
                .unwrap();
            }
            "raw" => {
                std::fs::write(
                    vault.path().join("sessions/note/notes.md"),
                    "Newer raw note",
                )
                .unwrap();
            }
            "future" | "old" | "unsafe" | "unknown" => {
                let path = intent(root.path());
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                match case {
                    "future" => value["schema_version"] = json!(5),
                    "old" => value["schema_version"] = json!(3),
                    "unsafe" => value["documents"][0]["path"] = json!("../foreign.md"),
                    _ => value["future"] = json!(true),
                }
                std::fs::write(path, value.to_string()).unwrap();
            }
            _ => {}
        }
        drop(store);
        let store = SessionStore::new(vault.path().into());
        if case == "recording" {
            store.note_recording_active("note");
        }
        let before = files(vault.path());
        let record = std::fs::read(intent(root.path())).unwrap();
        let result = store.recover_document_writes(root.path()).await;
        if case == "raw" {
            result.unwrap();
            assert_eq!(
                std::fs::read_to_string(vault.path().join("sessions/note/notes.md")).unwrap(),
                "Newer raw note"
            );
            assert_eq!(
                store.read_meta("note").await.unwrap().unwrap().title,
                "Meeting title"
            );
        } else {
            assert!(result.is_err(), "{case}");
            assert_eq!(files(vault.path()), before, "{case}");
            assert_eq!(
                std::fs::read(intent(root.path())).unwrap(),
                record,
                "{case}"
            );
        }
    }
}

#[tokio::test]
async fn title_preflights_all_outputs_before_the_first_write_and_never_overwrites_a_late_edit() {
    for stage in [Stage::Intent, Stage::Document(0)] {
        let (vault, root, store) = fixture().await;
        let prepared = baseline(&store, root.path()).await;
        let summary = vault.path().join("sessions/note/summary.md");
        let edit = summary.clone();
        assert!(
            store
                .apply_generated_title_with(root.path(), prepared, "Generated".into(), move |at| {
                    if at == stage {
                        std::fs::write(&edit, "Late user edit").unwrap();
                    }
                    async { Ok(()) }
                })
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(summary).unwrap(), "Late user edit");
        assert_eq!(store.read_meta("note").await.unwrap().unwrap().title, "");
        assert!(intent(root.path()).exists());
        if stage == Stage::Intent {
            assert_eq!(
                std::fs::read_to_string(vault.path().join("sessions/note/enhanced/output.md"))
                    .unwrap(),
                format!("{PREFIX}{OUTPUT}")
            );
        }
    }
}

#[tokio::test]
async fn title_limits_bound_candidates_document_count_hidden_entries_aggregate_and_actual_json() {
    let (vault, root, store) = fixture().await;
    let prepared = baseline(&store, root.path()).await;
    let before = files(vault.path());
    assert!(matches!(
        store
            .apply_generated_title(root.path(), prepared, "x".repeat(4097))
            .await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert_eq!(files(vault.path()), before);
    for case in ["count", "hidden", "aggregate"] {
        let (vault, root, store) = fixture().await;
        let enhanced = vault.path().join("sessions/note/enhanced");
        match case {
            "count" => {
                for n in 0..MAX_DOCUMENTS {
                    std::fs::write(
                        enhanced.join(format!("extra-{n}.md")),
                        format!("{PREFIX}Extra"),
                    )
                    .unwrap();
                }
            }
            "hidden" => {
                for n in 0..MAX_ENTRIES {
                    std::fs::write(enhanced.join(format!(".hidden-{n}")), []).unwrap();
                }
            }
            _ => {
                for n in 0..2 {
                    std::fs::write(
                        enhanced.join(format!("large-{n}.md")),
                        format!("{PREFIX}{}", "x".repeat(9 * 1024 * 1024)),
                    )
                    .unwrap();
                }
            }
        }
        assert!(
            matches!(
                store.prepare_generated_title(root.path(), "note").await,
                Err(StoreError::ResourceLimit(_))
            ),
            "{case}"
        );
        assert!(!intent(root.path()).exists());
    }
    let (vault, root, mut store) = fixture().await;
    store.content_limits = crate::ContentLimits::unlimited();
    std::fs::write(
        vault.path().join("sessions/note/summary.md"),
        vec![1u8; 6 * 1024 * 1024],
    )
    .unwrap();
    let prepared = baseline(&store, root.path()).await;
    let before = files(vault.path());
    assert!(matches!(
        store
            .apply_generated_title(root.path(), prepared, "Generated".into())
            .await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert_eq!(files(vault.path()), before);
    assert!(!intent(root.path()).exists());
}

#[tokio::test]
async fn title_intent_read_can_recover_the_exact_maximum_admitted_serialized_size() {
    let (vault, root, mut store) = fixture().await;
    let value = baseline(&store, root.path()).await;
    let planned = make_intent(
        value,
        "Meeting title".into(),
        &vault.path().join("sessions/note"),
        MAX_MATERIAL,
    )
    .unwrap();
    let size = serde_json::to_vec(&planned).unwrap().len();
    store.content_limits.recovery_bytes = size;
    failed(&store, root.path(), Stage::Intent).await;
    assert_eq!(
        std::fs::metadata(intent(root.path())).unwrap().len(),
        size as u64
    );
    drop(store);
    let mut store = SessionStore::new(vault.path().into());
    store.content_limits.recovery_bytes = size - 1;
    let before = files(vault.path());
    assert!(store.recover_document_writes(root.path()).await.is_err());
    assert_eq!(files(vault.path()), before);
    store.content_limits.recovery_bytes = size;
    store.recover_document_writes(root.path()).await.unwrap();
    assert_eq!(
        store.read_meta("note").await.unwrap().unwrap().title,
        "Meeting title"
    );
    assert!(!intent(root.path()).exists());
}

#[tokio::test]
async fn title_baselines_cannot_be_applied_to_another_vault_owner() {
    let (vault, root, store) = fixture().await;
    let prepared = baseline(&store, root.path()).await;
    let other = SessionStore::new(vault.path().into());
    let before = files(vault.path());
    assert!(
        other
            .apply_generated_title(root.path(), prepared, "Generated".into())
            .await
            .is_err()
    );
    assert_eq!(files(vault.path()), before);
}

#[test]
fn title_heading_rules_match_legacy_markdown_stamps() {
    assert_eq!(
        stamp_title("# Meeting\n\nBody", "Meeting"),
        "# Meeting\n\nBody"
    );
    assert_eq!(stamp_title(" \nBody \n", " Meeting "), "# Meeting\n\nBody");
    assert_eq!(
        stamp_title("# Other\nBody", "Meeting"),
        "# Meeting\n\n# Other\nBody"
    );
}

#[tokio::test]
async fn title_normal_concurrent_rename_is_serialized_and_raw_exporter_prefix_is_preserved() {
    let (vault, root, store) = fixture().await;
    let note = vault.path().join("sessions/note/notes.md");
    let prefix = "---\nid: note:raw\nposition: 0\nsession_id: note\nfuture: keep\n---\n\n";
    std::fs::write(&note, format!("{prefix}{NOTE}")).unwrap();
    let prepared = baseline(&store, root.path()).await;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let ready = std::sync::Arc::new(std::sync::Mutex::new(Some(ready_tx)));
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let resume = release.clone();
    let writer = store.clone();
    let operations = root.path().to_path_buf();
    let task = tokio::spawn(async move {
        writer
            .apply_generated_title_with(&operations, prepared, "Generated".into(), move |stage| {
                let ready = ready.clone();
                let resume = resume.clone();
                async move {
                    if stage == Stage::Intent {
                        ready.lock().unwrap().take().unwrap().send(()).unwrap();
                        resume.notified().await;
                    }
                    Ok(())
                }
            })
            .await
    });
    ready_rx.await.unwrap();
    let updater = store.clone();
    let rename = tokio::spawn(async move {
        updater
            .update_meta(
                "note",
                crate::SessionMetaPatch {
                    title: Some("Newer user title".into()),
                    ..Default::default()
                },
            )
            .await
    });
    release.notify_one();
    task.await.unwrap().unwrap();
    rename.await.unwrap().unwrap();
    assert_eq!(
        store.read_meta("note").await.unwrap().unwrap().title,
        "Newer user title"
    );
    assert_eq!(
        std::fs::read_to_string(note).unwrap(),
        format!("{prefix}# Generated\n\n{NOTE}")
    );
}

#[tokio::test]
async fn title_schema_four_recovers_beside_unchanged_schema_one_two_and_three_records() {
    let (vault, root, store) = fixture().await;
    failed(&store, root.path(), Stage::Document(0)).await;
    for id in ["old-document", "old-note", "old-generated"] {
        let meta = serde_json::from_value(
            json!({"id":id,"title":"","created_at":"2026-11-07T10:00:00Z","tags":[]}),
        )
        .unwrap();
        store
            .create_note_session(&meta, "Original note")
            .await
            .unwrap();
        store.ensure_summary(id).await.unwrap();
        let snapshot = store.lock_snapshot().await;
        let guard = &snapshot._guard;
        match id {
            "old-document" => {
                store
                    .begin_document_write_locked(
                        guard,
                        root.path(),
                        id,
                        &hypr_loofah_protocol::DocumentTarget::Summary,
                        b"not published",
                        &[],
                    )
                    .await
                    .unwrap();
            }
            "old-note" => {
                let current = store
                    .read_note_snapshot_locked(guard, id)
                    .await
                    .unwrap()
                    .unwrap();
                store
                    .begin_note_tasks_locked(guard, root.path(), id, &current, &[])
                    .await
                    .unwrap();
            }
            _ => {
                let current = store
                    .document_snapshot_locked(
                        guard,
                        id,
                        &hypr_loofah_protocol::DocumentTarget::Summary,
                    )
                    .await
                    .unwrap()
                    .unwrap();
                let tags = crate::document_operation::GeneratedTagMetadata::read(
                    &store.read_meta(id).await.unwrap().unwrap(),
                );
                store
                    .begin_generated_summary_write_locked(
                        guard,
                        root.path(),
                        id,
                        crate::document_operation::GeneratedSummaryWrite {
                            before_revision: &current.revision,
                            before_tasks_hash: &store.document_tasks_hash(id).await.unwrap(),
                            after_bytes: b"not published",
                            tasks: &[],
                            before_metadata: tags.clone(),
                            after_metadata: tags,
                            register_tags: vec![],
                        },
                    )
                    .await
                    .unwrap();
            }
        }
    }
    let mut versions = std::fs::read_dir(root.path())
        .unwrap()
        .map(|entry| {
            let value: Value =
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap();
            value["schema_version"].as_u64().unwrap()
        })
        .collect::<Vec<_>>();
    versions.sort();
    assert_eq!(versions, [1, 2, 3, 4]);
    drop(store);
    let store = SessionStore::new(vault.path().into());
    store.recover_document_writes(root.path()).await.unwrap();
    assert_eq!(
        store.read_meta("note").await.unwrap().unwrap().title,
        "Meeting title"
    );
    for id in ["old-document", "old-note", "old-generated"] {
        assert_eq!(
            store.read_note(id).await.unwrap().as_deref(),
            Some("Original note")
        );
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn title_killed_process_boundaries_finish_guarded_stamps_without_changing_task_bytes() {
    for stage in ["intent", "document", "metadata", "raw_note", "synced"] {
        let (vault, root, store) = fixture().await;
        let tasks = std::fs::read(vault.path().join("sessions/note/tasks.json")).unwrap();
        drop(store);
        let executable = std::env::current_exe().unwrap();
        let vault_path = vault.path().to_path_buf();
        let operations = root.path().to_path_buf();
        let result = tokio::task::spawn_blocking(move || {
            std::process::Command::new(executable)
                .args([
                    "--ignored",
                    "--exact",
                    "generated_title::tests::title_crash_child",
                ])
                .env("LOOFAH_TITLE_TEST_VAULT", vault_path)
                .env("LOOFAH_TITLE_TEST_OPERATIONS", operations)
                .env("LOOFAH_TITLE_TEST_STAGE", stage)
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(
            result.status.code(),
            Some(91),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let store = SessionStore::new(vault.path().into());
        store.recover_document_writes(root.path()).await.unwrap();
        assert_eq!(
            store.read_meta("note").await.unwrap().unwrap().title,
            "Meeting title"
        );
        assert_eq!(
            std::fs::read(vault.path().join("sessions/note/tasks.json")).unwrap(),
            tasks
        );
        assert!(!intent(root.path()).exists());
    }
}

#[tokio::test]
#[ignore = "subprocess fixture for abrupt title-write termination"]
async fn title_crash_child() {
    let Ok(vault) = std::env::var("LOOFAH_TITLE_TEST_VAULT") else {
        return;
    };
    let root = PathBuf::from(std::env::var("LOOFAH_TITLE_TEST_OPERATIONS").unwrap());
    let selected = std::env::var("LOOFAH_TITLE_TEST_STAGE").unwrap();
    let store = SessionStore::new(vault.into());
    store
        .apply_generated_title_with(
            &root,
            baseline(&store, &root).await,
            "Meeting title".into(),
            move |stage| {
                let name = match stage {
                    Stage::Intent => "intent",
                    Stage::Document(_) => "document",
                    Stage::Metadata => "metadata",
                    Stage::RawNote => "raw_note",
                    Stage::Synced => "synced",
                };
                if name == selected {
                    std::process::exit(91);
                }
                async { Ok(()) }
            },
        )
        .await
        .unwrap();
    panic!("requested boundary not reached");
}

#[tokio::test]
async fn title_oversized_schema_four_and_ambiguous_headers_are_refused_before_reading_the_body() {
    use std::io::Write;
    for header in [
        "{\"schema_version\":4,",
        "{\"schema_version\":5,",
        "{\"unknown_first\":true,",
    ] {
        let (vault, root, mut store) = fixture().await;
        store.content_limits = crate::ContentLimits::unlimited();
        let path = intent(root.path());
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(header.as_bytes()).unwrap();
        file.set_len((MAX_INTENT + 1) as u64).unwrap();
        drop(file);
        let before = files(vault.path());
        let error = store
            .recover_document_writes(root.path())
            .await
            .unwrap_err();
        if header.starts_with("{\"schema_version\":4") {
            assert!(matches!(error, StoreError::ResourceLimit(_)));
        }
        assert_eq!(files(vault.path()), before);
        assert_eq!(
            std::fs::metadata(path).unwrap().len(),
            (MAX_INTENT + 1) as u64
        );
    }
}

#[tokio::test]
async fn title_dispatch_preserves_official_oversized_local_schema_one_two_and_three_records() {
    for version in [1, 2, 3] {
        let (vault, root, mut store) = fixture().await;
        store.content_limits = crate::ContentLimits::unlimited();
        let before = files(vault.path());
        let mut tasks = store.session_tasks("note").await.unwrap();
        tasks[0].body = json!({"compatibility": "x".repeat(MAX_INTENT)});
        let snapshot = store.lock_snapshot().await;
        let guard = &snapshot._guard;
        match version {
            1 => {
                store
                    .begin_document_write_locked(
                        guard,
                        root.path(),
                        "note",
                        &hypr_loofah_protocol::DocumentTarget::Summary,
                        b"not published",
                        &tasks,
                    )
                    .await
                    .unwrap();
            }
            2 => {
                let current = store
                    .read_note_snapshot_locked(guard, "note")
                    .await
                    .unwrap()
                    .unwrap();
                store
                    .begin_note_write_locked(
                        guard,
                        root.path(),
                        "note",
                        &current,
                        ("not published", ""),
                        &tasks,
                    )
                    .await
                    .unwrap();
            }
            _ => {
                let current = store
                    .document_snapshot_locked(
                        guard,
                        "note",
                        &hypr_loofah_protocol::DocumentTarget::Summary,
                    )
                    .await
                    .unwrap()
                    .unwrap();
                let tags = crate::document_operation::GeneratedTagMetadata::read(
                    &store.read_meta("note").await.unwrap().unwrap(),
                );
                store
                    .begin_generated_summary_write_locked(
                        guard,
                        root.path(),
                        "note",
                        crate::document_operation::GeneratedSummaryWrite {
                            before_revision: &current.revision,
                            before_tasks_hash: &store.document_tasks_hash("note").await.unwrap(),
                            after_bytes: b"not published",
                            tasks: &tasks,
                            before_metadata: tags.clone(),
                            after_metadata: tags,
                            register_tags: vec![],
                        },
                    )
                    .await
                    .unwrap();
            }
        }
        assert!(std::fs::metadata(intent(root.path())).unwrap().len() > MAX_INTENT as u64);
        drop(snapshot);
        drop(tasks);
        drop(store);
        let mut store = SessionStore::new(vault.path().into());
        store.content_limits = crate::ContentLimits::unlimited();
        store.recover_document_writes(root.path()).await.unwrap();
        assert_eq!(files(vault.path()), before, "schema {version}");
        assert!(!intent(root.path()).exists());
    }
}

#[tokio::test]
async fn title_dispatch_rejects_duplicate_known_keys_in_every_official_schema() {
    for version in [1, 2, 3, 4] {
        let (vault, root, store) = fixture().await;
        if version == 4 {
            failed(&store, root.path(), Stage::Intent).await;
        } else {
            let snapshot = store.lock_snapshot().await;
            let guard = &snapshot._guard;
            match version {
                1 => {
                    store
                        .begin_document_write_locked(
                            guard,
                            root.path(),
                            "note",
                            &hypr_loofah_protocol::DocumentTarget::Summary,
                            b"not published",
                            &[],
                        )
                        .await
                        .unwrap();
                }
                2 => {
                    let current = store
                        .read_note_snapshot_locked(guard, "note")
                        .await
                        .unwrap()
                        .unwrap();
                    store
                        .begin_note_tasks_locked(guard, root.path(), "note", &current, &[])
                        .await
                        .unwrap();
                }
                _ => {
                    let current = store
                        .document_snapshot_locked(
                            guard,
                            "note",
                            &hypr_loofah_protocol::DocumentTarget::Summary,
                        )
                        .await
                        .unwrap()
                        .unwrap();
                    let tags = crate::document_operation::GeneratedTagMetadata::read(
                        &store.read_meta("note").await.unwrap().unwrap(),
                    );
                    store
                        .begin_generated_summary_write_locked(
                            guard,
                            root.path(),
                            "note",
                            crate::document_operation::GeneratedSummaryWrite {
                                before_revision: &current.revision,
                                before_tasks_hash: &store
                                    .document_tasks_hash("note")
                                    .await
                                    .unwrap(),
                                after_bytes: b"not published",
                                tasks: &[],
                                before_metadata: tags.clone(),
                                after_metadata: tags,
                                register_tags: vec![],
                            },
                        )
                        .await
                        .unwrap();
                }
            }
        }
        let before = files(vault.path());
        let original = std::fs::read_to_string(intent(root.path())).unwrap();
        let parsed: Value = serde_json::from_str(&original).unwrap();
        assert_eq!(parsed["schema_version"], version);
        drop(store);
        let store = SessionStore::new(vault.path().into());
        for property in [
            format!("\"schema_version\":{version},"),
            "\"session_id\":\"note\",".into(),
        ] {
            let duplicate = original.replacen('{', &format!("{{{property}"), 1);
            std::fs::write(intent(root.path()), &duplicate).unwrap();
            let error = store
                .recover_document_writes(root.path())
                .await
                .unwrap_err();
            assert!(error.to_string().contains("duplicate field"), "{error}");
            assert_eq!(
                std::fs::read(intent(root.path())).unwrap(),
                duplicate.as_bytes()
            );
            assert_eq!(files(vault.path()), before, "schema {version}");
        }
    }
}

#[tokio::test]
async fn title_opaque_baseline_debug_does_not_disclose_vault_paths_or_note_content() {
    let (vault, root, store) = fixture().await;
    let prepared = baseline(&store, root.path()).await;
    let rendered = format!("{prepared:?}");
    assert!(!rendered.contains(NOTE));
    assert!(!rendered.contains(SUMMARY));
    assert!(!rendered.contains(PREFIX));
    assert!(!rendered.contains(&vault.path().to_string_lossy().to_string()));
}
