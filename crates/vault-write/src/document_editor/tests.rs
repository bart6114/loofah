use super::*;
use serde_json::{Value, json};

fn root() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}
async fn fixture() -> (tempfile::TempDir, SessionStore, PathBuf) {
    let root = root();
    let store = SessionStore::new(root.path().join("vault"));
    let metadata = serde_json::from_value(json!({"id":"note","title":"Original","created_at":"2026-11-05T10:00:00Z","tags":["keep"],"future":{"nested":[1,true]}})).unwrap();
    store
        .create_note_session(&metadata, "# Raw title\nOriginal note")
        .await
        .unwrap();
    let operations = root.path().join("runtime/document-saves");
    (root, store, operations)
}
fn file(store: &SessionStore, name: &str) -> PathBuf {
    store.vault_base().join("sessions/note").join(name)
}
fn write(store: &SessionStore, name: &str, bytes: impl AsRef<[u8]>) {
    let path = file(store, name);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
fn raw_task(id: &str, kind: &str, source: &str) -> Value {
    json!({"id":id,"source_type":kind,"source_id":source,"source_order":0,"status":"todo","text":"Before","body":[],"due_at":"","assignee":"bart","created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z","future":{"retain":true}})
}
fn task(id: &str) -> TaskInput {
    TaskInput {
        id: id.into(),
        source_order: 0,
        status: "done".into(),
        text: "After".into(),
        body: json!([]),
        due_at: "".into(),
    }
}
fn request(snapshot: &DocumentEditorSnapshot) -> SaveDocumentEditorRequest {
    SaveDocumentEditorRequest {
        markdown: "# Edited\nBody".into(),
        title: Some("Manual title".into()),
        tasks: vec![task("owned")],
        expected_revision: snapshot.document.revision.clone(),
        expected_metadata_revision: snapshot.metadata.revision.clone(),
        expected_tasks_revision: snapshot.tasks.revision.clone(),
    }
}
async fn read(
    store: &SessionStore,
    operations: &Path,
    target: &DocumentTarget,
) -> DocumentEditorSnapshot {
    store
        .read_document_editor(Some(operations), "note", target)
        .await
        .unwrap()
        .unwrap()
}
async fn prepare(
    store: &SessionStore,
    operations: &Path,
    target: &DocumentTarget,
    input: SaveDocumentEditorRequest,
) -> DocumentEditorPlan {
    match store
        .prepare_document_editor(operations, "note", target, input)
        .await
        .unwrap()
        .unwrap()
    {
        DocumentEditorPreparation::Prepared(plan) => *plan,
        DocumentEditorPreparation::Conflict(_) => panic!("unexpected conflict"),
    }
}
fn intent_path(operations: &Path) -> PathBuf {
    operations.join(format!("{}.json", crate::sha256(b"note")))
}

#[tokio::test]
async fn summary_compound_save_preserves_raw_metadata_tasks_attachments_and_other_documents() {
    let (_root, store, operations) = fixture().await;
    write(&store, "summary.md", "Before summary");
    write(
        &store,
        "enhanced/template.md",
        "---\nkind: template_output\ntitle: Template\nfuture: nested\n---\nUnchanged output",
    );
    write(&store, "attachments/owned-by-user.txt", "Attachment");
    write(&store, "unknown.txt", "Unknown attachment");
    let foreign = raw_task("foreign", "enhanced_note", "template");
    write(&store,"tasks.json",serde_json::to_vec(&json!({"future_root":{"keep":true},"tasks":[raw_task("owned","session_summary","note"),foreign.clone()]})).unwrap());
    let metadata_before: Value =
        serde_json::from_slice(&std::fs::read(file(&store, "_meta.json")).unwrap()).unwrap();
    let raw_note = std::fs::read(file(&store, "notes.md")).unwrap();
    let other = std::fs::read(file(&store, "enhanced/template.md")).unwrap();
    let before = read(&store, &operations, &DocumentTarget::Summary).await;
    assert_eq!(before.tasks.tasks.len(), 1);
    let after = prepare(
        &store,
        &operations,
        &DocumentTarget::Summary,
        request(&before),
    )
    .await
    .commit()
    .await
    .unwrap();
    assert_eq!(after.metadata.meta.title, "Manual title");
    assert_eq!(after.document.document.unwrap().markdown, "# Edited\nBody");
    assert_eq!(after.tasks.tasks[0].source_type, "session_summary");
    assert_eq!(after.tasks.tasks[0].assignee, "bart");
    let mut metadata_after = metadata_before;
    metadata_after["title"] = "Manual title".into();
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(file(&store, "_meta.json")).unwrap())
            .unwrap(),
        metadata_after
    );
    let tasks: Value =
        serde_json::from_slice(&std::fs::read(file(&store, "tasks.json")).unwrap()).unwrap();
    assert_eq!(tasks["future_root"], json!({"keep":true}));
    assert_eq!(tasks["tasks"][0], foreign);
    assert_eq!(tasks["tasks"][1]["future"], json!({"retain":true}));
    assert_eq!(std::fs::read(file(&store, "notes.md")).unwrap(), raw_note);
    assert_eq!(
        std::fs::read(file(&store, "enhanced/template.md")).unwrap(),
        other
    );
    assert_eq!(
        std::fs::read(file(&store, "unknown.txt")).unwrap(),
        b"Unknown attachment"
    );
    assert_eq!(
        std::fs::read(file(&store, "attachments/owned-by-user.txt")).unwrap(),
        b"Attachment"
    );
    assert!(!intent_path(&operations).exists());
}

