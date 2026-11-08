use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use serde_json::json;

async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>) {
    let root = tempfile::tempdir().unwrap();
    let context = Arc::new(VaultContext::new(
        "test".into(),
        Arc::new(hypr_vault_write::SessionStore::new(root.path().into())),
        StartupState::new(Some(root.path())),
        Arc::new(ConfigState::load_or_default(root.path())),
        Arc::new(SearchService::default()),
    ));
    let meta = serde_json::from_value(json!({"id":"note","title":"Keep title","created_at":"2026-11-08T00:00:00Z","tags":["in-use"],"tag_suggestions":{"items":["import/audio","other"],"dismissed":[]},"future":{"keep":true}})).unwrap();
    context
        .store
        .create_note_session(&meta, "Keep note")
        .await
        .unwrap();
    (root, context)
}

#[tokio::test]
async fn complete_tag_operations_reuse_checked_storage_without_touching_other_content() {
    let (root, context) = fixture().await;
    std::fs::write(
        root.path().join("tags.json"),
        br#"{"future":true,"tags":[]}"#,
    )
    .unwrap();
    let tag = context
        .ensure_tag(EnsureTagRequest {
            name: " #IMPORT/manual ".into(),
        })
        .await
        .unwrap();
    assert_eq!(tag.id, "import/manual");
    assert_eq!(
        context
            .tag_suggestions(TagSuggestionsRequest {
                query: "".into(),
                excluded: vec![],
                limit: 8
            })
            .await
            .unwrap(),
        ["import/manual", "in-use"]
    );
    let before = context.read_metadata("note").await.unwrap();
    let action = TagSuggestionActionRequest {
        name: " #IMPORT/audio ".into(),
        expected_revision: before.revision.clone(),
    };
    let MetadataSaveOutcome::Saved(saved) = context
        .accept_tag_suggestion("note", action.clone())
        .await
        .unwrap()
    else {
        panic!("Not saved")
    };
    assert_eq!(saved.meta.tags, ["import/audio", "in-use"]);
    assert_eq!(saved.meta.extra, before.meta.extra);
    assert!(
        matches!(context.dismiss_tag_suggestion("note", TagSuggestionActionRequest {name:"other".into(),..action}).await.unwrap(), MetadataSaveOutcome::Conflict(current) if current == saved)
    );
    let MetadataSaveOutcome::Saved(dismissed) = context
        .dismiss_tag_suggestion(
            "note",
            TagSuggestionActionRequest {
                name: "other".into(),
                expected_revision: saved.revision,
            },
        )
        .await
        .unwrap()
    else {
        panic!("Not saved")
    };
    assert_eq!(dismissed.meta.tag_suggestions.unwrap().dismissed, ["other"]);
    let registry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("tags.json")).unwrap()).unwrap();
    assert_eq!(registry["future"], true);
    assert_eq!(
        std::fs::read_to_string(root.path().join("sessions/note/notes.md")).unwrap(),
        "Keep note"
    );
}

#[tokio::test]
async fn tag_validation_readiness_and_missing_sessions_are_not_committed() {
    let (root, context) = fixture().await;
    let action = TagSuggestionActionRequest {
        name: "other".into(),
        expected_revision: "revision".into(),
    };
    for (id, input, code) in [
        ("../escape", action.clone(), ErrorCode::Validation),
        ("missing", action.clone(), ErrorCode::NotFound),
        (
            "note",
            TagSuggestionActionRequest {
                name: " ".into(),
                ..action.clone()
            },
            ErrorCode::Validation,
        ),
        (
            "note",
            TagSuggestionActionRequest {
                expected_revision: "".into(),
                ..action.clone()
            },
            ErrorCode::Validation,
        ),
        (
            "note",
            TagSuggestionActionRequest {
                name: "x".repeat(4097),
                ..action.clone()
            },
            ErrorCode::ResourceLimit,
        ),
    ] {
        let failure = context.accept_tag_suggestion(id, input).await.unwrap_err();
        assert_eq!(failure.code, code);
        assert_eq!(
            failure.mutation_outcome,
            Some(MutationOutcome::NotCommitted)
        );
    }
    assert!(!root.path().join("tags.json").exists());
    context.store.set_startup_pending(true);
    assert_eq!(
        context
            .tag_suggestions(TagSuggestionsRequest {
                query: "".into(),
                excluded: vec![],
                limit: 8
            })
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    let failure = context
        .ensure_tag(EnsureTagRequest { name: "new".into() })
        .await
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::Unavailable);
    assert_eq!(
        failure.mutation_outcome,
        Some(MutationOutcome::NotCommitted)
    );
    assert!(!root.path().join("tags.json").exists());
}

#[tokio::test]
async fn cancelled_tag_writes_keep_the_context_and_lease_until_the_owned_storage_task_finishes() {
    for accept in [false, true] {
        let (root, mut context) = fixture().await;
        let before = context.read_metadata("note").await.unwrap();
        let lock_path = root.path().join(".test-lease");
        let lease = std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        lease.try_lock().unwrap();
        let probe = std::fs::File::options()
            .read(true)
            .write(true)
            .open(&lock_path)
            .unwrap();
        Arc::get_mut(&mut context).unwrap().lease = Some(lease.into());
        let blocked = context.store.lock_snapshot().await;
        let lifetime = Arc::downgrade(&context);
        let caller = context.clone();
        let request = tokio::spawn(async move {
            if accept {
                caller
                    .accept_tag_suggestion(
                        "note",
                        TagSuggestionActionRequest {
                            name: "import/audio".into(),
                            expected_revision: before.revision,
                        },
                    )
                    .await
                    .map(|_| ())
            } else {
                caller
                    .ensure_tag(EnsureTagRequest {
                        name: "registered".into(),
                    })
                    .await
                    .map(|_| ())
            }
        });
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while lifetime.strong_count() < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        request.abort();
        assert!(request.await.unwrap_err().is_cancelled());
        drop(context);
        assert!(matches!(
            probe.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(blocked);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match probe.try_lock() {
                    Ok(()) => break,
                    Err(std::fs::TryLockError::WouldBlock) => tokio::task::yield_now().await,
                    Err(error) => panic!("{error}"),
                }
            }
        })
        .await
        .unwrap();
        assert!(lifetime.upgrade().is_none());
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.path().join("tags.json")).unwrap()).unwrap();
        assert_eq!(
            value["tags"][0]["id"],
            if accept { "import/audio" } else { "registered" }
        );
    }
}
