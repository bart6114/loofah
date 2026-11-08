use super::*;
use serde_json::json;

fn target() -> DocumentTarget {
    DocumentTarget::TemplateOutput {
        document_id: "output".into(),
    }
}
fn source(store: &SessionStore) -> PathBuf {
    store.vault_base().join("sessions/note/enhanced/output.md")
}
async fn fixture() -> (
    tempfile::TempDir,
    SessionStore,
    PathBuf,
    DeleteDocumentRequest,
) {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = SessionStore::new(root.path().join("vault"));
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Keep","created_at":"2026-11-05T10:00:00Z","tags":[]}),
    )
    .unwrap();
    store.create_note_session(&meta, "Keep note").await.unwrap();
    std::fs::create_dir_all(source(&store).parent().unwrap()).unwrap();
    std::fs::write(source(&store), "---\nkind: template_output\ntitle: Output\nfuture: retained\n---\n# Original\n- [ ] Keep task\n").unwrap();
    let operations = root.path().join("runtime/document-saves");
    let snapshot = store
        .read_document_editor(Some(&operations), "note", &target())
        .await
        .unwrap()
        .unwrap();
    let input = DeleteDocumentRequest {
        expected_revision: snapshot.document.revision,
        expected_session_created_at: snapshot.metadata.meta.created_at,
    };
    (root, store, operations, input)
}
async fn delete(
    store: &SessionStore,
    operations: &Path,
    id: uuid::Uuid,
    input: DeleteDocumentRequest,
) -> Result<DeleteDocumentOutcome, StoreError> {
    store
        .delete_document_checked(operations, "vault:member", id, "note", &target(), input)
        .await
}
fn pending(root: &Path) -> PathBuf {
    root.join(format!("{}.json", crate::sha256(b"note")))
}

#[tokio::test]
async fn checked_deletion_preserves_tasks_metadata_unknown_files_and_retains_exact_output() {
    let (_root, store, operations, input) = fixture().await;
    let directory = store.vault_base().join("sessions/note");
    let tasks = br#"{"future":true,"tasks":[{"id":"keep","source_type":"enhanced_note","source_id":"output","source_order":0,"status":"todo","text":"Keep task","body":[],"due_at":"","assignee":"","created_at":"","updated_at":""}]}"#;
    std::fs::write(directory.join("tasks.json"), tasks).unwrap();
    for name in ["unknown.bin", ".private", "enhanced/another.md"] {
        std::fs::write(directory.join(name), b"keep exact").unwrap();
    }
    let metadata = std::fs::read(directory.join("_meta.json")).unwrap();
    let original = std::fs::read(source(&store)).unwrap();
    let request = uuid::Uuid::new_v4();
    let outcome = delete(&store, &operations, request, input.clone())
        .await
        .unwrap();
    assert!(matches!(outcome, DeleteDocumentOutcome::Deleted { .. }));
    assert!(!source(&store).exists());
    let key = operation_key("vault:member", request);
    let receipt = store
        .read_deletion_receipt(&operations.join(".deletions").join(format!("{key}.json")))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read(
            store
                .vault_base()
                .join(receipt.intent.unwrap().trash_relative())
        )
        .unwrap(),
        original
    );
    assert_eq!(std::fs::read(directory.join("tasks.json")).unwrap(), tasks);
    assert_eq!(
        std::fs::read(directory.join("_meta.json")).unwrap(),
        metadata
    );
    for name in ["unknown.bin", ".private", "enhanced/another.md"] {
        assert_eq!(std::fs::read(directory.join(name)).unwrap(), b"keep exact");
    }
    assert!(!pending(&operations).exists());
    std::fs::write(source(&store), &original).unwrap();
    assert_eq!(
        delete(&store, &operations, request, input.clone())
            .await
            .unwrap(),
        outcome
    );
    assert_eq!(std::fs::read(source(&store)).unwrap(), original);
    let mut changed = input;
    changed.expected_revision = "b".repeat(64);
    assert!(delete(&store, &operations, request, changed).await.is_err());
    assert_eq!(std::fs::read(source(&store)).unwrap(), original);
}