#[tokio::test]
async fn template_preserves_exact_frontmatter_and_never_creates_missing_output() {
    let (_root, store, operations) = fixture().await;
    let target = DocumentTarget::TemplateOutput {
        document_id: "template".into(),
    };
    assert!(
        store
            .read_document_editor(Some(&operations), "note", &target)
            .await
            .unwrap()
            .is_none()
    );
    let initial = read(&store, &operations, &DocumentTarget::Summary).await;
    assert!(
        store
            .prepare_document_editor(&operations, "note", &target, request(&initial))
            .await
            .unwrap()
            .is_none()
    );
    let prefix = "---\nkind: template_output\ntitle: 'Output title'\ntemplate_id: custom\nfuture:\n  nested: [1, true]\n---\n";
    write(&store, "enhanced/template.md", format!("{prefix}Old"));
    let before = read(&store, &operations, &target).await;
    let after = prepare(&store, &operations, &target, request(&before))
        .await
        .commit()
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(file(&store, "enhanced/template.md")).unwrap(),
        format!("{prefix}# Edited\nBody")
    );
    assert_eq!(after.document.document.unwrap().title, "Output title");
    assert_eq!(after.tasks.tasks[0].source_id, "template");
}

#[tokio::test]
async fn missing_summary_can_be_created_only_after_all_three_baselines_match() {
    let (_root, store, operations) = fixture().await;
    let initial = read(&store, &operations, &DocumentTarget::Summary).await;
    assert!(initial.document.document.is_none());
    let after = prepare(
        &store,
        &operations,
        &DocumentTarget::Summary,
        request(&initial),
    )
    .await
    .commit()
    .await
    .unwrap();
    assert_eq!(after.document.document.unwrap().markdown, "# Edited\nBody");
    assert!(file(&store, "summary.md").exists());
}

#[tokio::test]
async fn legacy_summary_projection_is_read_only_and_save_keeps_location_and_aliases() {
    let (_root, store, operations) = fixture().await;
    let prefix = "---\nkind: summary\ntitle: Legacy\nsort_order: -1\nfuture: keep\n---\n";
    write(&store, "summary.md", "Canonical shadow");
    write(&store, "enhanced/legacy.md", format!("{prefix}Legacy body"));
    write(&store,"tasks.json",serde_json::to_vec(&json!({"tasks":[raw_task("owned","enhanced_note","legacy"),raw_task("canonical","session_summary","note"),raw_task("foreign","enhanced_note","other")]})).unwrap());
    let raw_before = std::fs::read(file(&store, "tasks.json")).unwrap();
    let before = read(&store, &operations, &DocumentTarget::Summary).await;
    assert_eq!(
        before.document.document.as_ref().unwrap().markdown,
        "Legacy body"
    );
    assert_eq!(before.tasks.tasks.len(), 2);
    assert!(
        before
            .tasks
            .tasks
            .iter()
            .all(|task| task.source_type == "session_summary" && task.source_id == "note")
    );
    assert_eq!(
        std::fs::read(file(&store, "tasks.json")).unwrap(),
        raw_before
    );
    assert!(!operations.exists());
    let mut input = request(&before);
    input.tasks.push(task("canonical"));
    input.tasks.push(task("new"));
    prepare(&store, &operations, &DocumentTarget::Summary, input)
        .await
        .commit()
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(file(&store, "summary.md")).unwrap(),
        "Canonical shadow"
    );
    assert_eq!(
        std::fs::read_to_string(file(&store, "enhanced/legacy.md")).unwrap(),
        format!("{prefix}# Edited\nBody")
    );
    let rows: Value =
        serde_json::from_slice(&std::fs::read(file(&store, "tasks.json")).unwrap()).unwrap();
    for (id, kind, source) in [
        ("owned", "enhanced_note", "legacy"),
        ("canonical", "session_summary", "note"),
        ("new", "enhanced_note", "legacy"),
    ] {
        let row = rows["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .unwrap();
        assert_eq!(row["source_type"], kind);
        assert_eq!(row["source_id"], source);
    }
}

