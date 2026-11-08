use std::{fs::File, path::PathBuf, sync::Arc};

use hypr_vault_write::{
    SessionStore, StoreError,
    recording_publish::{
        RecordingPublication, RecordingPublicationDiscoveryLimits, RecordingPublicationInventory,
        RecordingPublicationRecoveryPreparation,
    },
};

use super::{gates::PublicationGates, operation_key};

impl crate::VaultContext {
    pub(crate) async fn bootstrap_recording_publications(
        store: Arc<SessionStore>,
        vault: String,
        operations: PathBuf,
        lease: Arc<File>,
    ) -> Result<(Arc<File>, PublicationGates), StoreError> {
        tokio::spawn(async move {
            let gates = PublicationGates::default();
            match store
                .discover_recording_publications(
                    &operations,
                    &vault,
                    RecordingPublicationDiscoveryLimits::default(),
                )
                .await
            {
                Err(_) => gates.block_all(),
                Ok(inventory) => {
                    retain_inventory(&gates, &inventory);
                    for pending in inventory.pending {
                        let request = pending.request().clone();
                        let key = operation_key(&request);
                        let preparation = store
                            .prepare_recording_publication_recovery(&operations, &vault, pending)
                            .await;
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
