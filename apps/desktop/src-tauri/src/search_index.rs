use crate::session_store::{IndexEntity, SessionStore};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;
use tauri_plugin_tantivy::TantivyPluginExt;

pub fn spawn(app: tauri::AppHandle, store: Arc<SessionStore>) {
    use tauri::Manager;
    let mut changes = store.subscribe_index_changes();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = app
            .state::<crate::startup::StartupState>()
            .wait_until_ready()
            .await
        {
            tracing::warn!(%error, "search cache stopped because vault startup failed");
            return;
        }
        loop {
            match app.tantivy().initialize().await {
                Ok(()) => break,
                Err(error) => {
                    tracing::error!(%error, "failed to initialize search cache");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
        let mut pending = BTreeSet::new();
        loop {
            let full = tokio::select! {
                change = changes.recv() => match change {
                    Some((entity, ids)) => {
                        if matches!(entity, IndexEntity::Sessions | IndexEntity::Docs | IndexEntity::Transcripts) {
                            pending.extend(ids);
                        } else if entity != IndexEntity::Tags { continue; }
                        false
                    },
                    None => break,
                },
                _ = tokio::time::sleep(Duration::from_secs(5)) => true,
            };
            // Keep affected IDs while coalescing events, so recording updates do not scan the whole vault.
            while let Ok((entity, ids)) = changes.try_recv() {
                if matches!(
                    entity,
                    IndexEntity::Sessions | IndexEntity::Docs | IndexEntity::Transcripts
                ) {
                    pending.extend(ids);
                }
            }
            let result = if full {
                app.tantivy().reconcile().await
            } else {
                app.tantivy()
                    .reconcile_sessions(pending.iter().cloned().collect())
                    .await
            };
            match result {
                Ok(()) => pending.clear(),
                Err(error) => {
                    tracing::warn!(%error, "failed to reconcile search cache; retrying in background")
                }
            }
        }
    });
}