#[tokio::test]
async fn stale_document_metadata_or_unrelated_tasks_returns_whole_coherent_conflict_without_writes()
{
    for changed in ["summary.md", "_meta.json", "tasks.json"] {
        let (_root, store, operations) = fixture().await;
        write(&store, "summary.md", "Before");
        let before = read(&store, &operations, &DocumentTarget::Summary).await;
        match changed {
            "summary.md" => write(&store, changed, "Other editor"),
            "_meta.json" => {
                let mut raw: Value =
                    serde_json::from_slice(&std::fs::read(file(&store, changed)).unwrap()).unwrap();
                raw["future"] = json!({"changed":true});
                write(&store, changed, serde_json::to_vec(&raw).unwrap());
            }
            _ => write(
                &store,
                changed,
                serde_json::to_vec(&json!({"tasks":[raw_task("other","enhanced_note","other")]}))
                    .unwrap(),
            ),
        }
        let actual = read(&store, &operations, &DocumentTarget::Summary).await;
        let DocumentEditorPreparation::Conflict(conflict) = store
            .prepare_document_editor(
                &operations,
                "note",
                &DocumentTarget::Summary,
                request(&before),
            )
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("expected conflict")
        };
        assert_eq!(*conflict, actual);
        assert!(!operations.exists());
        assert_eq!(
            read(&store, &operations, &DocumentTarget::Summary).await,
            actual
        );
    }
}

#[tokio::test]
async fn foreign_task_id_is_rejected_before_intent_in_compound_note_and_checked_task_operations() {
    use hypr_loofah_protocol::{SaveNoteEditorRequest, TaskSource};
    let (_root, store, operations) = fixture().await;
    write(&store, "summary.md", "Before");
    write(
        &store,
        "tasks.json",
        serde_json::to_vec(&json!({"tasks":[raw_task("owned","enhanced_note","foreign")]}))
            .unwrap(),
    );
    let unchanged = std::fs::read(file(&store, "tasks.json")).unwrap();
    let before = read(&store, &operations, &DocumentTarget::Summary).await;
    assert!(matches!(
        store
            .prepare_document_editor(
                &operations,
                "note",
                &DocumentTarget::Summary,
                request(&before)
            )
            .await,
        Err(StoreError::Conflict(_))
    ));
    let note = store
        .read_note_editor(Some(&operations), "note")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        store
            .save_note_editor(
                &operations,
                "note",
                SaveNoteEditorRequest {
                    markdown: "Changed".into(),
                    title: None,
                    tasks: vec![task("owned")],
                    expected_revision: note.note.revision,
                    expected_tasks_revision: note.tasks.revision
                }
            )
            .await,
        Err(StoreError::Conflict(_))
    ));
    assert!(matches!(
        store
            .replace_tasks_if_revision(
                &operations,
                "note",
                &TaskSource::Summary,
                vec![task("owned")],
                &before.tasks.revision
            )
            .await,
        Err(StoreError::Conflict(_))
    ));
    assert_eq!(
        std::fs::read(file(&store, "tasks.json")).unwrap(),
        unchanged
    );
    assert_eq!(
        std::fs::read_to_string(file(&store, "summary.md")).unwrap(),
        "Before"
    );
    assert!(!operations.exists());
}

