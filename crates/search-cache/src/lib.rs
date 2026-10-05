#![forbid(unsafe_code)]

mod cache;
mod error;
mod projection;
mod query;
mod schema;
mod search;
mod tokenizer;

pub use cache::{
    Cache, CacheStatus, Progress, ReconcileReport, SourceState, default_global_base,
    normalize_query,
};
pub use error::{Error, Result};
pub use projection::{SessionListItem, TagItem};
pub use schema::build_schema;
pub use search::search_index;
use serde::{Deserialize, Serialize};
pub use tokenizer::{get_tokenizer_name_for_language, register_tokenizers};
pub const SCHEMA_VERSION: u32 = 7;
pub const PROJECTION_VERSION: u32 = 9;

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SearchDocument {
    pub id: String,
    pub doc_type: String,
    pub language: Option<String>,
    pub title: String,
    pub content: String,
    pub created_at: i64,
    #[serde(default)]
    pub facets: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct Snippet {
    pub fragment: String,
    pub highlights: Vec<HighlightRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct HighlightRange {
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SearchHit {
    pub score: f32,
    pub document: SearchDocument,
    pub title_snippet: Option<Snippet>,
    pub content_snippet: Option<Snippet>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SearchResult {
    pub hits: Vec<SearchHit>,
    pub count: usize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, specta::Type)]
pub struct CreatedAtFilter {
    pub gte: Option<i64>,
    pub lte: Option<i64>,
    pub gt: Option<i64>,
    pub lt: Option<i64>,
    pub eq: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, specta::Type)]
pub struct SearchFilters {
    pub created_at: Option<CreatedAtFilter>,
    pub doc_type: Option<String>,
    pub facet: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, specta::Type)]
pub struct SearchOptions {
    pub fuzzy: Option<bool>,
    pub distance: Option<u8>,
    pub snippets: Option<bool>,
    pub snippet_max_chars: Option<usize>,
    pub phrase_slop: Option<u32>,
}

fn default_limit() -> usize {
    100
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub collection: Option<String>,
    #[serde(default)]
    pub filters: SearchFilters,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default)]
    pub options: SearchOptions,
}
