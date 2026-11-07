use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use hypr_loofah_protocol::{DocumentSaveOutcome, DocumentTarget, SaveDocumentRequest};
use serde_json::json;
use std::sync::Arc;

async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>) {
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Keep title","created_at":"2026-11-07T10:00:00Z","tags":[]}),
    )
    .unwrap();
    store
        .create_note_session(&meta, "Personal note")
        .await
        .unwrap();
    store
        .write_enhanced_doc(&EnhancedDoc {
            id: "output".into(),
            session_id: "note".into(),
            kind: "template_output".into(),
            title: "Keep document title".into(),
            template_id: "retired".into(),
            sort_order: 7,
            markdown: "Before".into(),
        })
        .await
        .unwrap();
    let context = VaultContext::new(
        "vault-a".into(),
        store,
        StartupState::new(Some(&vault)),
        Arc::new(ConfigState::load_or_default(&vault)),
        Arc::new(SearchService::default()),
    )
    .with_operation_storage(root.path().join("operations"));
    (root, Arc::new(context))
}
fn target() -> DocumentTarget {
    DocumentTarget::TemplateOutput {
        document_id: "output".into(),
    }
}
async fn input(context: &VaultContext) -> SaveDocumentRequest {
    SaveDocumentRequest {
        markdown: "# Actions\n- [ ] Send agenda".into(),
        expected_revision: context
            .read_document("note", &target())
            .await
            .unwrap()
            .revision,
    }
}
fn tags() -> Option<Vec<hypr_vault_write::ScoredTagSuggestion>> {
    Some(vec![hypr_vault_write::ScoredTagSuggestion {
        name: "Project/New".into(),
        confidence: 0.99,
    }])
}

#[tokio::test]
async fn generated_template_uses_current_policy_and_requires_an_existing_template() {
    for auto in [true, false] {
        let (root, context) = fixture().await;
        context
            .settings
            .set_values(std::collections::HashMap::from([(
                "auto_apply_high_confidence_tags".into(),
                json!(auto),
            )]))
            .await
            .unwrap();
        assert!(matches!(
            context
                .save_generated_template_output("note", "output", input(&context).await, tags())
                .await
                .unwrap(),
            DocumentSaveOutcome::Saved(_)
        ));
        let meta = context.store.read_meta("note").await.unwrap().unwrap();
        assert_eq!(meta.tags.contains(&"project/new".into()), auto);
        assert_eq!(meta.title, "Keep title");
        let before = std::fs::read(root.path().join("vault/sessions/note/_meta.json")).unwrap();
        let missing = DocumentTarget::TemplateOutput {
            document_id: "missing".into(),
        };
        let expected = context
            .read_document("note", &missing)
            .await
            .unwrap()
            .revision;
        assert_eq!(
            context
                .save_generated_template_output(
                    "note",
                    "missing",
                    SaveDocumentRequest {
                        markdown: "no creation".into(),
                        expected_revision: expected
                    },
                    tags()
                )
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(
            std::fs::read(root.path().join("vault/sessions/note/_meta.json")).unwrap(),
            before
        );
        assert!(
            !root
                .path()
                .join("vault/sessions/note/enhanced/missing.md")
                .exists()
        );
        std::fs::write(
            root.path().join("vault/sessions/note/enhanced/output.md"),
            "---\nkind: summary\n---\n\nLegacy body",
        )
        .unwrap();
        assert_eq!(
            context
                .save_generated_template_output(
                    "note",
                    "output",
                    SaveDocumentRequest {
                        markdown: "no migration".into(),
                        expected_revision: "unused".into()
                    },
                    tags()
                )
                .await
                .unwrap_err()
                .code,
            ErrorCode::Validation
        );
    }
}

#[tokio::test]
async fn cancelled_generated_template_retains_context_and_vault_lease_until_actual_io_finishes() {
    let (root, context) = fixture().await;
    let request = input(&context).await;
    let mut context = Arc::try_unwrap(context).ok().unwrap();
    let lock_path = root.path().join("lease");
    let lease = std::fs::File::create(&lock_path).unwrap();
    lease.try_lock().unwrap();
    context.lease = Some(lease);
    let context = Arc::new(context);
    let weak = Arc::downgrade(&context);
    let competitor = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .unwrap();
    let held = context.store.lock_snapshot().await;
    let caller = context.clone();
    let task = tokio::spawn(async move {
        caller
            .save_generated_template_output("note", "output", request, tags())
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while Arc::strong_count(&context) < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(context);
    assert!(weak.upgrade().is_some());
    assert!(competitor.try_lock().is_err());
    drop(held);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    competitor.try_lock().unwrap();
    let raw = std::fs::read_to_string(root.path().join("vault/sessions/note/enhanced/output.md"))
        .unwrap();
    assert!(raw.contains("Send agenda"));
    assert_eq!(
        std::fs::read_dir(root.path().join("operations/document-saves"))
            .unwrap()
            .count(),
        0
    );
}