#[tokio::test]
async fn prospective_body_frontmatter_metadata_tasks_and_journal_budgets_fail_before_any_write() {
    for part in [
        "body",
        "frontmatter",
        "metadata",
        "tasks",
        "journal",
        "request",
        "task_request",
    ] {
        let (_root, base, operations) = fixture().await;
        write(&base, "summary.md", "Old");
        let mut limits = crate::ContentLimits::default();
        let target = if part == "frontmatter" {
            write(
                &base,
                "enhanced/template.md",
                "---\nkind: template_output\nfuture: long-preserved-prefix\n---\nOld",
            );
            DocumentTarget::TemplateOutput {
                document_id: "template".into(),
            }
        } else {
            DocumentTarget::Summary
        };
        let before = read(&base, &operations, &target).await;
        let mut input = request(&before);
        match part {
            "body" => limits.note_write_bytes = 2,
            "frontmatter" => {
                limits.note_write_bytes = 20;
                input.markdown = "Small".into();
            }
            "metadata" => {
                limits.metadata_bytes =
                    std::fs::metadata(file(&base, "_meta.json")).unwrap().len() as usize + 1
            }
            "tasks" => limits.tasks_file_bytes = 1,
            "journal" => limits.recovery_bytes = 1,
            "request" => limits.editor_request_bytes = 1,
            _ => limits.task_request_bytes = 1,
        }
        if part == "metadata" {
            input.title = Some("T".repeat(1000));
        }
        let store = SessionStore::with_content_limits(base.vault_base().to_owned(), limits);
        let before_files = (
            std::fs::read(file(&base, "_meta.json")).unwrap(),
            std::fs::read(file(&base, "summary.md")).unwrap(),
        );
        assert!(
            matches!(
                store
                    .prepare_document_editor(&operations, "note", &target, input)
                    .await,
                Err(StoreError::ResourceLimit(_))
            ),
            "{part}"
        );
        assert_eq!(
            std::fs::read(file(&base, "_meta.json")).unwrap(),
            before_files.0
        );
        assert_eq!(
            std::fs::read(file(&base, "summary.md")).unwrap(),
            before_files.1
        );
        assert!(!file(&base, "tasks.json").exists());
        assert!(!operations.exists());
    }
}

#[tokio::test]
async fn no_op_commit_does_not_create_intent_or_rewrite_files() {
    let (_root, store, operations) = fixture().await;
    write(&store, "summary.md", "Unchanged");
    let before = read(&store, &operations, &DocumentTarget::Summary).await;
    let mut input = request(&before);
    input.markdown = "Unchanged".into();
    input.title = None;
    input.tasks.clear();
    assert_eq!(
        prepare(&store, &operations, &DocumentTarget::Summary, input)
            .await
            .commit()
            .await
            .unwrap(),
        before
    );
    assert!(!operations.exists());
    assert!(!file(&store, "tasks.json").exists());
}

pub(super) fn checkpoint(stage: &str) {
    if std::env::var("LOOFAH_DOCUMENT_EDITOR_CRASH").as_deref() == Ok(stage) {
        std::process::exit(86);
    }
}
#[tokio::test]
async fn compound_editor_crash_child() {
    let Ok(root) = std::env::var("LOOFAH_DOCUMENT_EDITOR_CHILD") else {
        return;
    };
    let root = PathBuf::from(root);
    let store = SessionStore::new(root.join("vault"));
    let operations = root.join("runtime/document-saves");
    let before = read(&store, &operations, &DocumentTarget::Summary).await;
    prepare(
        &store,
        &operations,
        &DocumentTarget::Summary,
        request(&before),
    )
    .await
    .commit()
    .await
    .unwrap();
    panic!("configured crash checkpoint did not fire");
}
#[tokio::test]
async fn compound_editor_recovers_real_process_exit_at_each_durable_boundary() {
    for stage in [
        "intent", "document", "metadata", "tasks", "readback", "removed",
    ] {
        let (root, store, operations) = fixture().await;
        write(&store, "summary.md", "Before");
        let child_root = root.path().to_owned();
        let status = tokio::task::spawn_blocking(move || {
            std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "document_editor::tests::compound_editor_crash_child",
                    "--nocapture",
                ])
                .env("LOOFAH_DOCUMENT_EDITOR_CHILD", child_root)
                .env("LOOFAH_DOCUMENT_EDITOR_CRASH", stage)
                .status()
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(status.code(), Some(86), "{stage}");
        let reopened = SessionStore::new(store.vault_base().to_owned());
        let snapshot = read(&reopened, &operations, &DocumentTarget::Summary).await;
        if stage == "intent" {
            assert_eq!(snapshot.document.document.unwrap().markdown, "Before");
            assert_eq!(snapshot.metadata.meta.title, "Original");
            assert!(snapshot.tasks.tasks.is_empty());
        } else {
            assert_eq!(
                snapshot.document.document.unwrap().markdown,
                "# Edited\nBody"
            );
            assert_eq!(snapshot.metadata.meta.title, "Manual title");
            assert_eq!(snapshot.tasks.tasks.len(), 1);
        }
        assert!(!intent_path(&operations).exists(), "{stage}");
    }
}

