use std::sync::Arc;

use hypr_loofah_protocol::{ApplicationError, ErrorCode, SessionSummary, SessionSummaryQuery};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn session_summaries(
        self: &Arc<Self>,
        input: SessionSummaryQuery,
    ) -> Result<Vec<SessionSummary>, ApplicationError> {
        hypr_loofah_protocol::validate_session_summary_query(&input)
            .map_err(|message| error(ErrorCode::Validation, message))?;
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is not ready"))?;
        let context = self.clone();
        let task = match input {
            SessionSummaryQuery::Ids { ids } => tokio::spawn(async move {
                context
                    .store
                    .canonical_session_summaries(context.document_operation_root().as_deref(), &ids)
                    .await
                    .map_err(|cause| storage_error(cause, false))
            }),
            input => tokio::task::spawn_blocking(move || {
                context
                    .store
                    .indexed_session_summaries(&input)
                    .map_err(|cause| storage_error(cause, false))
            }),
        };
        task.await.map_err(|_| {
            error(
                ErrorCode::Unavailable,
                "Session summaries could not be read",
            )
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};

    #[tokio::test]
    async fn cancelled_id_query_retains_vault_lease_until_actual_recovery_read_finishes() {
        let vault = tempfile::tempdir().unwrap();
        let lock_path = vault.path().join(".loofah-vault.lock");
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
        let store = Arc::new(hypr_vault_write::SessionStore::new(vault.path().into()));
        let blocked = store.lock_snapshot().await;
        let mut context = VaultContext::new(
            "test".into(),
            store,
            StartupState::new(Some(vault.path())),
            Arc::new(ConfigState::load_or_default(vault.path())),
            Arc::new(SearchService::default()),
        );
        context.lease = Some(lease.into());
        let context = Arc::new(context);
        let lifetime = Arc::downgrade(&context);
        let caller = context.clone();
        let request = tokio::spawn(async move {
            caller
                .session_summaries(SessionSummaryQuery::Ids {
                    ids: vec!["missing".into()],
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
        drop(blocked);
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                match probe.try_lock() {
                    Ok(()) => break,
                    Err(std::fs::TryLockError::WouldBlock) => tokio::task::yield_now().await,
                    Err(error) => panic!("Cannot inspect vault lease: {error}"),
                }
            }
        })
        .await
        .unwrap();
        assert!(lifetime.upgrade().is_none());
    }
}
