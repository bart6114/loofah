use super::*;
use serde_json::{Value, json};

const BEFORE: &str = "# Before\n- [ ] Follow up";
const GENERATED: &str = "# Actions\n- [ ] Follow up\n- [ ] Send agenda";
const PUBLISHED: &str = "# Keep title\n\n# Actions\n- [ ] Follow up\n- [ ] Send agenda";
const PREFIX: &str = "---\r\n# Keep this comment\r\nkind: template_output\r\ntitle: Existing output\r\ntemplate_id: retired-template\r\nsort_order: 7\r\nfuture:\r\n  nested: [one, two]\r\n---\r\n\r\n";

async fn fixture() -> (tempfile::TempDir, tempfile::TempDir, SessionStore) {
    let vault = tempfile::tempdir().unwrap();
    let operations = tempfile::tempdir().unwrap();
    let store = SessionStore::new(vault.path().into());
    let meta = serde_json::from_value(json!({"id":"note","title":"Keep title","created_at":"2026-11-07T10:00:00Z","tags":["attached"],"tag_suggestions":{"items":["pending"],"dismissed":["ignored"]},"future":{"keep":true}})).unwrap();
    store
        .create_note_session(&meta, "Personal note")
        .await
        .unwrap();
    store
        .write_enhanced_doc(&crate::EnhancedDoc {
            id: "output".into(),
            session_id: "note".into(),
            kind: "template_output".into(),
            title: "Existing output".into(),
            template_id: "retired-template".into(),
            sort_order: 7,
            markdown: BEFORE.into(),
        })
        .await
        .unwrap();
    store
        .update_enhanced_doc(
            "note",
            "output",
            crate::EnhancedDocPatch {
                markdown: Some(BEFORE.into()),
                reconcile_tasks: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let original = store.session_tasks("note").await.unwrap().remove(0);
    store
        .replace_tasks(
            "enhanced_note",
            "output",
            vec![crate::TaskInput {
                id: original.id,
                source_order: 0,
                status: "done".into(),
                text: "Follow up".into(),
                body: json!([]),
                due_at: "2026-12-01".into(),
            }],
        )
        .await
        .unwrap();
    store
        .replace_tasks(
            "session_raw_note",
            "note",
            vec![crate::TaskInput {
                id: "personal-task".into(),
                source_order: 0,
                status: "todo".into(),
                text: "Personal action".into(),
                body: json!([]),
                due_at: String::new(),
            }],
        )
        .await
        .unwrap();
    std::fs::write(body(vault.path()), format!("{PREFIX}{BEFORE}")).unwrap();
    std::fs::write(
        vault.path().join("sessions/note/foreign.md"),
        "attachment canary",
    )
    .unwrap();
    std::fs::write(vault.path().join("tags.json"), json!({"tags":[{"id":"attached","name":"attached","future":{"keep":true}}],"future":{"registry":true}}).to_string()).unwrap();
    (vault, operations, store)
}
fn body(vault: &Path) -> std::path::PathBuf {
    vault.join("sessions/note/enhanced/output.md")
}
fn target() -> DocumentTarget {
    DocumentTarget::TemplateOutput {
        document_id: "output".into(),
    }
}
fn intent(root: &Path) -> std::path::PathBuf {
    root.join(format!("{}.json", crate::sha256(b"note")))
}
fn scored(name: &str, confidence: f64) -> ScoredTagSuggestion {
    ScoredTagSuggestion {
        name: name.into(),
        confidence,
    }
}
fn suggestions() -> Option<Vec<ScoredTagSuggestion>> {
    Some(vec![
        scored("Project/New", 0.99),
        scored("pending", 0.85),
        scored("ignored", 1.0),
    ])
}
async fn revision(store: &SessionStore) -> String {
    store
        .read_document_snapshot("note", &target())
        .await
        .unwrap()
        .unwrap()
        .revision
}
fn replacement(markdown: &str, expected_revision: &str) -> SaveDocumentRequest {
    SaveDocumentRequest {
        markdown: markdown.into(),
        expected_revision: expected_revision.into(),
    }
}
async fn save(
    store: &SessionStore,
    root: &Path,
    tags: Option<Vec<ScoredTagSuggestion>>,
    auto: bool,
) -> DocumentSaveOutcome {
    store
        .save_generated_template_with_suggestions_if_revision(
            root,
            "note",
            "output",
            replacement(GENERATED, &revision(store).await),
            tags,
            auto,
        )
        .await
        .unwrap()
}
async fn failed_at(store: &SessionStore, root: &Path, stage: CommitStage) {
    assert!(
        store
            .save_generated_template_with_checkpoint(
                root,
                ("note", "output"),
                replacement(GENERATED, &revision(store).await),
                suggestions(),
                true,
                move |at| async move {
                    if at == stage {
                        Err(StoreError::Io("injected boundary failure".into()))
                    } else {
                        Ok(())
                    }
                }
            )
            .await
            .is_err()
    );
}
fn files(vault: &Path) -> Vec<Vec<u8>> {
    [
        "sessions/note/enhanced/output.md",
        "sessions/note/_meta.json",
        "sessions/note/tasks.json",
        "tags.json",
    ]
    .iter()
    .map(|name| std::fs::read(vault.join(name)).unwrap())
    .collect()
}

#[tokio::test]
async fn generated_template_preserves_raw_frontmatter_tasks_tag_policy_and_unknown_files() {
    let (vault, root, store) = fixture().await;
    let previous = store.session_tasks("note").await.unwrap();
    let mut events = store.subscribe_index_changes();
    assert!(matches!(
        save(&store, root.path(), suggestions(), true).await,
        DocumentSaveOutcome::Saved(_)
    ));
    assert_eq!(
        std::fs::read_to_string(body(vault.path())).unwrap(),
        format!("{PREFIX}{PUBLISHED}")
    );
    let meta = store.read_meta("note").await.unwrap().unwrap();
    assert_eq!(meta.tags, ["attached", "project/new"]);
    assert_eq!(meta.tag_suggestions.as_ref().unwrap().items, ["pending"]);
    assert_eq!(
        meta.tag_suggestions.as_ref().unwrap().dismissed,
        ["ignored"]
    );
    assert_eq!(meta.title, "Keep title");
    assert_eq!(meta.extra["future"], json!({"keep":true}));
    let tasks = store.session_tasks("note").await.unwrap();
    let stable = tasks.iter().find(|task| task.text == "Follow up").unwrap();
    let old = previous
        .iter()
        .find(|task| task.text == "Follow up")
        .unwrap();
    assert_eq!(
        (
            &stable.id,
            &stable.status,
            &stable.assignee,
            &stable.due_at,
            &stable.created_at
        ),
        (
            &old.id,
            &old.status,
            &old.assignee,
            &old.due_at,
            &old.created_at
        )
    );
    assert!(tasks.iter().any(|task| task.text == "Send agenda"));
    assert_eq!(
        tasks.iter().find(|task| task.id == "personal-task"),
        previous.iter().find(|task| task.id == "personal-task")
    );
    let registry: Value =
        serde_json::from_slice(&std::fs::read(vault.path().join("tags.json")).unwrap()).unwrap();
    assert_eq!(registry["future"], json!({"registry":true}));
    assert_eq!(registry["tags"][0]["future"], json!({"keep":true}));
    assert_eq!(registry["tags"].as_array().unwrap().len(), 2);
    let mut entities = std::collections::HashSet::new();
    while let Ok(event) = events.try_recv() {
        entities.insert(format!("{:?}", event.entity));
    }
    for entity in ["Sessions", "Docs", "Tasks", "Tags"] {
        assert!(entities.contains(entity), "{entities:?}");
    }
    assert_eq!(
        std::fs::read_to_string(vault.path().join("sessions/note/notes.md")).unwrap(),
        "Personal note"
    );
    assert_eq!(
        std::fs::read_to_string(vault.path().join("sessions/note/foreign.md")).unwrap(),
        "attachment canary"
    );
    assert!(!intent(root.path()).exists());
}

#[tokio::test]
async fn generated_template_threshold_opt_out_current_dismissed_empty_and_absent_tags_match_summary_policy()
 {
    for (tags, auto, attached, pending) in [
        (
            Some(vec![
                scored("threshold", 0.85),
                scored("below", 0.1),
                scored("above", 0.85001),
            ]),
            true,
            vec!["attached", "above"],
            vec!["threshold", "below"],
        ),
        (
            Some(vec![scored("new", 0.99)]),
            false,
            vec!["attached"],
            vec!["new"],
        ),
        (
            Some(vec![
                scored("ATTACHED", 1.0),
                scored("IGNORED", 1.0),
                scored("new", 0.4),
            ]),
            true,
            vec!["attached"],
            vec!["new"],
        ),
        (Some(vec![]), true, vec!["attached"], vec![]),
        (None, true, vec!["attached"], vec!["pending"]),
    ] {
        let (_vault, root, store) = fixture().await;
        save(&store, root.path(), tags, auto).await;
        let meta = store.read_meta("note").await.unwrap().unwrap();
        let mut attached = attached;
        attached.sort();
        assert_eq!(meta.tags, attached);
        assert_eq!(meta.tag_suggestions.as_ref().unwrap().items, pending);
        assert_eq!(
            meta.tag_suggestions.as_ref().unwrap().dismissed,
            ["ignored"]
        );
    }
}

#[tokio::test]
async fn generated_template_cas_covers_comments_unknown_frontmatter_body_and_metadata() {
    for change in ["comment", "unknown", "title", "template", "order", "body"] {
        let (vault, root, store) = fixture().await;
        let expected = revision(&store).await;
        let raw = std::fs::read_to_string(body(vault.path())).unwrap();
        let next = match change {
            "comment" => raw.replace("Keep this comment", "New comment"),
            "unknown" => raw.replace("one, two", "one, newer"),
            "title" => raw.replace("Existing output", "Renamed output"),
            "template" => raw.replace("retired-template", "other-template"),
            "order" => raw.replace("sort_order: 7", "sort_order: 9"),
            _ => format!("{raw}\nManual body edit"),
        };
        std::fs::write(body(vault.path()), &next).unwrap();
        let before = files(vault.path());
        assert!(matches!(
            store
                .save_generated_template_with_suggestions_if_revision(
                    root.path(),
                    "note",
                    "output",
                    replacement(GENERATED, &expected),
                    suggestions(),
                    true
                )
                .await
                .unwrap(),
            DocumentSaveOutcome::Conflict(_)
        ));
        assert_eq!(files(vault.path()), before, "{change}");
        assert!(!intent(root.path()).exists());
    }
}

#[tokio::test]
async fn generated_template_missing_legacy_recording_and_invalid_suggestions_never_write() {
    for case in ["missing", "legacy", "recording", "nan", "too_many"] {
        let (vault, root, store) = fixture().await;
        if case == "missing" {
            std::fs::remove_file(body(vault.path())).unwrap();
        }
        if case == "legacy" {
            std::fs::write(
                body(vault.path()),
                format!("---\nkind: summary\n---\n\n{BEFORE}"),
            )
            .unwrap();
        }
        let expected = if case == "legacy" {
            "unused".into()
        } else {
            revision(&store).await
        };
        if case == "recording" {
            store.note_recording_active("note");
        }
        let tags = match case {
            "nan" => Some(vec![scored("new", f64::NAN)]),
            "too_many" => Some((0..4).map(|n| scored(&format!("tag{n}"), 1.0)).collect()),
            _ => suggestions(),
        };
        let before = std::fs::read(body(vault.path())).ok();
        assert!(
            store
                .save_generated_template_with_suggestions_if_revision(
                    root.path(),
                    "note",
                    "output",
                    replacement(GENERATED, &expected),
                    tags,
                    true
                )
                .await
                .is_err(),
            "{case}"
        );
        assert_eq!(std::fs::read(body(vault.path())).ok(), before);
        assert!(!intent(root.path()).exists());
    }
}

#[tokio::test]
async fn generated_template_each_file_boundary_recovers_on_restart_without_rewriting_newer_metadata()
 {
    for stage in [
        CommitStage::Intent,
        CommitStage::Body,
        CommitStage::Tasks,
        CommitStage::Metadata,
        CommitStage::Registry,
    ] {
        let (vault, root, store) = fixture().await;
        failed_at(&store, root.path(), stage).await;
        let record: Value =
            serde_json::from_slice(&std::fs::read(intent(root.path())).unwrap()).unwrap();
        assert_eq!(record["schema_version"], 3);
        assert_eq!(record["target"]["kind"], "generated_template_output");
        let meta_path = vault.path().join("sessions/note/_meta.json");
        let mut meta: Value = serde_json::from_slice(&std::fs::read(&meta_path).unwrap()).unwrap();
        meta["title"] = json!("Newer title");
        meta["future"] = json!({"newer":true});
        std::fs::write(&meta_path, meta.to_string()).unwrap();
        let registry_path = vault.path().join("tags.json");
        let mut registry: Value =
            serde_json::from_slice(&std::fs::read(&registry_path).unwrap()).unwrap();
        registry["future"] = json!({"newer_registry":true});
        std::fs::write(&registry_path, registry.to_string()).unwrap();
        drop(store);
        let store = SessionStore::new(vault.path().into());
        store.recover_document_writes(root.path()).await.unwrap();
        assert!(!intent(root.path()).exists());
        assert_eq!(
            std::fs::read_to_string(body(vault.path())).unwrap(),
            format!(
                "{PREFIX}{}",
                if stage == CommitStage::Intent {
                    BEFORE
                } else {
                    PUBLISHED
                }
            )
        );
        let meta = store.read_meta("note").await.unwrap().unwrap();
        assert_eq!(meta.title, "Newer title");
        assert_eq!(meta.extra["future"], json!({"newer":true}));
        assert_eq!(
            meta.tags.contains(&"project/new".into()),
            stage != CommitStage::Intent
        );
        let registry: Value =
            serde_json::from_slice(&std::fs::read(registry_path).unwrap()).unwrap();
        assert_eq!(registry["future"], json!({"newer_registry":true}));
        let before = files(vault.path());
        store.recover_document_writes(root.path()).await.unwrap();
        assert_eq!(files(vault.path()), before);
    }
}

#[tokio::test]
async fn generated_template_recovery_preserves_ambiguous_frontmatter_tasks_tags_and_future_intents()
{
    for case in [
        "frontmatter",
        "tasks",
        "tags",
        "recording",
        "future",
        "old_version",
        "unknown_kind",
    ] {
        let (vault, root, store) = fixture().await;
        failed_at(&store, root.path(), CommitStage::Body).await;
        match case {
            "frontmatter" => {
                let raw = std::fs::read_to_string(body(vault.path())).unwrap();
                std::fs::write(body(vault.path()), raw.replace("one, two", "newer, user")).unwrap();
            }
            "tasks" => {
                let path = vault.path().join("sessions/note/tasks.json");
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                value["tasks"][0]["text"] = json!("Newer manual task");
                std::fs::write(path, value.to_string()).unwrap();
            }
            "tags" => {
                let path = vault.path().join("sessions/note/_meta.json");
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                value["tags"] = json!(["manual-newer"]);
                std::fs::write(path, value.to_string()).unwrap();
            }
            "future" | "old_version" | "unknown_kind" => {
                let path = intent(root.path());
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
                if case == "future" {
                    value["schema_version"] = json!(4);
                } else if case == "old_version" {
                    value["schema_version"] = json!(1);
                } else {
                    value["target"]["kind"] = json!("future_generated_target");
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
        assert!(
            store.recover_document_writes(root.path()).await.is_err(),
            "{case}"
        );
        assert_eq!(files(vault.path()), before, "{case}");
        assert_eq!(
            std::fs::read(intent(root.path())).unwrap(),
            record,
            "{case}"
        );
    }
}

#[test]
fn generated_template_body_replacement_reuses_parser_and_preserves_empty_and_unusual_headers() {
    for raw in [
        PREFIX.to_owned(),
        PREFIX.replace("\r\n", "\n"),
        "---\nkind: template_output\n---".into(),
        format!("  {PREFIX}{BEFORE}"),
    ] {
        let doc = hypr_vault_read::parse_enhanced_file("output", "note", &raw).unwrap();
        let prefix = raw.strip_suffix(&doc.markdown).unwrap();
        for markdown in ["# New", "\n# New", "", "---quoted output"] {
            let next = replace_body(&raw, &doc, markdown).unwrap();
            assert!(next.starts_with(prefix));
            let parsed = hypr_vault_read::parse_enhanced_file("output", "note", &next).unwrap();
            assert_eq!(parsed.markdown, markdown);
            assert_eq!(parsed.kind, "template_output");
        }
    }
}

#[tokio::test]
async fn generated_template_intents_recover_beside_existing_schema_one_two_and_summary_three() {
    let (vault, root, store) = fixture().await;
    failed_at(&store, root.path(), CommitStage::Body).await;
    for id in ["old-document", "old-note", "old-summary"] {
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
                        &DocumentTarget::Summary,
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
                    .document_snapshot_locked(guard, id, &DocumentTarget::Summary)
                    .await
                    .unwrap()
                    .unwrap();
                let metadata = store.read_meta(id).await.unwrap().unwrap();
                let tags = GeneratedTagMetadata::read(&metadata);
                store
                    .begin_generated_summary_write_locked(
                        guard,
                        root.path(),
                        id,
                        GeneratedSummaryWrite {
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
    assert_eq!(versions, [1, 2, 3, 3]);
    drop(store);
    let store = SessionStore::new(vault.path().into());
    store.recover_document_writes(root.path()).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(body(vault.path())).unwrap(),
        format!("{PREFIX}{PUBLISHED}")
    );
    for id in ["old-document", "old-note", "old-summary"] {
        assert_eq!(
            store.read_note(id).await.unwrap().as_deref(),
            Some("Original note")
        );
        assert_eq!(store.read_summary(id).await.unwrap().as_deref(), Some(""));
    }
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn generated_template_commit_serializes_normal_frontmatter_updates() {
    let (vault, root, store) = fixture().await;
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let ready = std::sync::Arc::new(std::sync::Mutex::new(Some(ready_tx)));
    let release = std::sync::Arc::new(tokio::sync::Notify::new());
    let writer = store.clone();
    let operations = root.path().to_path_buf();
    let resume = release.clone();
    let expected = revision(&store).await;
    let task = tokio::spawn(async move {
        writer
            .save_generated_template_with_checkpoint(
                &operations,
                ("note", "output"),
                replacement(GENERATED, &expected),
                suggestions(),
                true,
                move |stage| {
                    let ready = ready.clone();
                    let resume = resume.clone();
                    async move {
                        if stage == CommitStage::Intent {
                            ready.lock().unwrap().take().unwrap().send(()).unwrap();
                            resume.notified().await;
                        }
                        Ok(())
                    }
                },
            )
            .await
    });
    ready_rx.await.unwrap();
    let updater = store.clone();
    let update = tokio::spawn(async move {
        updater
            .update_enhanced_doc(
                "note",
                "output",
                crate::EnhancedDocPatch {
                    title: Some("Newer user title".into()),
                    ..Default::default()
                },
            )
            .await
    });
    release.notify_one();
    assert!(matches!(
        task.await.unwrap().unwrap(),
        DocumentSaveOutcome::Saved(_)
    ));
    update.await.unwrap().unwrap();
    let doc = store
        .read_enhanced_doc("note", "output")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(doc.title, "Newer user title");
    assert_eq!(doc.markdown, PUBLISHED);
    assert_eq!(
        store.read_meta("note").await.unwrap().unwrap().tags,
        ["attached", "project/new"]
    );
    assert!(body(vault.path()).exists());
}

#[tokio::test]
async fn generated_template_killed_process_boundaries_recover_without_losing_header_bytes() {
    for stage in ["intent", "body", "tasks", "metadata", "registry"] {
        let (vault, root, store) = fixture().await;
        drop(store);
        let executable = std::env::current_exe().unwrap();
        let vault_path = vault.path().to_path_buf();
        let operations = root.path().to_path_buf();
        let status = tokio::task::spawn_blocking(move || {
            std::process::Command::new(executable)
                .args([
                    "--ignored",
                    "--exact",
                    "enhanced::generated::tests::generated_template_crash_child",
                ])
                .env("LOOFAH_TEMPLATE_TEST_VAULT", vault_path)
                .env("LOOFAH_TEMPLATE_TEST_OPERATIONS", operations)
                .env("LOOFAH_TEMPLATE_TEST_STAGE", stage)
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(
            status.status.code(),
            Some(91),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        let store = SessionStore::new(vault.path().into());
        store.recover_document_writes(root.path()).await.unwrap();
        assert_eq!(
            std::fs::read_to_string(body(vault.path())).unwrap(),
            format!(
                "{PREFIX}{}",
                if stage == "intent" { BEFORE } else { PUBLISHED }
            )
        );
        assert_eq!(
            store
                .read_meta("note")
                .await
                .unwrap()
                .unwrap()
                .tags
                .contains(&"project/new".into()),
            stage != "intent"
        );
        assert!(!intent(root.path()).exists());
    }
}

#[tokio::test]
#[ignore = "subprocess fixture for abrupt generated-template termination"]
async fn generated_template_crash_child() {
    let Ok(vault) = std::env::var("LOOFAH_TEMPLATE_TEST_VAULT") else {
        return;
    };
    let root = std::path::PathBuf::from(std::env::var("LOOFAH_TEMPLATE_TEST_OPERATIONS").unwrap());
    let stage = std::env::var("LOOFAH_TEMPLATE_TEST_STAGE").unwrap();
    let store = SessionStore::new(vault.into());
    store
        .save_generated_template_with_checkpoint(
            &root,
            ("note", "output"),
            replacement(GENERATED, &revision(&store).await),
            suggestions(),
            true,
            move |at| {
                let selected = match at {
                    CommitStage::Prepared => "prepared",
                    CommitStage::Intent => "intent",
                    CommitStage::Body => "body",
                    CommitStage::Tasks => "tasks",
                    CommitStage::Metadata => "metadata",
                    CommitStage::Registry => "registry",
                };
                if selected == stage {
                    std::process::exit(91);
                }
                async { Ok(()) }
            },
        )
        .await
        .unwrap();
    panic!("requested boundary was not reached");
}

#[tokio::test]
async fn generated_enhanced_stamps_existing_title_with_legacy_markdown_semantics() {
    for (title, markdown, expected) in [
        ("Keep title", GENERATED, PUBLISHED),
        (
            " Keep title ",
            "  # Keep title\n\nAlready headed",
            "  # Keep title\n\nAlready headed",
        ),
        ("", GENERATED, GENERATED),
        ("  ", GENERATED, GENERATED),
        (
            "**Title**\n- [ ] Heading action",
            "# Provider heading\nBody",
            "# **Title**\n- [ ] Heading action\n\n# Provider heading\nBody",
        ),
    ] {
        let (vault, root, store) = fixture().await;
        store
            .update_meta(
                "note",
                crate::SessionMetaPatch {
                    title: Some(title.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let outcome = store
            .save_generated_template_with_suggestions_if_revision(
                root.path(),
                "note",
                "output",
                replacement(markdown, &revision(&store).await),
                None,
                false,
            )
            .await
            .unwrap();
        let DocumentSaveOutcome::Saved(snapshot) = outcome else {
            panic!("expected saved generated document")
        };
        assert_eq!(snapshot.document.unwrap().markdown, expected);
        assert_eq!(store.read_meta("note").await.unwrap().unwrap().title, title);
        if title.contains("Heading action") {
            assert!(
                store
                    .session_tasks("note")
                    .await
                    .unwrap()
                    .iter()
                    .any(|task| task.text == "Heading action")
            );
        }
        assert_eq!(
            std::fs::read_to_string(body(vault.path())).unwrap(),
            format!("{PREFIX}{expected}")
        );
        assert!(!intent(root.path()).exists());
    }
}

#[tokio::test]
async fn generated_enhanced_checks_prepared_title_before_any_publication() {
    for (before_title, after_title) in [
        ("Keep title", "Renamed"),
        ("", "New title"),
        ("Keep title", ""),
    ] {
        let (vault, root, store) = fixture().await;
        store
            .update_meta(
                "note",
                crate::SessionMetaPatch {
                    title: Some(before_title.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let expected_revision = revision(&store).await;
        let before = files(vault.path());
        let updater = store.clone();
        let outcome = store
            .save_generated_template_with_checkpoint(
                root.path(),
                ("note", "output"),
                replacement(GENERATED, &expected_revision),
                None,
                false,
                move |stage| {
                    let updater = updater.clone();
                    async move {
                        if stage == CommitStage::Prepared {
                            updater
                                .update_meta(
                                    "note",
                                    crate::SessionMetaPatch {
                                        title: Some(after_title.into()),
                                        ..Default::default()
                                    },
                                )
                                .await?;
                        }
                        Ok(())
                    }
                },
            )
            .await
            .unwrap();
        assert!(
            matches!(outcome, DocumentSaveOutcome::Conflict(snapshot) if snapshot.revision == expected_revision)
        );
        let after = files(vault.path());
        for index in [0, 2, 3] {
            assert_eq!(before[index], after[index]);
        }
        assert_eq!(
            store.read_meta("note").await.unwrap().unwrap().title,
            after_title
        );
        assert!(!intent(root.path()).exists());
    }
}

#[tokio::test]
async fn generated_enhanced_accepts_unrelated_metadata_edits_after_preparation() {
    let (_vault, root, store) = fixture().await;
    let expected_revision = revision(&store).await;
    let updater = store.clone();
    let outcome = store
        .save_generated_template_with_checkpoint(
            root.path(),
            ("note", "output"),
            replacement(GENERATED, &expected_revision),
            None,
            false,
            move |stage| {
                let updater = updater.clone();
                async move {
                    if stage == CommitStage::Prepared {
                        updater
                            .update_meta(
                                "note",
                                crate::SessionMetaPatch {
                                    tags: Some(vec!["manual".into()]),
                                    ..Default::default()
                                },
                            )
                            .await?;
                    }
                    Ok(())
                }
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(outcome, DocumentSaveOutcome::Saved(snapshot) if snapshot.document.as_ref().unwrap().markdown == PUBLISHED)
    );
    assert_eq!(
        store.read_meta("note").await.unwrap().unwrap().tags,
        ["manual"]
    );
}

#[tokio::test]
async fn generated_enhanced_quota_includes_stamped_title_before_intent() {
    let (vault, root, mut store) = fixture().await;
    store.content_limits.note_write_bytes = GENERATED.len();
    let before = files(vault.path());
    let markdown = GENERATED;
    let outcome = store
        .save_generated_template_with_suggestions_if_revision(
            root.path(),
            "note",
            "output",
            replacement(markdown, &revision(&store).await),
            None,
            false,
        )
        .await;
    assert!(matches!(outcome, Err(StoreError::ResourceLimit(_))));
    assert_eq!(files(vault.path()), before);
    assert!(!intent(root.path()).exists());
    store.content_limits.note_write_bytes = PUBLISHED.len();
    assert!(
        matches!(store.save_generated_template_with_suggestions_if_revision(root.path(), "note", "output", replacement(GENERATED, &revision(&store).await), None, false).await.unwrap(), DocumentSaveOutcome::Saved(snapshot) if snapshot.document.as_ref().unwrap().markdown == PUBLISHED)
    );
}
