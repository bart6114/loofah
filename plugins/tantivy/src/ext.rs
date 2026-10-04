use crate::schema::get_fields;
use crate::tokenizer::register_tokenizers;
use crate::{
    CollectionConfig, CollectionIndex, IndexState, SearchDocument, SearchRequest, SearchResult,
};
use tantivy::schema::Facet;
use tantivy::{DateTime, Index, ReloadPolicy, TantivyDocument, Term};
use tauri_plugin_settings::SettingsPluginExt;

pub fn detect_language(text: &str) -> hypr_language::Language {
    hypr_language::detect(text)
}

pub struct Tantivy<'a, R: tauri::Runtime, M: tauri::Manager<R>> {
    manager: &'a M,
    _runtime: std::marker::PhantomData<fn() -> R>,
}

impl<'a, R: tauri::Runtime, M: tauri::Manager<R>> Tantivy<'a, R, M> {
    pub async fn register_collection(&self, config: CollectionConfig) -> Result<(), crate::Error> {
        // The search index is a rebuildable cache, not user data: it lives in the
        // OS app-data dir (global_base), not the vault, so vault sync backends
        // (e.g. Google Drive) never see it churn.
        let global_base = self.manager.app_handle().settings().global_base()?;
        let index_path = global_base.join(&config.path).into_std_path_buf();
        let version_path = index_path.join("schema_version");

        std::fs::create_dir_all(&index_path)?;

        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        if guard.collections.contains_key(&config.name) {
            tracing::debug!("Collection '{}' already registered", config.name);
            return Ok(());
        }

        let schema = (config.schema_builder)();

        let needs_reindex = if index_path.join("meta.json").exists() {
            let stored_version = std::fs::read_to_string(&version_path)
                .ok()
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(0);
            stored_version != config.schema_version
        } else {
            false
        };

        let index = if index_path.join("meta.json").exists() && !needs_reindex {
            Index::open_in_dir(&index_path)?
        } else {
            if needs_reindex {
                tracing::debug!(
                    "Schema version changed for collection '{}', re-creating index",
                    config.name
                );
                std::fs::remove_dir_all(&index_path)?;
                std::fs::create_dir_all(&index_path)?;
            }
            Index::create_in_dir(&index_path, schema.clone())?
        };

        std::fs::write(&version_path, config.schema_version.to_string())?;

        register_tokenizers(&index);

        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;

        let writer = index.writer(50_000_000)?;

        let collection_index = CollectionIndex {
            schema,
            index,
            reader,
            writer,
        };

        guard
            .collections
            .insert(config.name.clone(), collection_index);

        tracing::debug!(
            "Tantivy collection '{}' registered at {:?} (version: {})",
            config.name,
            index_path,
            config.schema_version
        );
        Ok(())
    }

    fn get_collection_name(collection: Option<String>) -> String {
        collection.unwrap_or_else(|| "default".to_string())
    }

