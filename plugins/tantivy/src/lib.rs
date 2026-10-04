mod commands;
mod error;
mod ext;
mod schema;
mod tokenizer;

use std::collections::HashMap;
use tantivy::schema::Schema;
use tantivy::{Index, IndexReader, IndexWriter};
use tauri::Manager;
use tokio::sync::RwLock;

pub use error::{Error, Result};
pub use ext::*;
pub use schema::build_schema;
pub use tokenizer::{get_tokenizer_name_for_language, register_tokenizers};

const PLUGIN_NAME: &str = "tantivy";

pub use hypr_search_cache::{
    CreatedAtFilter, HighlightRange, SearchDocument, SearchFilters, SearchHit, SearchOptions,
    SearchRequest, SearchResult, Snippet,
};

// 2 -> 3: title/content moved from 1-3 char ngrams to whole-word tokens; the
// index must be rebuilt or every existing posting is an unmatchable fragment.
// 5 -> 6 removes the retired related-tag content field.
pub const SCHEMA_VERSION: u32 = 6;

pub struct CollectionConfig {
    pub name: String,
    pub path: String,
    pub schema_builder: fn() -> Schema,
    pub schema_version: u32,
}

pub struct CollectionIndex {
    pub schema: Schema,
    pub index: Index,
    pub reader: IndexReader,
    pub writer: IndexWriter,
}

#[derive(Default)]
pub struct IndexStateInner {
    pub collections: HashMap<String, CollectionIndex>,
}

#[derive(Default)]
pub struct CacheState(
    pub std::sync::RwLock<Option<std::sync::Arc<std::sync::Mutex<hypr_search_cache::Cache>>>>,
    pub std::sync::RwLock<Option<hypr_search_cache::SearchReader>>,
);

pub struct IndexState {
    pub inner: RwLock<IndexStateInner>,
}

impl Default for IndexState {
    fn default() -> Self {
        Self {
            inner: RwLock::new(IndexStateInner::default()),
        }
    }
}

fn make_specta_builder<R: tauri::Runtime>() -> tauri_specta::Builder<R> {
    tauri_specta::Builder::<R>::new()
        .plugin_name(PLUGIN_NAME)
        .commands(tauri_specta::collect_commands![
            commands::search::<tauri::Wry>,
            commands::reindex::<tauri::Wry>,
            commands::add_document::<tauri::Wry>,
            commands::update_document::<tauri::Wry>,
            commands::update_documents::<tauri::Wry>,
            commands::remove_document::<tauri::Wry>,
        ])
        .error_handling(tauri_specta::ErrorHandlingMode::Result)
}

pub fn init() -> tauri::plugin::TauriPlugin<tauri::Wry> {
    let specta_builder = make_specta_builder();

    tauri::plugin::Builder::new(PLUGIN_NAME)
        .invoke_handler(specta_builder.invoke_handler())
        .setup(|app, _api| {
            app.manage(IndexState::default());

            app.manage(CacheState::default());

            Ok(())
        })
        .build()
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn export_types() {
        const OUTPUT_FILE: &str = "./js/bindings.gen.ts";

        make_specta_builder::<tauri::Wry>()
            .export(
                specta_typescript::Typescript::default()
                    .formatter(specta_typescript::formatter::prettier)
                    .bigint(specta_typescript::BigIntExportBehavior::Number),
                OUTPUT_FILE,
            )
            .unwrap();

        let content = std::fs::read_to_string(OUTPUT_FILE).unwrap();
        std::fs::write(OUTPUT_FILE, format!("// @ts-nocheck\n{content}")).unwrap();
    }
}
