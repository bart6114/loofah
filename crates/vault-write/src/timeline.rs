use std::{
    cmp::Ordering,
    collections::{BTreeMap, BinaryHeap, HashSet},
};

use hypr_loofah_protocol::*;
use serde::{Deserialize, Serialize};

use crate::{SessionMeta, SessionStore, StoreError};

const SCAN_ENTRIES_LIMIT: usize = 1_000_000;
const SCAN_TEXT_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    version: u32,
    revision: String,
    view: TimelineView,
    limit: u32,
    last_id: String,
}

#[derive(Clone, Copy)]
struct Candidate<'a> {
    meta: &'a SessionMeta,
    timestamp: Option<i64>,
    oldest_first: bool,
}

impl PartialEq for Candidate<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Candidate<'_> {}
impl PartialOrd for Candidate<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Candidate<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        let time = if self.oldest_first {
            self.timestamp
                .unwrap_or(0)
                .cmp(&other.timestamp.unwrap_or(0))
        } else {
            other
                .timestamp
                .unwrap_or(0)
                .cmp(&self.timestamp.unwrap_or(0))
        };
        self.oldest_first
            .cmp(&other.oldest_first)
            .then(time)
            .then_with(|| {
                self.meta
                    .title
                    .encode_utf16()
                    .cmp(other.meta.title.encode_utf16())
            })
            .then_with(|| self.meta.created_at.cmp(&other.meta.created_at))
            .then_with(|| self.meta.id.cmp(&other.meta.id))
    }
}

// The client owns day boundaries and labels. Only unambiguous stored instants are
// projected here; naive/locale strings retain their raw value in the Undated view.
fn timestamp(value: &str) -> Option<i64> {
    if let Ok(date) = chrono::DateTime::parse_from_rfc3339(value) {
        if date.timestamp_subsec_nanos() >= 1_000_000_000 {
            return None;
        }
        return Some(date.timestamp_millis());
    }
    if value.len() == 10 && value.as_bytes()[4] == b'-' && value.as_bytes()[7] == b'-' {
        return chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d")
            .ok()?
            .and_hms_opt(0, 0, 0)
            .map(|v| v.and_utc().timestamp_millis());
    }
    None
}

fn limit(message: &str) -> StoreError {
    StoreError::ResourceLimit(message.into())
}
fn changed() -> StoreError {
    StoreError::Conflict("Timeline changed; restart pagination".into())
}
fn charge(budget: &mut usize, bytes: usize) -> Result<(), StoreError> {
    *budget = budget
        .checked_sub(bytes)
        .ok_or_else(|| limit("Timeline projection text limit exceeded"))?;
    Ok(())
}
fn count(value: usize) -> Result<u32, StoreError> {
    u32::try_from(value).map_err(|_| limit("Timeline count limit exceeded"))
}

fn tag_paths(meta: &SessionMeta, budget: &mut usize) -> Result<HashSet<String>, StoreError> {
    let mut paths = HashSet::new();
    for tag in &meta.tags {
        charge(budget, tag.len().saturating_add(64))?;
        if tag.len() > 4096 {
            return Err(limit("Timeline tag path limit exceeded"));
        }
        let path = tag
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect::<Vec<_>>()
            .join("/");
        if !path.is_empty() {
            paths.insert(path);
        }
    }
    Ok(paths)
}

fn matches(view: &TimelineView, candidate: &Candidate<'_>, paths: &HashSet<String>) -> bool {
    match view {
        TimelineView::Date { before_ms } => {
            candidate.timestamp.is_some_and(|value| value < *before_ms)
        }
        TimelineView::Upcoming {
            after_ms,
            before_ms,
        } => candidate.timestamp.is_some_and(|value| {
            value > *after_ms && before_ms.is_none_or(|before| value < before)
        }),
        TimelineView::Tag { path } => paths.contains(path),
        TimelineView::Untagged {} => paths.is_empty(),
        TimelineView::Undated {} => candidate.timestamp.is_none(),
    }
}

fn bounded<T: Serialize>(value: T) -> Result<T, StoreError> {
    if serde_json::to_vec(&value)
        .map_err(|_| StoreError::Serialize("Timeline serialization failed".into()))?
        .len()
        > TIMELINE_RESPONSE_LIMIT
    {
        return Err(limit("Timeline response limit exceeded"));
    }
    Ok(value)
}

