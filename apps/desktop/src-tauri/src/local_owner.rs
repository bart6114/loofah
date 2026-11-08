mod lifecycle;
pub(crate) use lifecycle::configure_lifecycle;

use std::sync::Arc;

use hypr_loofah_core::{VaultContext, local_cli::LocalCli};
use hypr_loofah_local_owner::{CoreOwner, EndpointRoot, OwnerServer};
use hypr_storage::vault::VaultLease;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct LocalOwnerState {
    server: Mutex<Option<OwnerServer>>,
    stop: CancellationToken,
    started: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    before_bind: std::sync::Mutex<
        Option<(
            tokio::sync::oneshot::Sender<()>,
            Arc<tokio::sync::Semaphore>,
        )>,
    >,
}

impl LocalOwnerState {
    pub fn initialized(&self) -> bool {
        self.started.load(std::sync::atomic::Ordering::Acquire)
    }

    pub async fn start(
        &self,
        context: Arc<VaultContext>,
        lease: VaultLease,
        endpoints: EndpointRoot,
    ) -> Result<(), String> {
        let mut slot = self.server.lock().await;
        if self.stop.is_cancelled() || slot.is_some() {
            return Ok(());
        }
        context
            .store
            .ensure_ready()
            .map_err(|error| error.to_string())?;
        let dispatcher = Arc::new(
            LocalCli::new(context)
                .map_err(|error| error.to_string())?
                .with_media_host(Arc::new(hypr_loofah_local_media::LocalMediaEngine::new())),
        );
        let handler = Arc::new(CoreOwner::new(dispatcher, endpoints.path().to_owned()));
        self.started
            .store(true, std::sync::atomic::Ordering::Release);
        #[cfg(test)]
        {
            let probe = self.before_bind.lock().unwrap().take();
            if let Some((entered, release)) = probe {
                let _ = entered.send(());
                let _permit = release.acquire().await.map_err(|error| error.to_string())?;
            }
        }
        let server = OwnerServer::bind(lease, &endpoints, endpoints.path(), handler)
            .await
            .map_err(|error| error.to_string())?;
        if self.stop.is_cancelled() {
            server.request_shutdown();
        }
        *slot = Some(server);
        Ok(())
    }

    pub fn request_shutdown(&self) {
        self.stop.cancel();
        if let Ok(slot) = self.server.try_lock()
            && let Some(server) = slot.as_ref()
        {
            server.request_shutdown();
        }
    }

    pub async fn drain(&self, deadline: std::time::Duration) -> bool {
        self.request_shutdown();
        tokio::time::timeout(deadline, async {
            let mut slot = self.server.lock().await;
            if let Some(server) = slot.as_ref() {
                if let Err(error) = server.shutdown().await {
                    tracing::error!(%error, "local owner work could not be confirmed stopped");
                    return false;
                }
            }
            slot.take();
            true
        })
        .await
        .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypr_loofah_core::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_loofah_local_owner::{OwnerClient, OwnerResolution};
    use hypr_loofah_protocol::{LocalCliOperation, LocalCliReply};
    use hypr_storage::vault::LeaseAttempt;

    #[tokio::test]
    async fn managed_context_serves_the_original_store_without_a_second_runtime_or_login() {
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let vault = root.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        let LeaseAttempt::Acquired(lease) = VaultLease::try_acquire_existing(&vault).unwrap()
        else {
            panic!("unowned fixture")
        };
        let store = Arc::new(hypr_vault_write::SessionStore::new(vault.clone()));
        let meta = serde_json::from_value(serde_json::json!({
            "id":"managed", "title":"Original", "created_at":"2026-11-08T10:00:00Z", "tags":[]
        }))
        .unwrap();
        store
            .create_note_session(&meta, "Managed note")
            .await
            .unwrap();
        let context = Arc::new(
            VaultContext::new(
                "local:managed".into(),
                store.clone(),
                StartupState::new(Some(&vault)),
                Arc::new(ConfigState::load_or_default(&vault)),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(root.path().join("operations"))
            .with_vault_lease(lease.clone())
            .unwrap(),
        );
        // A short private root also exercises the production Unix socket length budget.
        let socket_base = std::env::var_os("LOOFAH_OWNER_TEST_ROOT")
            .or_else(|| std::env::var_os("RUNNER_TEMP"))
            .map(std::path::PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let socket_root = tempfile::tempdir_in(socket_base).unwrap();
        let endpoints = EndpointRoot::new(socket_root.path().join("e")).unwrap();
        let state = LocalOwnerState::default();
        state
            .start(context.clone(), lease.clone(), endpoints.clone())
            .await
            .unwrap();
        let OwnerResolution::Connected(client) =
            OwnerClient::resolve(&vault, &endpoints, endpoints.path())
                .await
                .unwrap()
        else {
            panic!("managed owner must retain the original lease")
        };
        let response = client
            .execute(
                uuid::Uuid::new_v4().to_string(),
                LocalCliOperation::Note {
                    session_id: "managed".into(),
                },
                None,
            )
            .await
            .unwrap();
        assert!(
            matches!(response.response.result.unwrap(), LocalCliReply::Note { snapshot } if snapshot.markdown.as_deref() == Some("Managed note"))
        );
        assert!(Arc::ptr_eq(&context.store, &store));
        assert!(state.drain(std::time::Duration::from_secs(5)).await);
        assert!(matches!(
            VaultLease::try_acquire_existing(&vault).unwrap(),
            LeaseAttempt::Busy
        ));
        let (entered, reached) = tokio::sync::oneshot::channel();
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let starting = Arc::new(LocalOwnerState {
            before_bind: std::sync::Mutex::new(Some((entered, release.clone()))),
            ..Default::default()
        });
        let start = {
            let state = starting.clone();
            let context = context.clone();
            let lease = lease.clone();
            let endpoints = endpoints.clone();
            tokio::spawn(async move { state.start(context, lease, endpoints).await })
        };
        reached.await.unwrap();
        assert!(starting.initialized());
        starting.request_shutdown();
        assert!(!starting.drain(std::time::Duration::from_millis(20)).await);
        release.add_permits(1);
        start.await.unwrap().unwrap();
        assert!(starting.drain(std::time::Duration::from_secs(5)).await);
        drop(context);
        drop(store);
        drop(lease);
        assert!(matches!(
            VaultLease::try_acquire_existing(&vault).unwrap(),
            LeaseAttempt::Acquired(_)
        ));
    }

    #[tokio::test]
    async fn uninitialized_remote_shutdown_never_opens_a_local_endpoint() {
        let state = LocalOwnerState::default();
        assert!(!state.initialized());
        assert!(state.drain(std::time::Duration::from_secs(1)).await);
        assert!(!state.initialized());
    }
}
