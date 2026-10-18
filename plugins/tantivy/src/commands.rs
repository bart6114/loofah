use crate::{SearchDocument, SearchRequest, SearchResult, TantivyPluginExt};

#[tauri::command]
#[specta::specta]
pub(crate) async fn search<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    request: SearchRequest,
) -> Result<SearchResult, String> {
    use tauri::Manager;
    if let Some(connection) =
        app.try_state::<std::sync::Arc<hypr_vault_access::connection::ConnectionState>>()
    {
        if connection.active() {
            let value = connection
                .call("search", serde_json::json!({"request":request}), true)
                .await
                .map_err(|error| error.to_string())?;
            return serde_json::from_value(value).map_err(|error| error.to_string());
        }
    }

    app.tantivy()
        .search(request)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn reindex<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    collection: Option<String>,
) -> Result<(), String> {
    app.tantivy()
        .reindex(collection)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn add_document<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    document: SearchDocument,
    collection: Option<String>,
) -> Result<(), String> {
    app.tantivy()
        .add_document(collection, document)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn update_document<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    document: SearchDocument,
    collection: Option<String>,
) -> Result<(), String> {
    app.tantivy()
        .update_document(collection, document)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn update_documents<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    documents: Vec<SearchDocument>,
    collection: Option<String>,
) -> Result<(), String> {
    app.tantivy()
        .update_documents(collection, documents)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
#[specta::specta]
pub(crate) async fn remove_document<R: tauri::Runtime>(
    app: tauri::AppHandle<R>,
    id: String,
    collection: Option<String>,
) -> Result<(), String> {
    app.tantivy()
        .remove_document(collection, id)
        .await
        .map_err(|e| e.to_string())
}
