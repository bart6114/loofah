use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashSet},
};

use hypr_loofah_protocol::{
    SESSION_SUMMARY_TEXT_LIMIT, SessionSummary, SessionSummaryOrder, SessionSummaryQuery,
};

use crate::{SessionMeta, SessionStore, StoreError};

impl SessionStore {
    pub fn indexed_session_summaries(
        &self,
        input: &SessionSummaryQuery,
    ) -> Result<Vec<SessionSummary>, StoreError> {
        self.ensure_ready()?;
        hypr_loofah_protocol::validate_session_summary_query(input)
            .map_err(|message| StoreError::ResourceLimit(message.into()))?;
        let index = self.index.read().unwrap();
        let mut budget = SESSION_SUMMARY_TEXT_LIMIT;
        let ids = match input {
            SessionSummaryQuery::Ids { ids } => {
                let mut seen = HashSet::new();
                ids.iter()
                    .filter(|id| seen.insert(id.as_str()))
                    .map(String::as_str)
                    .collect()
            }
            SessionSummaryQuery::List {
                limit,
                order,
                titled_only,
            } => {
                let entries = index
                    .sessions
                    .values()
                    .filter(|entry| !*titled_only || !entry.meta.title.is_empty());
                match order {
                    SessionSummaryOrder::OldestFirst => first(
                        entries
                            .map(|entry| (entry.meta.created_at.as_str(), entry.meta.id.as_str())),
                        *limit as usize,
                    ),
                    SessionSummaryOrder::NewestFirst => first(
                        entries.map(|entry| {
                            (
                                Reverse(entry.meta.created_at.as_str()),
                                entry.meta.id.as_str(),
                            )
                        }),
                        *limit as usize,
                    ),
                }
            }
            SessionSummaryQuery::Title { query, limit } => {
                let query = query.trim().to_lowercase();
                first(
                    index
                        .sessions
                        .values()
                        .filter(|entry| entry.meta.title.to_lowercase().contains(&query))
                        .map(|entry| {
                            (
                                Reverse(entry.meta.created_at.as_str()),
                                entry.meta.id.as_str(),
                            )
                        }),
                    *limit as usize,
                )
            }
        };
        ids.into_iter()
            .filter_map(|id| index.sessions.get(id))
            .map(|entry| project(&entry.meta, &mut budget))
            .collect()
    }

    pub async fn canonical_session_summaries(
        &self,
        operation_root: Option<&std::path::Path>,
        ids: &[String],
    ) -> Result<Vec<SessionSummary>, StoreError> {
        self.ensure_ready()?;
        hypr_loofah_protocol::validate_session_summary_query(&SessionSummaryQuery::Ids {
            ids: ids.to_vec(),
        })
        .map_err(|message| StoreError::ResourceLimit(message.into()))?;
        let mut seen = HashSet::new();
        let mut summaries = Vec::with_capacity(ids.len());
        let mut budget = SESSION_SUMMARY_TEXT_LIMIT;
        let mut bounded = self.clone();
        bounded.content_limits.metadata_bytes =
            bounded.content_limits.metadata_bytes.min(8 * 1024 * 1024);
        for id in ids {
            if seen.insert(id)
                && let Some(snapshot) = bounded
                    .read_recovered_metadata_snapshot(operation_root, id)
                    .await?
            {
                summaries.push(project(&snapshot.meta, &mut budget)?);
            }
        }
        Ok(summaries)
    }
}

fn project(meta: &SessionMeta, budget: &mut usize) -> Result<SessionSummary, StoreError> {
    let bytes = meta
        .id
        .len()
        .saturating_add(meta.title.len())
        .saturating_add(meta.created_at.len());
    *budget = budget.checked_sub(bytes).ok_or_else(|| {
        StoreError::ResourceLimit("Session summary response exceeds its text budget".into())
    })?;
    Ok(SessionSummary {
        id: meta.id.clone(),
        title: meta.title.clone(),
        created_at: meta.created_at.clone(),
    })
}

fn first<'a, K: Ord>(keys: impl Iterator<Item = (K, &'a str)>, limit: usize) -> Vec<&'a str> {
    let mut selected = BinaryHeap::with_capacity(limit);
    for key in keys {
        if selected.len() < limit {
            selected.push(key);
        } else if selected.peek().is_some_and(|last| &key < last) {
            *selected.peek_mut().expect("selection is full") = key;
        }
    }
    selected
        .into_sorted_vec()
        .into_iter()
        .map(|(_, id)| id)
        .collect()
}

#[cfg(test)]
mod tests;
