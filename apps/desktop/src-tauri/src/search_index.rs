use crate::session_store::{IndexEntity, SessionStore};
use std::sync::{Arc, Mutex};
use tauri::Manager;

pub async fn open(app: &tauri::AppHandle, store: &SessionStore) -> Result<bool, String> {
    let vault = store.vault_base().to_path_buf();
    let cache =
        tokio::task::spawn_blocking(move || hypr_search_cache::Cache::open(&vault, "desktop"))
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
    let ready = cache.bootstrapped;
    let cache = Arc::new(Mutex::new(cache));
    store.attach_search_cache(cache.clone());
    *app.state::<tauri_plugin_tantivy::CacheState>()
        .1
        .write()
        .unwrap() = Some(cache.lock().unwrap().reader());
    *app.state::<tauri_plugin_tantivy::CacheState>()
        .0
        .write()
        .unwrap() = Some(cache);
    Ok(ready)
}

pub fn spawn(app: tauri::AppHandle, store: Arc<SessionStore>) {
    let mut changes = store.subscribe_index_changes();
    tauri::async_runtime::spawn(async move {
        if app
            .state::<crate::startup::StartupState>()
            .wait_until_ready()
            .await
            .is_err()
        {
            return;
        }
        // Startup already reconciled its source snapshot; canonical writes made
        // during startup also have durable repair records and filesystem events.
        while changes.try_recv().is_ok() {}
        loop {
            if app
                .state::<tauri_plugin_tantivy::CacheState>()
                .0
                .read()
                .unwrap()
                .is_none()
            {
                let _ = open(&app, &store).await;
            }
            let cache = app
                .state::<tauri_plugin_tantivy::CacheState>()
                .0
                .read()
                .unwrap()
                .clone();
            if let Some(cache) = cache {
                let mut ids = std::collections::BTreeSet::new();
                while let Ok((entity, changed)) = changes.try_recv() {
                    if matches!(
                        entity,
                        IndexEntity::Sessions
                            | IndexEntity::Docs
                            | IndexEntity::Transcripts
                            | IndexEntity::Tasks
                    ) {
                        ids.extend(changed.into_iter().filter(|id| !id.is_empty()));
                    }
                }
                if !ids.is_empty() {
                    let _ = tokio::task::spawn_blocking(move || {
                        let mut cache = cache.lock().unwrap();
                        if let Err(error) = cache.mark_many(ids) {
                            tracing::warn!(%error, "cannot queue search repair");
                        }
                    })
                    .await;
                }
                if let Err(error) = store.reconcile_incremental().await {
                    tracing::warn!(%error, "search maintenance will retry");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    });
}