async fn leave_intent(store: &SessionStore, operations: &Path) -> recovery::Intent {
    let before = read(store, operations, &DocumentTarget::Summary).await;
    let plan = prepare(
        store,
        operations,
        &DocumentTarget::Summary,
        request(&before),
    )
    .await;
    let record = plan.record.clone();
    store
        .begin_editor_write_locked(&plan.guard._guard, operations, record.clone())
        .await
        .unwrap();
    drop(plan);
    record
}
#[tokio::test]
async fn recovery_rejects_external_divergence_and_deleted_target_without_resurrection() {
    for changed in ["document", "metadata", "tasks", "deleted"] {
        let (_root, store, operations) = fixture().await;
        write(&store, "summary.md", "Before");
        leave_intent(&store, &operations).await;
        write(&store, "summary.md", "# Edited\nBody");
        match changed {
            "document" => write(&store, "summary.md", "External"),
            "metadata" => {
                let mut value: Value =
                    serde_json::from_slice(&std::fs::read(file(&store, "_meta.json")).unwrap())
                        .unwrap();
                value["title"] = "External".into();
                write(&store, "_meta.json", serde_json::to_vec(&value).unwrap());
            }
            "tasks" => write(
                &store,
                "tasks.json",
                serde_json::to_vec(
                    &json!({"tasks":[raw_task("external","session_summary","note")]}),
                )
                .unwrap(),
            ),
            _ => std::fs::remove_file(file(&store, "summary.md")).unwrap(),
        }
        let intent = std::fs::read(intent_path(&operations)).unwrap();
        assert!(
            store
                .read_document_editor(Some(&operations), "note", &DocumentTarget::Summary)
                .await
                .is_err(),
            "{changed}"
        );
        assert_eq!(std::fs::read(intent_path(&operations)).unwrap(), intent);
        if changed == "deleted" {
            assert!(!file(&store, "summary.md").exists());
        }
    }
}
#[tokio::test]
async fn corrupt_future_duplicate_and_wrong_identity_recovery_records_fail_closed_unchanged() {
    for corruption in [
        "future",
        "duplicate",
        "identity",
        "nested_duplicate",
        "target_field",
        "location_field",
        "task_ids",
        "corrupt",
        "after_hash",
    ] {
        let (_root, store, operations) = fixture().await;
        write(&store, "summary.md", "Before");
        leave_intent(&store, &operations).await;
        let path = intent_path(&operations);
        let bytes = std::fs::read(&path).unwrap();
        let mut raw: Value = serde_json::from_slice(&bytes).unwrap();
        let bad = match corruption {
            "future" => {
                raw["schema_version"] = 99.into();
                serde_json::to_vec(&raw).unwrap()
            }
            "identity" => {
                raw["session_id"] = "other".into();
                serde_json::to_vec(&raw).unwrap()
            }
            "after_hash" => {
                raw["after_metadata"] = "0".repeat(64).into();
                serde_json::to_vec(&raw).unwrap()
            }
            "target_field" => {
                raw["target"]["future"] = true.into();
                serde_json::to_vec(&raw).unwrap()
            }
            "location_field" => {
                raw["location"]["future"] = true.into();
                serde_json::to_vec(&raw).unwrap()
            }
            "task_ids" => {
                let tasks = raw["tasks"]["tasks"].as_array_mut().unwrap();
                tasks.push(tasks[0].clone());
                raw["after_tasks"] = crate::tasks_revision::revision(
                    "note",
                    Some(&serde_json::to_vec_pretty(&raw["tasks"]).unwrap()),
                )
                .into();
                serde_json::to_vec(&raw).unwrap()
            }
            "duplicate" => String::from_utf8(bytes)
                .unwrap()
                .replacen('{', "{\"schema_version\":5,", 1)
                .into_bytes(),
            "nested_duplicate" => String::from_utf8(bytes)
                .unwrap()
                .replace("\"nested\":", "\"nested\":null,\"nested\":")
                .into_bytes(),
            _ => b"{not JSON".to_vec(),
        };
        std::fs::write(&path, &bad).unwrap();
        assert!(
            store
                .read_document_editor(Some(&operations), "note", &DocumentTarget::Summary)
                .await
                .is_err(),
            "{corruption}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bad);
        assert_eq!(
            std::fs::read_to_string(file(&store, "summary.md")).unwrap(),
            "Before"
        );
    }
}

#[tokio::test]
async fn duplicate_metadata_or_task_keys_and_duplicate_task_ids_are_never_rewritten() {
    for (name,content) in [("_meta.json",b"{\"id\":\"note\",\"id\":\"note\"}".to_vec()),("tasks.json",b"{\"tasks\":[],\"tasks\":[]}".to_vec()),("tasks.json",serde_json::to_vec(&json!({"tasks":[raw_task("same","session_summary","note"),raw_task("same","session_summary","note")]})).unwrap())] {
        let (_root,store,operations)=fixture().await;write(&store,name,&content);
        assert!(store.read_document_editor(Some(&operations),"note",&DocumentTarget::Summary).await.is_err());
        assert_eq!(std::fs::read(file(&store,name)).unwrap(),content);assert!(!operations.exists());
    }
}

#[tokio::test]
async fn prepared_plan_holds_write_lease_until_drop_and_rechecks_external_edits_at_commit() {
    let (_root, store, operations) = fixture().await;
    write(&store, "summary.md", "Before");
    let before = read(&store, &operations, &DocumentTarget::Summary).await;
    let plan = prepare(
        &store,
        &operations,
        &DocumentTarget::Summary,
        request(&before),
    )
    .await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), store.lock_snapshot())
            .await
            .is_err()
    );
    write(&store, "summary.md", "External");
    assert!(matches!(plan.commit().await, Err(StoreError::Conflict(_))));
    assert_eq!(
        std::fs::read_to_string(file(&store, "summary.md")).unwrap(),
        "External"
    );
    assert!(!operations.exists());
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), store.lock_snapshot())
            .await
            .is_ok()
    );
}

