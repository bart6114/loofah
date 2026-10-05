use crate::{IndexState, SearchDocument, SearchRequest, SearchResult};
use tauri_plugin_settings::SettingsPluginExt;

pub struct Tantivy<'a, R: tauri::Runtime, M: tauri::Manager<R>> {
    manager: &'a M,
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> Tantivy<'a, R, M> {
    pub async fn initialize(&self) -> Result<(), crate::Error> {
        let app = self.manager.app_handle();
        let global = app.settings().global_base()?.into_std_path_buf();
        let vault = app.settings().vault_base()?.into_std_path_buf();
        let cache = hypr_search_cache::Cache::new(&global, &vault)?;
        let worker = cache.clone();
        tokio::task::spawn_blocking(move || worker.initialize(&mut |_| {}))
            .await
            .map_err(|error| crate::Error::Worker(error.to_string()))??;
        *self.manager.state::<IndexState>().cache.write().await = Some(cache);
        Ok(())
    }

    fn validate_collection(collection: &Option<String>) -> Result<(), crate::Error> {
        if let Some(name) = collection
            && name != "default"
        {
            return Err(crate::Error::CollectionNotFound(name.clone()));
        }
        Ok(())
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResult, crate::Error> {
        Self::validate_collection(&request.collection)?;
        let cache = self
            .manager
            .state::<IndexState>()
            .cache
            .read()
            .await
            .clone()
            .ok_or(crate::Error::IndexNotInitialized)?;
        tokio::task::spawn_blocking(move || cache.search(request))
            .await
            .map_err(|error| crate::Error::Worker(error.to_string()))?
            .map_err(Into::into)
    }

    pub async fn reconcile(&self) -> Result<(), crate::Error> {
        let cache = self
            .manager
            .state::<IndexState>()
            .cache
            .read()
            .await
            .clone()
            .ok_or(crate::Error::IndexNotInitialized)?;
        tokio::task::spawn_blocking(move || cache.reconcile(&mut |_| {}))
            .await
            .map_err(|error| crate::Error::Worker(error.to_string()))??;
        Ok(())
    }

    pub async fn reconcile_sessions(&self, ids: Vec<String>) -> Result<(), crate::Error> {
        let cache = self
            .manager
            .state::<IndexState>()
            .cache
            .read()
            .await
            .clone()
            .ok_or(crate::Error::IndexNotInitialized)?;
        tokio::task::spawn_blocking(move || cache.reconcile_sessions(&ids))
            .await
            .map_err(|error| crate::Error::Worker(error.to_string()))??;
        Ok(())
    }

    pub async fn reindex(&self, collection: Option<String>) -> Result<(), crate::Error> {
        Self::validate_collection(&collection)?;
        self.reconcile().await
    }
    // Cache writes always project current vault files; callers cannot overwrite newer content with stale documents.
    pub async fn add_document(
        &self,
        collection: Option<String>,
        _document: SearchDocument,
    ) -> Result<(), crate::Error> {
        self.reindex(collection).await
    }
    pub async fn update_document(
        &self,
        collection: Option<String>,
        _document: SearchDocument,
    ) -> Result<(), crate::Error> {
        self.reindex(collection).await
    }
    pub async fn update_documents(
        &self,
        collection: Option<String>,
        _documents: Vec<SearchDocument>,
    ) -> Result<(), crate::Error> {
        self.reindex(collection).await
    }
    pub async fn remove_document(
        &self,
        collection: Option<String>,
        _id: String,
    ) -> Result<(), crate::Error> {
        self.reindex(collection).await
    }
}

pub trait TantivyPluginExt<R: tauri::Runtime> {
    fn tantivy(&self) -> Tantivy<'_, R, Self>
    where
        Self: tauri::Manager<R> + Sized;
}
impl<R: tauri::Runtime, T: tauri::Manager<R>> TantivyPluginExt<R> for T {
    fn tantivy(&self) -> Tantivy<'_, R, Self> {
        Tantivy {
            manager: self,
            _runtime: std::marker::PhantomData,
        }
    }
}
