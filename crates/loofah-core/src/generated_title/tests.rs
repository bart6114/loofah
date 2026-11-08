use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
use serde_json::json;

async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>) {
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
    let meta = serde_json::from_value(
        json!({"id":"note","title":"","created_at":"2026-11-07T10:00:00Z","tags":[]}),
    )
    .unwrap();
    store.create_note_session(&meta, "Raw note").await.unwrap();
    store.ensure_summary("note").await.unwrap();
    store
        .update_summary("note", "# Summary", Some(""))
        .await
        .unwrap();
    let context = VaultContext::new(
        "vault".into(),
        store,
        StartupState::new(Some(&vault)),
        Arc::new(ConfigState::load_or_default(&vault)),
        Arc::new(SearchService::default()),
    )
    .with_operation_storage(root.path().join("operations"));
    (root, Arc::new(context))
}
async fn baseline(context: &Arc<VaultContext>) -> GeneratedTitleBaseline {
    let GeneratedTitlePreparation::Ready(value) =
        context.prepare_generated_title("note").await.unwrap()
    else {
        panic!("expected blank title");
    };
    value
}

#[tokio::test]
async fn title_core_is_login_free_and_keeps_empty_and_existing_titles_distinct() {
    let (root, context) = fixture().await;
    assert_eq!(
        context
            .apply_generated_title(baseline(&context).await, " <EMPTY> ".into())
            .await
            .unwrap(),
        GeneratedTitleOutcome::Empty
    );
    assert_eq!(
        context
            .store
            .read_meta("note")
            .await
            .unwrap()
            .unwrap()
            .title,
        ""
    );
    assert_eq!(
        context
            .apply_generated_title(baseline(&context).await, " Meeting title ".into())
            .await
            .unwrap(),
        GeneratedTitleOutcome::Applied {
            raw_note: RawNoteTitleOutcome::Applied
        }
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("vault/sessions/note/summary.md")).unwrap(),
        "# Meeting title\n\n# Summary"
    );
    assert!(matches!(
        context.prepare_generated_title("note").await.unwrap(),
        GeneratedTitlePreparation::AlreadyNamed
    ));
}

#[tokio::test]
async fn title_core_baseline_cannot_be_redirected_even_to_a_context_sharing_the_same_store() {
    let (root, context) = fixture().await;
    let prepared = baseline(&context).await;
    let other = Arc::new(
        VaultContext::new(
            "other-context".into(),
            context.store.clone(),
            StartupState::new(Some(&root.path().join("vault"))),
            context.settings.clone(),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.path().join("other-operations")),
    );
    let failure = other
        .apply_generated_title(prepared, "Redirected".into())
        .await
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::Validation);
    assert_eq!(
        failure.mutation_outcome,
        Some(MutationOutcome::NotCommitted)
    );
    assert_eq!(
        context
            .store
            .read_meta("note")
            .await
            .unwrap()
            .unwrap()
            .title,
        ""
    );
    assert!(!root.path().join("other-operations").exists());
}

#[tokio::test]
async fn cancelled_title_prepare_and_apply_retain_context_and_real_vault_lease_until_io_finishes() {
    for apply in [false, true] {
        let (root, context) = fixture().await;
        let mut context = Arc::try_unwrap(context).ok().unwrap();
        let path = root.path().join("lease");
        let lease = std::fs::File::create(&path).unwrap();
        lease.try_lock().unwrap();
        context.lease = Some(lease.into());
        let context = Arc::new(context);
        let prepared = if apply {
            Some(baseline(&context).await)
        } else {
            None
        };
        let weak = Arc::downgrade(&context);
        let competing = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        let held = context.store.lock_snapshot().await;
        let caller = context.clone();
        let task = tokio::spawn(async move {
            if let Some(prepared) = prepared {
                caller
                    .apply_generated_title(prepared, "Generated".into())
                    .await
                    .map(|_| ())
            } else {
                caller.prepare_generated_title("note").await.map(|_| ())
            }
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
        assert!(competing.try_lock().is_err());
        drop(held);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        competing.try_lock().unwrap();
        let meta: serde_json::Value = serde_json::from_slice(
            &std::fs::read(root.path().join("vault/sessions/note/_meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["title"], if apply { "Generated" } else { "" });
    }
}
