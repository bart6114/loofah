use std::sync::Arc;

use hypr_loofah_client::{DraftCache, NoteDraftRecovery, failure};
use hypr_loofah_protocol::{ApplicationError, ErrorCode};
use tauri::Manager;

#[derive(Default)]
pub struct DraftCacheState(tokio::sync::OnceCell<Arc<DraftCache>>);

async fn cache<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
) -> Result<Arc<DraftCache>, ApplicationError> {
    let state = app.state::<DraftCacheState>();
    state
        .0
        .get_or_try_init(|| async {
            let path = app
                .path()
                .app_local_data_dir()
                .map_err(|_| {
                    failure(
                        ErrorCode::Internal,
                        "Cannot locate device draft recovery storage",
                    )
                })?
                .join("note-drafts");
            Ok(Arc::new(DraftCache::new(path)?))
        })
        .await
        .cloned()
}

#[tauri::command]
#[specta::specta]
pub async fn draft_cache_get<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    scope: String,
    session_id: String,
) -> Result<Option<NoteDraftRecovery>, ApplicationError> {
    cache(&app)
        .await?
        .get(window.label().into(), scope, session_id)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn draft_cache_put<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    recovery: NoteDraftRecovery,
) -> Result<(), ApplicationError> {
    cache(&app)
        .await?
        .put(window.label().into(), recovery)
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn draft_cache_remove<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    window: tauri::Window<R>,
    scope: String,
    session_id: String,
) -> Result<(), ApplicationError> {
    cache(&app)
        .await?
        .remove(window.label().into(), scope, session_id)
        .await
}
