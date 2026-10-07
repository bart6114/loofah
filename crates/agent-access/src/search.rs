use crate::{Pagination, Result, pagination};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use specta::Type;

pub const DEFAULT_SEARCH_LIMIT: u32 = 20;
pub const MAX_SEARCH_LIMIT: u32 = 50;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema, Type)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct SearchMeetingsInput {
    #[schemars(
        description = "Desktop-compatible full-text query: words, quoted phrases, and a trailing word prefix"
    )]
    pub query: String,
    #[schemars(description = "Maximum session hits; defaults to 20 and is capped at 50")]
    #[schemars(range(min = 1, max = 50))]
    pub limit: Option<u32>,
    #[schemars(description = "Number of session hits to skip; defaults to 0")]
    pub offset: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct SearchHit {
    pub session_id: String,
    pub title: String,
    pub created_at: i64,
    pub score: f32,
    pub title_snippet: Option<String>,
    pub content_snippet: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Type)]
pub struct SearchPage {
    pub hits: Vec<SearchHit>,
    pub pagination: Pagination,
}

pub async fn search_meetings(
    cache: &hypr_search_cache::Cache,
    input: SearchMeetingsInput,
) -> Result<SearchPage> {
    let query = hypr_search_cache::normalize_query(&input.query);
    if query.is_empty() {
        return Err(crate::Error::InvalidInput("search requires a query".into()));
    }
    let limit = input
        .limit
        .unwrap_or(DEFAULT_SEARCH_LIMIT)
        .clamp(1, MAX_SEARCH_LIMIT);
    let offset = input.offset.unwrap_or(0);
    let cache = cache.clone();
    tokio::task::spawn_blocking(move || {
        let result = cache.search_fresh(hypr_search_cache::SearchRequest {
            query,
            collection: None,
            filters: Default::default(),
            limit: offset as usize + limit as usize,
            options: hypr_search_cache::SearchOptions {
                snippets: Some(true),
                ..Default::default()
            },
        })?;
        let hits: Vec<_> = result
            .hits
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|hit| SearchHit {
                session_id: hit.document.id,
                title: hit.document.title,
                created_at: hit.document.created_at,
                score: hit.score,
                title_snippet: hit.title_snippet.map(|snippet| snippet.fragment),
                content_snippet: hit.content_snippet.map(|snippet| snippet.fragment),
            })
            .collect();
        let page = pagination(
            offset,
            limit,
            hits.len(),
            Some(result.count),
            offset as usize + hits.len() < result.count,
        );
        Ok(SearchPage {
            hits,
            pagination: page,
        })
    })
    .await
    .map_err(|error| crate::Error::Vault {
        action: "search sessions",
        reason: error.to_string(),
    })?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_contract_requires_query_and_rejects_retired_filters() {
        for input in [
            serde_json::json!({}),
            serde_json::json!({"query":"budget","speaker":"Alice"}),
            serde_json::json!({"query":"budget","kinds":["note"]}),
        ] {
            assert!(serde_json::from_value::<SearchMeetingsInput>(input).is_err());
        }
    }
}
