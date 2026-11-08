use std::sync::Arc;

use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, TimelineFacets, TimelineFacetsQuery, TimelinePage, TimelineQuery,
};
use hypr_vault_write::StoreError;

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

fn projection_error(cause: StoreError) -> ApplicationError {
    if matches!(cause, StoreError::Conflict(_)) {
        let mut failure = error(ErrorCode::Conflict, "Timeline changed; restart pagination");
        failure.retryable = true;
        failure
    } else {
        storage_error(cause, false)
    }
}

impl VaultContext {
    pub async fn timeline_headers(
        self: &Arc<Self>,
        input: TimelineQuery,
    ) -> Result<TimelinePage, ApplicationError> {
        hypr_loofah_protocol::validate_timeline_query(&input)
            .map_err(|message| error(ErrorCode::Validation, message))?;
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is not ready"))?;
        let context = self.clone();
        tokio::task::spawn_blocking(move || {
            context
                .store
                .indexed_timeline_headers(&input)
                .map_err(projection_error)
        })
        .await
        .map_err(|_| error(ErrorCode::Unavailable, "Timeline could not be read"))?
    }

    pub async fn timeline_facets(
        self: &Arc<Self>,
        input: TimelineFacetsQuery,
    ) -> Result<TimelineFacets, ApplicationError> {
        hypr_loofah_protocol::validate_timeline_facets_query(&input)
            .map_err(|message| error(ErrorCode::Validation, message))?;
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is not ready"))?;
        let context = self.clone();
        tokio::task::spawn_blocking(move || {
            context
                .store
                .indexed_timeline_facets(&input)
                .map_err(projection_error)
        })
        .await
        .map_err(|_| error(ErrorCode::Unavailable, "Timeline facets could not be read"))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};

    #[test]
    fn abandoned_timeline_read_keeps_context_lease_until_queued_projection_really_finishes() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let vault = tempfile::tempdir().unwrap();
            let lock = vault.path().join(".loofah-vault.lock");
            let lease = std::fs::File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&lock)
                .unwrap();
            lease.try_lock().unwrap();
            let probe = std::fs::File::options()
                .read(true)
                .write(true)
                .open(&lock)
                .unwrap();
            let store = Arc::new(hypr_vault_write::SessionStore::new(vault.path().into()));
            let mut context = VaultContext::new(
                "test".into(),
                store,
                StartupState::new(Some(vault.path())),
                Arc::new(ConfigState::load_or_default(vault.path())),
                Arc::new(SearchService::default()),
            );
            context.lease = Some(lease);
            let context = Arc::new(context);
            let lifetime = Arc::downgrade(&context);
            let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
            let (release_tx, release_rx) = std::sync::mpsc::channel();
            let blocker = tokio::task::spawn_blocking(move || {
                ready_tx.send(()).unwrap();
                release_rx.recv().unwrap();
            });
            ready_rx.await.unwrap();
            let caller = context.clone();
            let request = tokio::spawn(async move {
                caller
                    .timeline_headers(TimelineQuery {
                        view: hypr_loofah_protocol::TimelineView::Untagged {},
                        limit: 200,
                        cursor: None,
                        expected_revision: None,
                    })
                    .await
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
            release_tx.send(()).unwrap();
            blocker.await.unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(10), async {
                loop {
                    match probe.try_lock() {
                        Ok(()) => break,
                        Err(std::fs::TryLockError::WouldBlock) => tokio::task::yield_now().await,
                        Err(cause) => panic!("{cause}"),
                    }
                }
            })
            .await
            .unwrap();
            assert!(lifetime.upgrade().is_none());
        });
    }

    #[test]
    fn stale_projection_conflicts_are_retryable_and_reads_have_no_mutation_outcome() {
        let error = projection_error(StoreError::Conflict("private cursor detail".into()));
        assert_eq!(error.code, ErrorCode::Conflict);
        assert!(error.retryable);
        assert!(error.mutation_outcome.is_none());
        assert!(!error.message.contains("private"));
    }
}
