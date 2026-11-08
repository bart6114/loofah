use std::{fs::File, path::PathBuf, sync::Arc};

use hypr_vault_write::{
    SessionStore, StoreError,
    recording_publish::{
        RecordingPublication, RecordingPublicationDiscoveryLimits, RecordingPublicationInventory,
        RecordingPublicationRecovery, RecordingPublicationRecoveryPreparation,
    },
};

use super::{gates::PublicationGates, operation_key};

impl crate::VaultContext {
    /// Native startup reuses its managed Context; recovery must finish before readiness.
    pub async fn recover_recording_publications(
        self: &Arc<Self>,
    ) -> Result<(), hypr_loofah_protocol::ApplicationError> {
        use crate::vault::{error, storage_error};
        use hypr_loofah_protocol::ErrorCode;
        let context = self.clone();
        tokio::spawn(async move {
            let _pin = context.lease_pin().ok_or_else(|| {
                error(
                    ErrorCode::Unavailable,
                    "Vault lease required for recording recovery",
                )
            })?;
            if !context.recording_publications.initialized() && context.store.ensure_ready().is_ok()
            {
                return Err(error(
                    ErrorCode::Unavailable,
                    "Recording recovery must run before vault readiness",
                ));
            }
            let operations = context
                .operations
                .as_ref()
                .ok_or_else(|| {
                    error(
                        ErrorCode::Unavailable,
                        "Operation storage is not configured",
                    )
                })?
                .root
                .join("recording-publications");
            let recovery = context
                .store
                .recording_publication_startup_recovery(&_pin)
                .map_err(|cause| storage_error(cause, false))?;
            recover(
                &recovery,
                &context.events.vault_id(),
                &operations,
                &context.recording_publications,
            )
            .await;
            Ok(())
        })
        .await
        .map_err(|cause| {
            storage_error(
                StoreError::Io(format!("Recording recovery worker stopped: {cause}")),
                true,
            )
        })?
    }

    pub(crate) async fn bootstrap_recording_publications(
        store: Arc<SessionStore>,
        vault: String,
        operations: PathBuf,
        lease: Arc<File>,
    ) -> Result<(Arc<File>, PublicationGates), StoreError> {
        tokio::spawn(async move {
            let gates = PublicationGates::default();
            let recovery = store.recording_publication_recovery()?;
            recover(&recovery, &vault, &operations, &gates).await;
            Ok((lease, gates))
        })
        .await
        .map_err(|cause| StoreError::Io(format!("Recording recovery worker stopped: {cause}")))?
    }
}

pub(super) fn retain_inventory(
    gates: &PublicationGates,
    inventory: &RecordingPublicationInventory,
) {
    for pending in &inventory.pending {
        let request = pending.request();
        gates.retain(&request.session_id, &operation_key(request));
    }
    for issue in &inventory.issues {
        if let Some(session) = &issue.session_id {
            gates.review(session);
        } else {
            gates.block_all();
        }
    }
}

