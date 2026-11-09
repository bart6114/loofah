use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use hypr_vault_write::{ContentLimits, SessionStore};
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};

async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>) {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let vault = root.path().join("vault");
    let store = Arc::new(SessionStore::new(vault.clone()));
    let context = Arc::new(
        VaultContext::new(
            "test".into(),
            store,
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.path().join("runtime")),
    );
    let meta = serde_json::from_value(
        json!({"id":"note","title":"Before","created_at":"2026-11-05T10:00:00Z","tags":[]}),
    )
    .unwrap();
    context
        .store
        .create_note_session(&meta, "Raw note")
        .await
        .unwrap();
    (root, context)
}
fn request(snapshot: &DocumentEditorSnapshot) -> SaveDocumentEditorRequest {
    SaveDocumentEditorRequest {
        markdown: "# Edited summary".into(),
        title: Some("Edited title".into()),
        tasks: vec![],
        expected_revision: snapshot.document.revision.clone(),
        expected_metadata_revision: snapshot.metadata.revision.clone(),
        expected_tasks_revision: snapshot.tasks.revision.clone(),
    }
}
#[tokio::test]
async fn compound_editor_readiness_targets_and_coherent_saved_and_conflict_snapshots() {
    let (_root, context) = fixture().await;
    context.store.set_startup_pending(true);
    assert_eq!(
        context
            .read_document_editor("note", &DocumentTarget::Summary)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    context.store.set_startup_pending(false);
    assert_eq!(
        context
            .read_document_editor("../note", &DocumentTarget::Summary)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
    assert_eq!(
        context
            .read_document_editor(
                "note",
                &DocumentTarget::TemplateOutput {
                    document_id: "missing".into()
                }
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::NotFound
    );
    let before = context
        .read_document_editor("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let input = request(&before);
    let DocumentEditorSaveOutcome::Saved(after) = context
        .save_document_editor("note", &DocumentTarget::Summary, input.clone())
        .await
        .unwrap()
    else {
        panic!("not saved")
    };
    assert_eq!(
        after.document.document.as_ref().unwrap().markdown,
        "# Edited summary"
    );
    assert_eq!(after.metadata.meta.title, "Edited title");
    let DocumentEditorSaveOutcome::Conflict(conflict) = context
        .save_document_editor("note", &DocumentTarget::Summary, input)
        .await
        .unwrap()
    else {
        panic!("not conflict")
    };
    assert_eq!(conflict, after);
}

#[tokio::test]
async fn denied_grant_exposes_neither_conflict_snapshot_nor_prepared_write() {
    let (_root, context) = fixture().await;
    let before = context
        .read_document_editor("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    for conflict in [false, true] {
        let mut input = request(&before);
        if conflict {
            input.expected_metadata_revision = "stale".into();
        }
        let called = Arc::new(AtomicUsize::new(0));
        let seen = called.clone();
        let error = context
            .save_document_editor_authorized(
                "note",
                &DocumentTarget::Summary,
                input,
                move || async move {
                    seen.fetch_add(1, Ordering::SeqCst);
                    Err::<(), _>(error(ErrorCode::Forbidden, "Revoked"))
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Forbidden);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
        assert_eq!(called.load(Ordering::SeqCst), 1);
        assert_eq!(
            context
                .read_document_editor("note", &DocumentTarget::Summary)
                .await
                .unwrap(),
            before
        );
        assert!(!context.document_operation_root().unwrap().exists());
    }
    let denied = context
        .read_document_editor_authorized("note", &DocumentTarget::Summary, || async {
            Err::<(), _>(error(ErrorCode::Forbidden, "Revoked"))
        })
        .await
        .unwrap_err();
    assert_eq!(denied.code, ErrorCode::Forbidden);
}

#[tokio::test]
async fn caller_abort_retains_context_authorization_and_grant_until_actual_commit() {
    let (_root, context) = fixture().await;
    let before = context
        .read_document_editor("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = tokio::sync::oneshot::channel();
    let (dropped_tx, mut dropped_rx) = tokio::sync::oneshot::channel();
    let storage_guard = context.store.lock_snapshot().await;
    struct Grant(Option<tokio::sync::oneshot::Sender<()>>);
    impl Drop for Grant {
        fn drop(&mut self) {
            if let Some(tx) = self.0.take() {
                let _ = tx.send(());
            }
        }
    }
    let owned = context.clone();
    let input = request(&before);
    let task = tokio::spawn(async move {
        owned
            .save_document_editor_authorized(
                "note",
                &DocumentTarget::Summary,
                input,
                move || async move {
                    let _ = entered_tx.send(());
                    release_rx.await.unwrap();
                    Ok(Grant(Some(dropped_tx)))
                },
            )
            .await
    });
    entered_rx.await.unwrap();
    task.abort();
    let _ = task.await;
    let weak = Arc::downgrade(&context);
    let vault = context.store.vault_base().to_owned();
    drop(context);
    assert!(weak.upgrade().is_some());
    release_tx.send(()).unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut dropped_rx)
            .await
            .is_err()
    );
    drop(storage_guard);
    tokio::time::timeout(std::time::Duration::from_secs(5), dropped_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(vault.join("sessions/note/summary.md")).unwrap(),
        "# Edited summary"
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&std::fs::read(vault.join("sessions/note/_meta.json")).unwrap())
            .unwrap();
    assert_eq!(meta["title"], "Edited title");
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn compound_editor_grant_is_fresh_once_before_preparation_and_survives_saved_readback() {
    let (_root, context) = fixture().await;
    let before = context
        .read_document_editor("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let owned = context.clone();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let outcome = context
        .save_document_editor_authorized(
            "note",
            &DocumentTarget::Summary,
            request(&before),
            move || async move {
                assert!(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(20),
                        owned.store.lock_snapshot()
                    )
                    .await
                    .is_ok()
                );
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(owned)
            },
        )
        .await
        .unwrap();
    assert!(matches!(outcome, DocumentEditorSaveOutcome::Saved(_)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn compound_editor_local_unlimited_capacity_preserves_large_title_without_remote_limits() {
    let (root, context) = fixture().await;
    let vault = context.store.vault_base().to_owned();
    drop(context);
    let context = Arc::new(
        VaultContext::new(
            "local".into(),
            Arc::new(SessionStore::with_content_limits(
                vault.clone(),
                ContentLimits::unlimited(),
            )),
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.path().join("runtime")),
    );
    let before = context
        .read_document_editor("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let mut input = request(&before);
    input.title = Some("t".repeat(ContentLimits::default().title_bytes + 1));
    let DocumentEditorSaveOutcome::Saved(after) = context
        .save_document_editor("note", &DocumentTarget::Summary, input)
        .await
        .unwrap()
    else {
        panic!("not saved")
    };
    assert_eq!(after.metadata.meta.title.len(), 4097);
}

#[tokio::test]
async fn post_commit_owned_worker_panic_reports_unknown_and_keeps_complete_canonical_state() {
    let (_root, context) = fixture().await;
    let before = context
        .read_document_editor("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    struct PanicGrant;
    impl Drop for PanicGrant {
        fn drop(&mut self) {
            panic!("post-commit grant-drop fixture");
        }
    }
    let failure = context
        .save_document_editor_authorized(
            "note",
            &DocumentTarget::Summary,
            request(&before),
            || async { Ok(PanicGrant) },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::Internal);
    assert_eq!(failure.mutation_outcome, Some(MutationOutcome::Unknown));
    let after = context
        .read_document_editor("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    assert_eq!(
        after.document.document.unwrap().markdown,
        "# Edited summary"
    );
    assert_eq!(after.metadata.meta.title, "Edited title");
}

#[tokio::test]
async fn queued_membership_writer_and_catalog_first_note_writer_cannot_deadlock_document_save() {
    let (_root, context) = fixture().await;
    let before = context
        .read_document_editor("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let note = context.read_note_editor("note").await.unwrap();
    let catalog = Arc::new(tokio::sync::RwLock::new(()));
    let existing_access = catalog.clone().read_owned().await;
    let mut membership_write = Box::pin(catalog.clone().write_owned());
    std::future::poll_fn(|cx| {
        assert!(membership_write.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    })
    .await;
    let changed = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let membership_changed = changed.clone();
    let membership = tokio::spawn(async move {
        let _grant = membership_write.await;
        membership_changed.store(true, Ordering::SeqCst);
    });
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let document_context = context.clone();
    let document = tokio::spawn(async move {
        document_context
            .save_document_editor_authorized(
                "note",
                &DocumentTarget::Summary,
                request(&before),
                move || async move {
                    let _ = entered_tx.send(());
                    let grant = catalog.read_owned().await;
                    assert!(changed.load(Ordering::SeqCst));
                    Ok(grant)
                },
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), entered_rx)
        .await
        .unwrap()
        .unwrap();
    let note_context = context.clone();
    let legacy_note = tokio::spawn(async move {
        let _grant = existing_access;
        note_context
            .save_note_editor(
                "note",
                hypr_loofah_protocol::SaveNoteEditorRequest {
                    markdown: "Existing catalog-first note write".into(),
                    title: None,
                    tasks: vec![],
                    expected_revision: note.note.revision,
                    expected_tasks_revision: note.tasks.revision,
                },
            )
            .await
    });
    let (document, legacy_note, membership) =
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            tokio::join!(document, legacy_note, membership)
        })
        .await
        .expect("catalog/store lock order deadlocked");
    assert!(matches!(
        document.unwrap().unwrap(),
        DocumentEditorSaveOutcome::Saved(_)
    ));
    assert!(matches!(
        legacy_note.unwrap().unwrap(),
        hypr_loofah_protocol::NoteEditorSaveOutcome::Saved(_)
    ));
    membership.unwrap();
    assert_eq!(
        context.read_note("note").await.unwrap().markdown.as_deref(),
        Some("Existing catalog-first note write")
    );
}

#[tokio::test]
async fn invalid_editor_input_is_rejected_before_catalog_authorization() {
    let (_root, context) = fixture().await;
    let before = context
        .read_document_editor("note", &DocumentTarget::Summary)
        .await
        .unwrap();
    let mut input = request(&before);
    input.expected_metadata_revision.clear();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let failure = context
        .save_document_editor_authorized(
            "note",
            &DocumentTarget::Summary,
            input,
            move || async move {
                observed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::Validation);
    assert_eq!(
        failure.mutation_outcome,
        Some(MutationOutcome::NotCommitted)
    );
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
