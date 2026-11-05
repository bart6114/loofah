use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use hypr_search_cache::{Cache, SearchRequest, SearchResult};
use hypr_vault_write::IndexEntity;
use hypr_vault_write::index::IndexChangeReceiver;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;

use crate::startup::StartupState;

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error(transparent)]
    Cache(#[from] hypr_search_cache::Error),
    #[error("Search worker failed: {0}")]
    Worker(#[from] tokio::task::JoinError),
    #[error("Index not initialized")]
    NotInitialized,
}

#[derive(Default)]
pub struct SearchService {
    cache: RwLock<Option<Cache>>,
}

impl SearchService {
    pub async fn initialize(&self, global: &Path, vault: &Path) -> Result<(), SearchError> {
        let cache = Cache::new(global, vault)?;
        let worker = cache.clone();
        tokio::task::spawn_blocking(move || worker.initialize(&mut |_| {})).await??;
        *self.cache.write().await = Some(cache);
        Ok(())
    }

    async fn cache(&self) -> Result<Cache, SearchError> {
        self.cache
            .read()
            .await
            .clone()
            .ok_or(SearchError::NotInitialized)
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResult, SearchError> {
        let cache = self.cache().await?;
        Ok(tokio::task::spawn_blocking(move || cache.search(request)).await??)
    }

    pub async fn reconcile(&self, ids: Option<Vec<String>>) -> Result<bool, SearchError> {
        let cache = self.cache().await?;
        let report = tokio::task::spawn_blocking(move || match ids {
            Some(ids) => cache.reconcile_sessions(&ids),
            None => cache.reconcile(&mut |_| {}),
        })
        .await??;
        Ok(report.added + report.updated + report.deleted > 0)
    }

    pub async fn run(
        &self,
        global: &Path,
        vault: &Path,
        startup: &StartupState,
        mut changes: IndexChangeReceiver,
        stop: CancellationToken,
        ready: impl Fn() + Send,
    ) {
        let result = tokio::select! {
            _ = stop.cancelled() => return,
            result = startup.wait_until_ready() => result,
        };
        if let Err(error) = result {
            tracing::warn!(%error, "search cache stopped because vault startup failed");
            return;
        }
        loop {
            match self.initialize(global, vault).await {
                Ok(()) => {
                    ready();
                    break;
                }
                Err(error) => tracing::error!(%error, "failed to initialize search cache"),
            }
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_secs(5)) => {},
            }
        }
        let mut pending = BTreeSet::new();
        loop {
            let full = tokio::select! {
                _ = stop.cancelled() => return,
                change = changes.recv() => match change {
                    Some((entity, ids)) => {
                        if affects_sessions(entity) { pending.extend(ids); }
                        else if entity != IndexEntity::Tags { continue; }
                        false
                    },
                    None => return,
                },
                _ = tokio::time::sleep(Duration::from_secs(5)) => true,
            };
            while let Ok((entity, ids)) = changes.try_recv() {
                if affects_sessions(entity) {
                    pending.extend(ids);
                }
            }
            let ids = (!full).then(|| pending.iter().cloned().collect());
            match self.reconcile(ids).await {
                Ok(changed) => {
                    pending.clear();
                    if changed {
                        ready();
                    }
                }
                Err(error) => {
                    tracing::warn!(%error, "failed to reconcile search cache; retrying in background")
                }
            }
        }
    }
}

fn affects_sessions(entity: IndexEntity) -> bool {
    matches!(
        entity,
        IndexEntity::Sessions | IndexEntity::Docs | IndexEntity::Transcripts
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::startup::StartupPhase;
    use hypr_vault_write::{SessionMeta, SessionStore};
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    #[tokio::test]
    async fn worker_projects_writes_before_announcing_search_ready_and_stops() {
        let vault = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let store = SessionStore::new(vault.path().into());
        let startup = StartupState::new(Some(vault.path()));
        startup.set_phase(StartupPhase::Ready);
        let service = Arc::new(SearchService::default());
        let stop = CancellationToken::new();
        let signals = Arc::new(AtomicUsize::new(0));
        let notify = Arc::new(tokio::sync::Notify::new());
        let changes = store.subscribe_index_changes();
        let task = {
            let service = service.clone();
            let stop = stop.clone();
            let signals = signals.clone();
            let notify = notify.clone();
            let global = cache.path().to_path_buf();
            let vault = vault.path().to_path_buf();
            tokio::spawn(async move {
                service
                    .run(&global, &vault, &startup, changes, stop, move || {
                        signals.fetch_add(1, Ordering::SeqCst);
                        notify.notify_one();
                    })
                    .await;
            })
        };
        tokio::time::timeout(Duration::from_secs(10), notify.notified())
            .await
            .unwrap();
        assert_eq!(signals.load(Ordering::SeqCst), 1);
        let meta: SessionMeta = serde_json::from_value(serde_json::json!({
            "id": "search-test", "title": "Search test", "created_at": "2026-11-05T10:00:00Z", "tags": []
        })).unwrap();
        store.write_meta(&meta).await.unwrap();
        store
            .write_note("search-test", "durable widget pipeline")
            .await
            .unwrap();
        let request = || SearchRequest {
            query: "widget".into(),
            collection: None,
            filters: Default::default(),
            limit: 10,
            options: Default::default(),
        };
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                notify.notified().await;
                if service.search(request()).await.unwrap().count > 0 {
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert!(signals.load(Ordering::SeqCst) >= 2);
        stop.cancel();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    }
}