async fn recover(
    store: &RecordingPublicationRecovery,
    vault: &str,
    operations: &std::path::Path,
    gates: &PublicationGates,
) {
    let _initialization = gates.bootstrap_guard().await;
    if gates.initialized() {
        return;
    }
    match store
        .discover(
            operations,
            vault,
            RecordingPublicationDiscoveryLimits::default(),
        )
        .await
    {
        Err(_) => gates.block_all(),
        Ok(inventory) => {
            retain_inventory(gates, &inventory);
            for pending in inventory.pending {
                let request = pending.request().clone();
                let key = operation_key(&request);
                let preparation = store.prepare(operations, vault, pending).await;
                match preparation {
                    Ok(RecordingPublicationRecoveryPreparation::NoLongerPending) => {
                        gates.resolve(&request.session_id, &key);
                    }
                    Ok(RecordingPublicationRecoveryPreparation::Prepared(plan)) => {
                        // This only completes an already-admitted filesystem intent;
                        // access to the resulting recording is authorized separately.
                        if let Ok(plan) = (*plan).recheck().await
                            && matches!(
                                plan.commit().await,
                                Ok(RecordingPublication::Published { .. })
                            )
                        {
                            gates.resolve(&request.session_id, &key);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    gates.finish_bootstrap();
}

#[cfg(test)]
mod native_tests {
    use super::*;
    use crate::{
        VaultContext, search::SearchService, settings::ConfigState, startup::StartupState,
    };
    use hypr_storage::vault::{LeaseAttempt, VaultLease};

    async fn fixture() -> (tempfile::TempDir, Arc<VaultContext>, std::sync::Weak<File>) {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let vault = root.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        let lease = match VaultLease::try_acquire_existing(&vault).unwrap() {
            LeaseAttempt::Acquired(lease) => lease,
            _ => panic!("unexpected owner"),
        };
        let original = Arc::downgrade(&lease.pin());
        let store = Arc::new(SessionStore::new(lease.canonical_vault().into()));
        store.pin_vault_lease(&lease).unwrap();
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Saved note","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Preserve the user's note")
            .await
            .unwrap();
        let mut wav = hound::WavWriter::create(
            vault.join("sessions/note/audio.wav"),
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        wav.write_sample(0.0f32).unwrap();
        wav.finalize().unwrap();
        store.set_startup_pending(true);
        let context = VaultContext::new(
            "vault".into(),
            store,
            StartupState::new(Some(&vault)),
            Arc::new(ConfigState::load_with_vault_lease(&lease)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(root.path().join("operations"))
        .with_vault_lease(lease)
        .unwrap();
        (root, Arc::new(context), original)
    }

    #[tokio::test]
    async fn managed_native_context_recovers_its_existing_gates_once_before_readiness() {
        let (root, context, _) = fixture().await;
        let existing = context.recording_publications.clone();
        let (first, second) = tokio::join!(
            context.recover_recording_publications(),
            context.recover_recording_publications()
        );
        first.unwrap();
        second.unwrap();
        assert!(existing.initialized());
        assert!(context.recording_publications.initialized());
        assert!(
            !root
                .path()
                .join("operations/recording-publications")
                .exists()
        );
        context.store.set_startup_pending(false);
        assert!(context.audio_metadata("note").await.unwrap().is_some());
        context.recover_recording_publications().await.unwrap();
    }

    #[tokio::test]
    async fn failed_discovery_blocks_recordings_and_keeps_notes_and_unknown_files_intact() {
        let (root, context, _) = fixture().await;
        let operations = root.path().join("operations/recording-publications");
        std::fs::create_dir_all(&operations).unwrap();
        std::fs::write(operations.join("user-file"), b"preserve unknown state").unwrap();
        std::fs::write(
            operations.join(format!("recording-publication-{}.json", "a".repeat(64))),
            b"not a receipt",
        )
        .unwrap();
        context.recover_recording_publications().await.unwrap();
        context.store.set_startup_pending(false);
        assert!(context.audio_metadata("note").await.is_err());
        assert_eq!(
            context.read_note("note").await.unwrap().markdown.as_deref(),
            Some("Preserve the user's note")
        );
        assert_eq!(
            std::fs::read(operations.join("user-file")).unwrap(),
            b"preserve unknown state"
        );
    }

    #[tokio::test]
    async fn cancelled_native_startup_waiter_keeps_context_original_lease_and_initialization_owner()
    {
        let (root, context, original) = fixture().await;
        let gate = context.recording_publications.bootstrap_guard().await;
        let weak = Arc::downgrade(&context);
        let caller = context.clone();
        let waiter = tokio::spawn(async move { caller.recover_recording_publications().await });
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while weak.strong_count() < 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        waiter.abort();
        let _ = waiter.await;
        drop(context);
        assert!(weak.upgrade().is_some());
        assert!(original.upgrade().is_some());
        assert!(matches!(
            VaultLease::try_acquire_existing(&root.path().join("vault")).unwrap(),
            LeaseAttempt::Busy
        ));
        drop(gate);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while weak.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(original.upgrade().is_none());
    }

    #[tokio::test]
    async fn uninitialized_context_cannot_start_recording_recovery_after_readiness() {
        let (_root, context, _) = fixture().await;
        context.store.set_startup_pending(false);
        assert!(context.recover_recording_publications().await.is_err());
        assert!(!context.recording_publications.initialized());
    }
}
