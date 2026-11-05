use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use hypr_loofah_protocol::Change;
use hypr_vault_write::{SessionStore, StoreError};
use tokio_util::sync::CancellationToken;

use crate::{
    VaultContext,
    search::SearchService,
    settings::ConfigState,
    startup::{self, StartupPhase, StartupState},
};

pub struct VaultRuntime {
    pub context: Arc<VaultContext>,
    stop: CancellationToken,
    workers: Vec<tokio::task::JoinHandle<()>>,
}

impl VaultRuntime {
    pub async fn open(
        id: String,
        vault: &Path,
        cache_root: &Path,
        operation_root: &Path,
    ) -> Result<Self, StoreError> {
        let vault = vault.to_path_buf();
        let (vault, lease) = tokio::task::spawn_blocking(move || {
            std::fs::create_dir_all(&vault).map_err(io_error)?;
            let vault = std::fs::canonicalize(&vault).map_err(io_error)?;
            let lock_path = vault.join(".loofah-vault.lock");
            if std::fs::symlink_metadata(&lock_path).is_ok_and(|meta| meta.file_type().is_symlink())
            {
                return Err(StoreError::Io(
                    "vault lock must not be a symbolic link".into(),
                ));
            }
            let lease = std::fs::File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(lock_path)
                .map_err(io_error)?;
            lease.try_lock().map_err(|_| {
                StoreError::Conflict("vault is already open or cannot be locked".into())
            })?;
            Ok((vault, lease))
        })
        .await
        .map_err(|error| StoreError::Io(error.to_string()))??;
        let store = Arc::new(SessionStore::new(vault.clone()));
        store.set_startup_pending(true);
        let startup = StartupState::new(Some(&vault));
        startup::prepare(store.clone(), startup.clone())
            .await
            .map_err(|error| StoreError::Io(error.to_string()))?;
        let mut context = VaultContext::new(
            id,
            store.clone(),
            startup.clone(),
            Arc::new(ConfigState::load_or_default(&vault)),
            Arc::new(SearchService::default()),
        )
        .with_operation_storage(operation_root.to_path_buf());
        context.lease = Some(lease);
        let context = Arc::new(context);
        let stop = CancellationToken::new();
        let changes = store
            .take_index_change_receiver()
            .expect("new store owns its change receiver");
        let events = context.events.clone();
        let events_stop = stop.clone();
        let events_worker = tokio::spawn(async move {
            tokio::select! {
                _ = events_stop.cancelled() => {},
                _ = hypr_vault_write::index::run_index_change_dispatcher(changes, move |event| {
                    events.publish(Change::IndexChanged { entity: event.entity, ids: event.ids });
                }) => {},
            }
        });
        let projection_changes = store.subscribe_index_changes();
        let service = context.search.clone();
        let search_events = context.events.clone();
        let search_stop = stop.clone();
        let cache_root = cache_root.to_path_buf();
        store.set_startup_pending(false);
        startup.set_phase(StartupPhase::Ready);
        let search_worker = tokio::spawn(async move {
            service
                .run(
                    &cache_root,
                    &vault,
                    &startup,
                    projection_changes,
                    search_stop,
                    move || {
                        search_events.publish(Change::SearchReady);
                    },
                )
                .await;
        });
        Ok(Self {
            context,
            stop,
            workers: vec![events_worker, search_worker],
        })
    }

    pub async fn shutdown(mut self) -> Result<(), StoreError> {
        self.context.store.set_startup_pending(true);
        self.context.startup.set_phase(StartupPhase::Stopping);
        self.stop.cancel();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        let flush = tokio::time::timeout_at(deadline, self.context.store.flush_all())
            .await
            .map_err(|_| StoreError::Io("vault flush exceeded shutdown deadline".into()))?;
        flush?;
        for worker in &mut self.workers {
            tokio::time::timeout_at(deadline, worker)
                .await
                .map_err(|_| {
                    StoreError::Io("vault workers did not stop before shutdown deadline".into())
                })?
                .map_err(|error| StoreError::Io(format!("vault worker failed: {error}")))?;
        }
        self.context.startup.set_phase(StartupPhase::Stopped);
        Ok(())
    }
}

impl Drop for VaultRuntime {
    fn drop(&mut self) {
        self.stop.cancel();
        for worker in &self.workers {
            worker.abort();
        }
    }
}

fn io_error(error: std::io::Error) -> StoreError {
    StoreError::Io(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypr_loofah_protocol::{NoteSaveOutcome, SaveNoteRequest};

    #[tokio::test]
    async fn headless_lifecycle_exclusively_owns_vault_and_reopens_canonical_notes() {
        let vault = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let runtime = VaultRuntime::open(
            "private".into(),
            vault.path(),
            cache.path(),
            &cache.path().join("operations"),
        )
        .await
        .unwrap();
        assert!(runtime.context.startup.is_ready());
        assert!(matches!(
            VaultRuntime::open(
                "other".into(),
                vault.path(),
                cache.path(),
                &cache.path().join("operations")
            )
            .await,
            Err(StoreError::Conflict(_))
        ));
        let meta = serde_json::from_value(serde_json::json!({
            "id": "note", "title": "Original", "created_at": "2026-11-05T10:00:00Z", "tags": []
        }))
        .unwrap();
        runtime.context.store.write_meta(&meta).await.unwrap();
        let revision = runtime.context.read_note("note").await.unwrap().revision;
        let saved = runtime
            .context
            .save_note(
                "note",
                SaveNoteRequest {
                    markdown: "restart-safe".into(),
                    title: None,
                    expected_revision: revision,
                },
            )
            .await
            .unwrap();
        let NoteSaveOutcome::Saved(saved) = saved else {
            panic!("initial save conflicted");
        };
        let remaining_client = runtime.context.clone();
        runtime.shutdown().await.unwrap();
        assert_eq!(
            remaining_client.read_note("note").await.unwrap_err().code,
            hypr_loofah_protocol::ErrorCode::Unavailable
        );
        assert!(matches!(
            VaultRuntime::open(
                "other".into(),
                vault.path(),
                cache.path(),
                &cache.path().join("operations")
            )
            .await,
            Err(StoreError::Conflict(_))
        ));
        drop(remaining_client);
        let reopened = VaultRuntime::open(
            "private".into(),
            vault.path(),
            cache.path(),
            &cache.path().join("operations"),
        )
        .await
        .unwrap();
        assert_eq!(reopened.context.read_note("note").await.unwrap(), saved);
        reopened.shutdown().await.unwrap();
    }
}