#[tokio::test]
async fn checked_deletion_refuses_changed_document_and_recreated_session_generation() {
    let (_root, store, operations, input) = fixture().await;
    let original = std::fs::read(source(&store)).unwrap();
    let mut changed = original.clone();
    changed.extend_from_slice(b"changed");
    std::fs::write(source(&store), &changed).unwrap();
    assert!(matches!(
        delete(&store, &operations, uuid::Uuid::new_v4(), input.clone())
            .await
            .unwrap(),
        DeleteDocumentOutcome::Conflict { .. }
    ));
    assert_eq!(std::fs::read(source(&store)).unwrap(), changed);
    std::fs::write(source(&store), &original).unwrap();
    let metadata = store.vault_base().join("sessions/note/_meta.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&metadata).unwrap()).unwrap();
    value["created_at"] = "2026-11-06T10:00:00Z".into();
    std::fs::write(metadata, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(matches!(
        delete(&store, &operations, uuid::Uuid::new_v4(), input)
            .await
            .unwrap(),
        DeleteDocumentOutcome::Conflict { .. }
    ));
    assert_eq!(std::fs::read(source(&store)).unwrap(), original);
}

pub(super) fn checkpoint(stage: &str) {
    if std::env::var("LOOFAH_DOCUMENT_DELETE_CRASH").as_deref() == Ok(stage) {
        std::process::exit(86);
    }
}
#[tokio::test]
async fn checked_deletion_crash_child() {
    let Ok(root) = std::env::var("LOOFAH_DOCUMENT_DELETE_CHILD") else {
        return;
    };
    let root = PathBuf::from(root);
    let store = SessionStore::new(root.join("vault"));
    let (request, input): (uuid::Uuid, DeleteDocumentRequest) =
        serde_json::from_slice(&std::fs::read(root.join("request.json")).unwrap()).unwrap();
    delete(&store, &root.join("runtime/document-saves"), request, input)
        .await
        .unwrap();
    panic!("crash hook was not reached");
}
#[tokio::test]
async fn checked_deletion_crash_recovery_never_deletes_on_startup_or_replays_against_restored_copy()
{
    for stage in ["intent", "moved"] {
        let (root, store, operations, input) = fixture().await;
        let original = std::fs::read(source(&store)).unwrap();
        let request = uuid::Uuid::new_v4();
        std::fs::write(
            root.path().join("request.json"),
            serde_json::to_vec(&(request, &input)).unwrap(),
        )
        .unwrap();
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "document_delete::tests::checked_deletion_crash_child",
                "--nocapture",
            ])
            .env("LOOFAH_DOCUMENT_DELETE_CHILD", root.path())
            .env("LOOFAH_DOCUMENT_DELETE_CRASH", stage)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(86));
        assert!(pending(&operations).exists());
        if stage == "moved" {
            assert!(!source(&store).exists());
            std::fs::write(source(&store), &original).unwrap();
        }
        store.recover_document_writes(&operations).await.unwrap();
        assert_eq!(std::fs::read(source(&store)).unwrap(), original);
        assert!(!pending(&operations).exists());
        let outcome = delete(&store, &operations, request, input.clone()).await;
        if stage == "moved" {
            assert!(matches!(
                outcome.unwrap(),
                DeleteDocumentOutcome::Deleted { .. }
            ));
        } else {
            assert!(outcome.is_err());
        }
        assert_eq!(std::fs::read(source(&store)).unwrap(), original);
    }
}

#[tokio::test]
async fn checked_deletion_crash_then_move_back_restore_requires_review_forever_for_original_uuid() {
    let (root, store, operations, input) = fixture().await;
    let original = std::fs::read(source(&store)).unwrap();
    let request = uuid::Uuid::new_v4();
    std::fs::write(
        root.path().join("request.json"),
        serde_json::to_vec(&(request, &input)).unwrap(),
    )
    .unwrap();
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "document_delete::tests::checked_deletion_crash_child",
            "--nocapture",
        ])
        .env("LOOFAH_DOCUMENT_DELETE_CHILD", root.path())
        .env("LOOFAH_DOCUMENT_DELETE_CRASH", "moved")
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(86));
    let receipt_path = operations
        .join(".deletions")
        .join(format!("{}.json", operation_key("vault:member", request)));
    let receipt = store
        .read_deletion_receipt(&receipt_path)
        .await
        .unwrap()
        .unwrap();
    let trash = store
        .vault_base()
        .join(receipt.intent.unwrap().trash_relative());
    assert!(!source(&store).exists());
    std::fs::rename(&trash, source(&store)).unwrap();
    drop(store);
    for _ in 0..2 {
        let reopened = SessionStore::new(root.path().join("vault"));
        reopened.recover_document_writes(&operations).await.unwrap();
        assert!(!pending(&operations).exists());
        assert!(
            delete(&reopened, &operations, request, input.clone())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read(source(&reopened)).unwrap(), original);
        assert!(!trash.exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn checked_deletion_rejects_symlink_source_and_trash_parent_without_changing_bytes() {
    let (root, store, operations, input) = fixture().await;
    let outside = root.path().join("outside.md");
    let original = std::fs::read(source(&store)).unwrap();
    std::fs::write(&outside, &original).unwrap();
    std::fs::remove_file(source(&store)).unwrap();
    std::os::unix::fs::symlink(&outside, source(&store)).unwrap();
    assert!(
        delete(&store, &operations, uuid::Uuid::new_v4(), input.clone())
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&outside).unwrap(), original);
    std::fs::remove_file(source(&store)).unwrap();
    std::fs::write(source(&store), &original).unwrap();
    let external = root.path().join("external-trash");
    std::fs::create_dir(&external).unwrap();
    std::os::unix::fs::symlink(&external, store.vault_base().join(".trash")).unwrap();
    assert!(
        delete(&store, &operations, uuid::Uuid::new_v4(), input)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(source(&store)).unwrap(), original);
    assert_eq!(std::fs::read_dir(external).unwrap().count(), 0);
}

#[tokio::test]
async fn checked_deletion_never_treats_plain_or_legacy_summary_files_as_template_outputs() {
    let (_root, store, operations, input) = fixture().await;
    assert!(
        store
            .delete_document_checked(
                &operations,
                "vault:member",
                uuid::Uuid::new_v4(),
                "note",
                &DocumentTarget::Summary,
                input.clone()
            )
            .await
            .is_err()
    );
    for raw in [
        "User attachment without frontmatter",
        "---\nkind: summary\n---\nLegacy summary",
    ] {
        std::fs::write(source(&store), raw).unwrap();
        assert!(
            delete(&store, &operations, uuid::Uuid::new_v4(), input.clone())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(source(&store)).unwrap(), raw);
    }
    assert!(!pending(&operations).exists());
}
