use super::*;
use crate::{
    local_session_work::LocalSessionWorkRegistry, search::SearchService, settings::ConfigState,
    startup::StartupState,
};
use std::sync::{
    Weak,
    atomic::{AtomicBool, Ordering},
};

async fn fixture() -> (
    tempfile::TempDir,
    Arc<VaultContext>,
    hypr_storage::vault::VaultLease,
) {
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    std::fs::create_dir(&vault).unwrap();
    let hypr_storage::vault::LeaseAttempt::Acquired(lease) =
        hypr_storage::vault::VaultLease::try_acquire_existing(&vault).unwrap()
    else {
        panic!("fixture lease")
    };
    let store = Arc::new(SessionStore::new(vault.clone()));
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Original","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
    store
        .create_note_session(&meta, "Keep notes")
        .await
        .unwrap();
    let context = Arc::new(
        VaultContext::new(
            "local:fixture".into(),
            store,
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_vault_lease(lease.clone())
        .unwrap()
        .with_operation_storage(root.path().join("operations")),
    );
    (root, context, lease)
}

#[tokio::test]
async fn standalone_original_lease_moves_stale_channels_and_opaque_files_without_enabling_server_bypass()
 {
    let (_root, context, lease) = fixture().await;
    let directory = context.store.vault_base().join("sessions/note");
    for (name, bytes) in [
        ("audio_mic.wav", b"retained mic".as_slice()),
        ("audio_spk.wav", b"retained system"),
        ("unknown.bin", b"user attachment"),
        (".private", b"opaque private"),
    ] {
        std::fs::write(directory.join(name), bytes).unwrap();
    }
    assert!(context.read_local_session_lifecycle("note").await.is_err());
    context
        .enable_standalone_local_session_lifecycle(&lease)
        .unwrap();
    assert!(context.read_session_lifecycle("note").await.is_err());
    let before = context.read_local_session_lifecycle("note").await.unwrap();
    let request = uuid::Uuid::new_v4().to_string();
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: before.revision,
    };
    assert!(matches!(
        context
            .delete_local_session("cli".into(), request.clone(), input.clone())
            .await
            .unwrap(),
        DeleteSessionOutcome::Trashed { .. }
    ));
    let details = context
        .local_session_deletion_details("cli".into(), request, input)
        .await
        .unwrap()
        .unwrap();
    assert!(!directory.exists());
    for (name, bytes) in [
        ("audio_mic.wav", b"retained mic".as_slice()),
        ("audio_spk.wav", b"retained system"),
        ("unknown.bin", b"user attachment"),
        (".private", b"opaque private"),
    ] {
        assert_eq!(std::fs::read(details.trash_path.join(name)).unwrap(), bytes);
    }
    assert_eq!(
        std::fs::read_to_string(details.trash_path.join("notes.md")).unwrap(),
        "Keep notes"
    );
}

struct ManagedHost {
    context: Weak<VaultContext>,
    known_idle: AtomicBool,
    physical: Arc<tokio::sync::RwLock<()>>,
}
impl LocalSessionLifecycleHost for ManagedHost {
    fn admit<'a>(
        &'a self,
        context: &'a Arc<VaultContext>,
        _: &'a str,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Box<dyn Send + Sync>, ApplicationError>> + Send + 'a>,
    > {
        Box::pin(async move {
            if !self
                .context
                .upgrade()
                .is_some_and(|original| Arc::ptr_eq(&original, context))
                || !self.known_idle.load(Ordering::Acquire)
            {
                return Err(crate::session_lifecycle_gate::session_lifecycle_busy());
            }
            let guard = self
                .physical
                .clone()
                .try_write_owned()
                .map_err(|_| crate::session_lifecycle_gate::session_lifecycle_busy())?;
            Ok(Box::new(guard) as Box<dyn Send + Sync>)
        })
    }
}

#[tokio::test]
async fn managed_local_proof_requires_known_physical_idle_and_both_original_generation_barriers() {
    let (_root, context, _lease) = fixture().await;
    let directory = context.store.vault_base().join("sessions/note");
    std::fs::write(directory.join("audio_mic.wav"), b"raw mic").unwrap();
    let host = Arc::new(ManagedHost {
        context: Arc::downgrade(&context),
        known_idle: false.into(),
        physical: Arc::new(tokio::sync::RwLock::new(())),
    });
    context
        .configure_local_session_lifecycle_host(host.clone())
        .unwrap();
    assert!(context.read_local_session_lifecycle("note").await.is_err());
    host.known_idle.store(true, Ordering::Release);
    let physical = host.physical.clone().read_owned().await;
    assert!(context.read_local_session_lifecycle("note").await.is_err());
    drop(physical);
    let original = context.read_local_session_lifecycle("note").await.unwrap();
    let registry = LocalSessionWorkRegistry::new(context.clone());
    let first = uuid::Uuid::new_v4().to_string();
    let work = registry.begin("note".into(), first.clone()).await.unwrap();
    assert!(context.read_local_session_lifecycle("note").await.is_err());
    work.producer_finished().unwrap();
    assert!(context.read_local_session_lifecycle("note").await.is_err());
    context
        .store
        .write_note("note", "Persisted producer output")
        .await
        .unwrap();
    registry.persistence_finished("note", &first).unwrap();
    assert!(context.read_local_session_lifecycle("note").await.is_ok());
    let second = uuid::Uuid::new_v4().to_string();
    let current = registry.begin("note".into(), second.clone()).await.unwrap();
    work.producer_finished().unwrap();
    registry.persistence_finished("note", &first).unwrap();
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: original.revision,
    };
    assert!(
        context
            .delete_local_session(
                "cli".into(),
                uuid::Uuid::new_v4().to_string(),
                input.clone()
            )
            .await
            .is_err()
    );
    current.producer_finished().unwrap();
    registry.persistence_finished("note", &second).unwrap();
    assert!(matches!(
        context
            .delete_local_session("cli".into(), uuid::Uuid::new_v4().to_string(), input)
            .await
            .unwrap(),
        DeleteSessionOutcome::Conflict { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(directory.join("notes.md")).unwrap(),
        "Persisted producer output"
    );
    let unknown = registry
        .begin("note".into(), uuid::Uuid::new_v4().to_string())
        .await
        .unwrap();
    drop(unknown);
    assert!(context.read_local_session_lifecycle("note").await.is_err());
}
