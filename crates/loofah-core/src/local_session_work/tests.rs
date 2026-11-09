use super::*;
use crate::{search::SearchService, settings::ConfigState, startup::StartupState};

async fn fixture() -> (
    tempfile::TempDir,
    Arc<VaultContext>,
    LocalSessionWorkRegistry,
) {
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Note","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
    store.create_note_session(&meta, "Body").await.unwrap();
    let context = Arc::new(
        VaultContext::new(
            "vault".into(),
            store,
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.path().join("operations")),
    );
    let registry = LocalSessionWorkRegistry::new(context.clone());
    (root, context, registry)
}

#[tokio::test]
async fn native_completion_and_durable_persistence_are_independent_barriers() {
    for native_first in [true, false] {
        let (_root, context, registry) = fixture().await;
        let id = uuid::Uuid::new_v4().to_string();
        let work = registry.begin("note".into(), id.clone()).await.unwrap();
        assert!(context.try_session_lifecycle_change("note").is_err());
        if native_first {
            work.producer_finished().unwrap();
        } else {
            registry.persistence_finished("note", &id).unwrap();
        }
        assert!(context.try_session_lifecycle_change("note").is_err());
        if native_first {
            registry.persistence_finished("note", &id).unwrap();
        } else {
            work.producer_finished().unwrap();
        }
        assert!(context.try_session_lifecycle_change("note").is_ok());
    }
}

#[tokio::test]
async fn late_acknowledgements_cannot_release_another_generation_and_wrong_sessions_are_rejected() {
    let (_root, context, registry) = fixture().await;
    let first = uuid::Uuid::new_v4().to_string();
    let second = uuid::Uuid::new_v4().to_string();
    let old = registry.begin("note".into(), first.clone()).await.unwrap();
    old.producer_finished().unwrap();
    registry.persistence_finished("note", &first).unwrap();
    assert!(registry.begin("note".into(), first.clone()).await.is_err());
    let current = registry.begin("note".into(), second.clone()).await.unwrap();
    current.producer_finished().unwrap();
    old.producer_finished().unwrap();
    registry.persistence_finished("note", &first).unwrap();
    assert!(registry.persistence_finished("other", &second).is_err());
    assert!(context.try_session_lifecycle_change("note").is_err());
    registry.persistence_finished("note", &second).unwrap();
    assert!(context.try_session_lifecycle_change("note").is_ok());
}

#[tokio::test]
async fn dropping_callers_and_duplicate_starts_never_release_unconfirmed_work() {
    let (_root, context, registry) = fixture().await;
    let id = uuid::Uuid::new_v4().to_string();
    let work = registry.begin("note".into(), id.clone()).await.unwrap();
    assert_eq!(
        registry
            .begin("note".into(), id.clone())
            .await
            .err()
            .unwrap()
            .mutation_outcome,
        Some(MutationOutcome::Unknown)
    );
    drop(work);
    registry.persistence_finished("note", &id).unwrap();
    assert!(context.try_session_lifecycle_change("note").is_err());
}

#[tokio::test]
async fn definitive_rejection_releases_only_its_own_admission() {
    let (_root, context, registry) = fixture().await;
    let first = registry
        .begin("note".into(), uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    let rejected = registry
        .begin("note".into(), uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    rejected.not_started().unwrap();
    assert!(context.try_session_lifecycle_change("note").is_err());
    first.not_started().unwrap();
    assert!(context.try_session_lifecycle_change("note").is_ok());
    assert!(
        registry
            .begin("note".into(), "not-a-request".into())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn weak_lifecycle_observer_rejects_foreign_missing_and_unconfirmed_registry() {
    let (_root, context, registry) = fixture().await;
    let observer = registry.downgrade();
    assert!(observer.ensure_idle_for(&context, "note").is_ok());
    let (_other_root, other, _registry) = fixture().await;
    assert!(observer.ensure_idle_for(&other, "note").is_err());
    let request = uuid::Uuid::new_v4().to_string();
    let work = registry
        .begin("note".into(), request.clone())
        .await
        .unwrap();
    assert!(observer.ensure_idle_for(&context, "note").is_err());
    work.producer_finished().unwrap();
    assert!(observer.ensure_idle_for(&context, "note").is_err());
    registry.persistence_finished("note", &request).unwrap();
    assert!(observer.ensure_idle_for(&context, "note").is_ok());
    drop(work);
    drop(registry);
    assert!(observer.ensure_idle_for(&context, "note").is_err());
}

#[tokio::test]
async fn weak_lifecycle_observer_rejects_poisoned_inventory() {
    let (_root, context, registry) = fixture().await;
    let observer = registry.downgrade();
    let other = registry.clone();
    let _ = std::thread::spawn(move || {
        let _state = other.0.state.lock().unwrap();
        panic!("unconfirmed producer state");
    })
    .join();
    assert!(observer.ensure_idle_for(&context, "note").is_err());
}

#[tokio::test]
async fn unknown_and_foreign_completed_acknowledgements_are_not_success() {
    let (_root, context, registry) = fixture().await;
    let id = uuid::Uuid::new_v4().to_string();
    assert!(registry.persistence_finished("note", &id).is_err());
    let work = registry.begin("note".into(), id.clone()).await.unwrap();
    work.producer_finished().unwrap();
    registry.persistence_finished("note", &id).unwrap();
    registry.persistence_finished("note", &id).unwrap();
    assert!(registry.persistence_finished("other", &id).is_err());
    assert!(context.try_session_lifecycle_change("note").is_ok());
}