#[tokio::test]
async fn remote_legacy_inspection_bounds_all_names_without_reading_unknown_content() {
    let (_root, base, operations) = fixture().await;
    write(&base, "summary.md", "Before");
    for index in 0..4097 {
        write(
            &base,
            &format!("enhanced/.unknown-{index}"),
            b"not document content",
        );
    }
    assert!(matches!(
        base.read_document_editor(Some(&operations), "note", &DocumentTarget::Summary)
            .await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert!(!operations.exists());
    assert_eq!(
        std::fs::read(file(&base, "enhanced/.unknown-0")).unwrap(),
        b"not document content"
    );
    let local = SessionStore::with_content_limits(
        base.vault_base().to_owned(),
        crate::ContentLimits::unlimited(),
    );
    assert_eq!(
        read(&local, &operations, &DocumentTarget::Summary)
            .await
            .document
            .document
            .unwrap()
            .markdown,
        "Before"
    );
}

#[tokio::test]
async fn large_raw_task_reconciliation_preserves_every_unrelated_row_and_unknown_field() {
    let (_root, store, operations) = fixture().await;
    write(&store, "summary.md", "Before");
    let foreign: Vec<_> = (0..1500)
        .map(|index| raw_task(&format!("foreign-{index}"), "enhanced_note", "other"))
        .collect();
    let owned: Vec<_> = (0..1500)
        .map(|index| raw_task(&format!("owned-{index}"), "session_summary", "note"))
        .collect();
    let rows: Vec<_> = foreign.iter().chain(&owned).cloned().collect();
    write(
        &store,
        "tasks.json",
        serde_json::to_vec(&json!({"future_root":"keep","tasks":rows})).unwrap(),
    );
    let before = read(&store, &operations, &DocumentTarget::Summary).await;
    let mut input = request(&before);
    input.tasks = (0..1500)
        .map(|index| task(&format!("owned-{index}")))
        .collect();
    let after = prepare(&store, &operations, &DocumentTarget::Summary, input)
        .await
        .commit()
        .await
        .unwrap();
    assert_eq!(after.tasks.tasks.len(), 1500);
    let raw: Value =
        serde_json::from_slice(&std::fs::read(file(&store, "tasks.json")).unwrap()).unwrap();
    assert_eq!(raw["future_root"], "keep");
    let rows = raw["tasks"].as_array().unwrap();
    assert_eq!(rows.len(), 3000);
    assert_eq!(&rows[..1500], foreign.as_slice());
    assert!(
        rows[1500..]
            .iter()
            .all(|row| row["future"] == json!({"retain":true}) && row["assignee"] == "bart")
    );
}