impl SessionStore {
    pub fn indexed_timeline_headers(
        &self,
        input: &TimelineQuery,
    ) -> Result<TimelinePage, StoreError> {
        self.ensure_ready()?;
        validate_timeline_query(input).map_err(limit)?;
        let cursor = input
            .cursor
            .as_deref()
            .map(|raw| {
                serde_json::from_str::<Cursor>(raw).map_err(|_| limit("Invalid timeline cursor"))
            })
            .transpose()?;
        let index = self.index.read().unwrap();
        let revision = index.timeline_revision.to_string();
        if input
            .expected_revision
            .as_ref()
            .is_some_and(|expected| expected != &revision)
        {
            return Err(changed());
        }
        let after = if let Some(cursor) = &cursor {
            if cursor.version != 1 || cursor.view != input.view || cursor.limit != input.limit {
                return Err(limit("Timeline cursor does not match query"));
            }
            if cursor.revision != revision {
                return Err(changed());
            }
            let meta = &index
                .sessions
                .get(&cursor.last_id)
                .ok_or_else(changed)?
                .meta;
            let after = Candidate {
                meta,
                timestamp: timestamp(&meta.created_at),
                oldest_first: matches!(input.view, TimelineView::Upcoming { .. }),
            };
            let mut budget = SCAN_TEXT_LIMIT;
            if !matches(&input.view, &after, &tag_paths(meta, &mut budget)?) {
                return Err(limit("Timeline cursor does not match view"));
            }
            Some(after)
        } else {
            None
        };
        if index.sessions.len() > SCAN_ENTRIES_LIMIT {
            return Err(limit("Timeline session limit exceeded"));
        }
        let mut budget = SCAN_TEXT_LIMIT;
        let mut candidates = BinaryHeap::new();
        let mut total = 0;
        let mut future = 0;
        let mut undated = 0;
        for entry in index.sessions.values() {
            let meta = &entry.meta;
            charge(
                &mut budget,
                meta.id.len() + meta.title.len() + meta.created_at.len(),
            )?;
            let candidate = Candidate {
                meta,
                timestamp: timestamp(&meta.created_at),
                oldest_first: matches!(input.view, TimelineView::Upcoming { .. }),
            };
            if candidate.timestamp.is_none() {
                undated += 1;
            }
            if let TimelineView::Date { before_ms } = &input.view
                && candidate.timestamp.is_some_and(|value| value >= *before_ms)
            {
                future += 1;
            }
            let paths = tag_paths(meta, &mut budget)?;
            if !matches(&input.view, &candidate, &paths) {
                continue;
            }
            total += 1;
            if after.is_some_and(|after| candidate <= after) {
                continue;
            }
            candidates.push(candidate);
            if candidates.len() > input.limit as usize + 1 {
                candidates.pop();
            }
        }
        let mut candidates = candidates.into_sorted_vec();
        let has_more = candidates.len() > input.limit as usize;
        candidates.truncate(input.limit as usize);
        let next_cursor = if has_more {
            Some(
                serde_json::to_string(&Cursor {
                    version: 1,
                    revision: revision.clone(),
                    view: input.view.clone(),
                    limit: input.limit,
                    last_id: candidates.last().unwrap().meta.id.clone(),
                })
                .map_err(|_| {
                    StoreError::Serialize("Timeline cursor serialization failed".into())
                })?,
            )
        } else {
            None
        };
        let mut projection_budget = TIMELINE_RESPONSE_LIMIT;
        let mut items = Vec::with_capacity(candidates.len());
        for candidate in candidates {
            let meta = candidate.meta;
            charge(
                &mut projection_budget,
                meta.id.len()
                    + meta.title.len()
                    + meta.created_at.len()
                    + meta.folder.as_deref().map_or(0, str::len)
                    + meta.author.as_deref().map_or(0, str::len),
            )?;
            for tag in &meta.tags {
                charge(&mut projection_budget, tag.len().saturating_add(8))?;
            }
            items.push(TimelineHeader {
                header: index.session_header(&meta.id).unwrap(),
                timestamp_ms: candidate.timestamp,
            });
        }
        drop(index);
        bounded(TimelinePage {
            items,
            next_cursor,
            revision,
            total_count: count(total)?,
            future_count: count(future)?,
            undated_count: count(undated)?,
        })
    }

    pub fn indexed_timeline_facets(
        &self,
        input: &TimelineFacetsQuery,
    ) -> Result<TimelineFacets, StoreError> {
        self.ensure_ready()?;
        validate_timeline_facets_query(input).map_err(limit)?;
        let index = self.index.read().unwrap();
        let revision = index.timeline_revision.to_string();
        if input
            .expected_revision
            .as_ref()
            .is_some_and(|expected| expected != &revision)
        {
            return Err(changed());
        }
        if index.sessions.len() > SCAN_ENTRIES_LIMIT {
            return Err(limit("Timeline session limit exceeded"));
        }
        let mut budget = SCAN_TEXT_LIMIT;
        let mut projection_budget = TIMELINE_RESPONSE_LIMIT;
        let mut nodes: BTreeMap<String, TimelineTagFacet> = BTreeMap::new();
        let mut untagged = 0;
        for entry in index.sessions.values() {
            let paths = tag_paths(&entry.meta, &mut budget)?;
            if paths.is_empty() {
                untagged += 1;
            }
            let mut ancestors = HashSet::new();
            for path in &paths {
                let mut parent = None;
                let mut current = String::new();
                for segment in path.split('/') {
                    if !current.is_empty() {
                        current.push('/');
                    }
                    current.push_str(segment);
                    if !nodes.contains_key(&current) {
                        if nodes.len() >= TIMELINE_FACET_LIMIT {
                            return Err(limit("Timeline facet limit exceeded"));
                        }
                        charge(
                            &mut projection_budget,
                            current.len() + parent.as_deref().map_or(0, str::len) + segment.len(),
                        )?;
                        nodes.insert(
                            current.clone(),
                            TimelineTagFacet {
                                path: current.clone(),
                                parent_path: parent.clone(),
                                label: segment.into(),
                                direct_count: 0,
                                subtree_count: 0,
                            },
                        );
                    }
                    ancestors.insert(current.clone());
                    parent = Some(current.clone());
                }
                nodes.get_mut(path).unwrap().direct_count += 1;
            }
            for path in ancestors {
                nodes.get_mut(&path).unwrap().subtree_count += 1;
            }
        }
        drop(index);
        bounded(TimelineFacets {
            revision,
            nodes: nodes.into_values().collect(),
            untagged_count: count(untagged)?,
        })
    }
}

#[cfg(test)]
mod tests;