    pub async fn search(&self, request: SearchRequest) -> Result<SearchResult, crate::Error> {
        if request
            .collection
            .as_deref()
            .is_none_or(|name| name == "default")
        {
            let cache = self
                .manager
                .try_state::<crate::CacheState>()
                .and_then(|state| state.1.read().unwrap().clone());
            if let Some(cache) = cache {
                return tokio::task::spawn_blocking(move || cache.search(request))
                    .await
                    .map_err(|e| crate::Error::Cache(e.to_string()))?
                    .map_err(|e| crate::Error::Cache(e.to_string()));
            }
        }
        let collection_name = Self::get_collection_name(request.collection.clone());
        let state = self.manager.state::<IndexState>();
        let guard = state.inner.read().await;

        let collection_index = guard
            .collections
            .get(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        Ok(hypr_search_cache::search(
            &collection_index.index,
            &collection_index.reader,
            request,
        )?)
    }

    pub async fn reindex(&self, collection: Option<String>) -> Result<(), crate::Error> {
        if collection.as_deref().is_none_or(|name| name == "default") {
            let cache = self
                .manager
                .try_state::<crate::CacheState>()
                .and_then(|state| state.0.read().unwrap().clone());
            if let Some(cache) = cache {
                return tokio::task::spawn_blocking(move || cache.lock().unwrap().refresh(true))
                    .await
                    .map_err(|e| crate::Error::Cache(e.to_string()))?
                    .map(|_| ())
                    .map_err(|e| crate::Error::Cache(e.to_string()));
            }
        }
        let collection_name = Self::get_collection_name(collection);
        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        let collection_index = guard
            .collections
            .get_mut(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let writer = &mut collection_index.writer;

        writer.delete_all_documents()?;

        let fields = get_fields(schema);

        writer.commit()?;

        tracing::debug!(
            "Reindex completed for collection '{}'. Index cleared and ready for new documents. Fields: {:?}",
            collection_name,
            fields.id
        );

        Ok(())
    }

    pub async fn add_document(
        &self,
        collection: Option<String>,
        document: SearchDocument,
    ) -> Result<(), crate::Error> {
        let collection_name = Self::get_collection_name(collection);
        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        let collection_index = guard
            .collections
            .get_mut(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let writer = &mut collection_index.writer;
        let fields = get_fields(schema);

        let mut doc = TantivyDocument::new();
        doc.add_text(fields.id, &document.id);
        doc.add_text(fields.doc_type, &document.doc_type);
        doc.add_text(fields.language, document.language.as_deref().unwrap_or(""));
        doc.add_text(fields.title, &document.title);
        doc.add_text(fields.content, &document.content);
        doc.add_date(
            fields.created_at,
            DateTime::from_timestamp_millis(document.created_at),
        );

        for facet_path in &document.facets {
            if let Ok(facet) = Facet::from_text(facet_path) {
                doc.add_facet(fields.facets, facet);
            }
        }

        writer.add_document(doc)?;
        writer.commit()?;

        tracing::debug!(
            "Added document '{}' to collection '{}'",
            document.id,
            collection_name
        );

        Ok(())
    }

    pub async fn update_document(
        &self,
        collection: Option<String>,
        document: SearchDocument,
    ) -> Result<(), crate::Error> {
        let collection_name = Self::get_collection_name(collection);
        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        let collection_index = guard
            .collections
            .get_mut(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let writer = &mut collection_index.writer;
        let fields = get_fields(schema);

        let id_term = Term::from_field_text(fields.id, &document.id);
        writer.delete_term(id_term);

        let mut doc = TantivyDocument::new();
        doc.add_text(fields.id, &document.id);
        doc.add_text(fields.doc_type, &document.doc_type);
        doc.add_text(fields.language, document.language.as_deref().unwrap_or(""));
        doc.add_text(fields.title, &document.title);
        doc.add_text(fields.content, &document.content);
        doc.add_date(
            fields.created_at,
            DateTime::from_timestamp_millis(document.created_at),
        );

        for facet_path in &document.facets {
            if let Ok(facet) = Facet::from_text(facet_path) {
                doc.add_facet(fields.facets, facet);
            }
        }

        writer.add_document(doc)?;
        writer.commit()?;

        tracing::debug!(
            "Updated document '{}' in collection '{}'",
            document.id,
            collection_name
        );

        Ok(())
    }

    pub async fn update_documents(
        &self,
        collection: Option<String>,
        documents: Vec<SearchDocument>,
    ) -> Result<(), crate::Error> {
        let collection_name = Self::get_collection_name(collection);
        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        let collection_index = guard
            .collections
            .get_mut(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let writer = &mut collection_index.writer;
        let fields = get_fields(schema);

        let count = documents.len();

        for document in documents {
            let id_term = Term::from_field_text(fields.id, &document.id);
            writer.delete_term(id_term);

            let mut doc = TantivyDocument::new();
            doc.add_text(fields.id, &document.id);
            doc.add_text(fields.doc_type, &document.doc_type);
            doc.add_text(fields.language, document.language.as_deref().unwrap_or(""));
            doc.add_text(fields.title, &document.title);
            doc.add_text(fields.content, &document.content);
            doc.add_date(
                fields.created_at,
                DateTime::from_timestamp_millis(document.created_at),
            );

            for facet_path in &document.facets {
                if let Ok(facet) = Facet::from_text(facet_path) {
                    doc.add_facet(fields.facets, facet);
                }
            }

            writer.add_document(doc)?;
        }

        writer.commit()?;

        tracing::debug!(
            "Updated {} documents in collection '{}'",
            count,
            collection_name
        );

        Ok(())
    }

    pub async fn remove_document(
        &self,
        collection: Option<String>,
        id: String,
    ) -> Result<(), crate::Error> {
        let collection_name = Self::get_collection_name(collection);
        let state = self.manager.state::<IndexState>();
        let mut guard = state.inner.write().await;

        let collection_index = guard
            .collections
            .get_mut(&collection_name)
            .ok_or_else(|| crate::Error::CollectionNotFound(collection_name.clone()))?;

        let schema = &collection_index.schema;
        let writer = &mut collection_index.writer;
        let fields = get_fields(schema);

        let id_term = Term::from_field_text(fields.id, &id);
        writer.delete_term(id_term);
        writer.commit()?;

        tracing::debug!(
            "Removed document '{}' from collection '{}'",
            id,
            collection_name
        );

        Ok(())
    }
}

pub trait TantivyPluginExt<R: tauri::Runtime> {
    fn tantivy(&self) -> Tantivy<'_, R, Self>
    where
        Self: tauri::Manager<R> + Sized;
}

impl<R: tauri::Runtime, T: tauri::Manager<R>> TantivyPluginExt<R> for T {
    fn tantivy(&self) -> Tantivy<'_, R, Self>
    where
        Self: Sized,
    {
        Tantivy {
            manager: self,
            _runtime: std::marker::PhantomData,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenizer::get_tokenizer_name_for_language;
    use crate::{CollectionIndex, IndexState, SearchDocument, SearchFilters, SearchRequest};
    use tauri::Manager;

    #[test]
    fn test_detect_language_tokenizer_integration() {
        let text = "The quick brown fox jumps over the lazy dog.";
        let lang = detect_language(text);
        let tokenizer_name = get_tokenizer_name_for_language(&lang);
        assert_eq!(tokenizer_name, "lang_en");
    }

    async fn harness() -> tauri::App<tauri::test::MockRuntime> {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(IndexState::default());

        let schema = crate::build_schema();
        let index = tantivy::Index::create_in_ram(schema.clone());
        register_tokenizers(&index);
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .unwrap();
        let writer = index.writer(50_000_000).unwrap();
        app.state::<IndexState>()
            .inner
            .write()
            .await
            .collections
            .insert(
                "default".to_string(),
                CollectionIndex {
                    schema,
                    index,
                    reader,
                    writer,
                },
            );
        app
    }

    fn doc(id: &str, title: &str, content: &str) -> SearchDocument {
        SearchDocument {
            id: id.to_string(),
            doc_type: "session".to_string(),
            language: None,
            title: title.to_string(),
            content: content.to_string(),
            created_at: 0,
            facets: Vec::new(),
        }
    }

    async fn search(
        app: &tauri::App<tauri::test::MockRuntime>,
        query: &str,
    ) -> crate::SearchResult {
        {
            let state = app.state::<IndexState>();
            let guard = state.inner.read().await;
            guard.collections["default"].reader.reload().unwrap();
        }
        app.tantivy()
            .search(SearchRequest {
                query: query.to_string(),
                collection: None,
                filters: SearchFilters::default(),
                limit: 10,
                options: crate::SearchOptions {
                    snippets: Some(true),
                    ..Default::default()
                },
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn matches_whole_words_not_letter_fragments() {
        let app = harness().await;
        app.tantivy()
            .add_document(
                None,
                doc(
                    "s1",
                    "M&A process",
                    "dat David zijn werk vastleggen uiteraard",
                ),
            )
            .await
            .unwrap();
        app.tantivy()
            .add_document(None, doc("s2", "tim debrief", "de mailkooning is geworden"))
            .await
            .unwrap();

        // The ngram-era regression: "david" matched every document containing
        // the letters d/a/v/i and highlighted 1-3 char fragments everywhere.
        let result = search(&app, "david").await;
        assert_eq!(result.count, 1);
        assert_eq!(result.hits[0].document.id, "s1");

        // Casing and accents normalize through the analyzer.
        assert_eq!(search(&app, "David").await.count, 1);

        // Subsequence/infix letters must not match.
        assert_eq!(search(&app, "avid").await.count, 0);
        assert_eq!(search(&app, "xyz").await.count, 0);

        // Multi-term queries require every word.
        assert_eq!(search(&app, "david vastleggen").await.count, 1);
        assert_eq!(search(&app, "david mailkooning").await.count, 0);
    }

    #[tokio::test]
    async fn trailing_term_matches_as_prefix_with_whole_word_highlight() {
        let app = harness().await;
        app.tantivy()
            .add_document(None, doc("s1", "debrief", "dat David zijn werk"))
            .await
            .unwrap();

        let result = search(&app, "dav").await;
        assert_eq!(result.count, 1, "search-as-you-type prefix must match");

        let snippet = result.hits[0].content_snippet.as_ref().unwrap();
        let highlighted: Vec<&str> = snippet
            .highlights
            .iter()
            .map(|range| &snippet.fragment[range.start..range.end])
            .collect();
        assert_eq!(
            highlighted,
            vec!["David"],
            "the whole matched word is highlighted, nothing else"
        );

        // A trailing space means the word is finished: no prefix matching.
        assert_eq!(search(&app, "dav ").await.count, 0);
    }
}
