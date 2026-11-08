use super::*;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

pub(super) fn validate_operation(operation: &RemoteOperation) -> Result<()> {
    match operation {
        RemoteOperation::TimelineHeaders { input, .. } => {
            validate_timeline_query(input).map_err(invalid)
        }
        RemoteOperation::TimelineFacets { input, .. } => {
            validate_timeline_facets_query(input).map_err(invalid)
        }
        _ => Ok(()),
    }
}
fn malformed() -> ApplicationError {
    failure(
        ErrorCode::Unavailable,
        "Server returned an invalid timeline projection",
    )
}
fn canonical_tags(row: &SessionListHeader) -> HashSet<String> {
    row.tags
        .iter()
        .map(|tag| {
            tag.split('/')
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join("/")
        })
        .filter(|path| !path.is_empty())
        .collect()
}
fn compare(a: &TimelineHeader, b: &TimelineHeader, oldest_first: bool) -> Ordering {
    let time = if oldest_first {
        a.timestamp_ms
            .unwrap_or(0)
            .cmp(&b.timestamp_ms.unwrap_or(0))
    } else {
        b.timestamp_ms
            .unwrap_or(0)
            .cmp(&a.timestamp_ms.unwrap_or(0))
    };
    time.then_with(|| {
        a.header
            .title
            .encode_utf16()
            .cmp(b.header.title.encode_utf16())
    })
    .then_with(|| a.header.created_at.cmp(&b.header.created_at))
    .then_with(|| a.header.id.cmp(&b.header.id))
}
pub(super) fn validate_page(page: &TimelinePage, input: &TimelineQuery) -> Result<()> {
    if !valid_timeline_revision(&page.revision)
        || input
            .expected_revision
            .as_ref()
            .is_some_and(|revision| revision != &page.revision)
        || page.items.len() > input.limit as usize
        || page.total_count < (page.items.len() as u32)
        || (input.cursor.is_none()
            && page.next_cursor.is_none()
            && page.total_count != page.items.len() as u32)
        || (page.next_cursor.is_some() && page.total_count <= page.items.len() as u32)
        || (matches!(input.view, TimelineView::Undated {})
            && page.total_count != page.undated_count)
        || page.next_cursor.as_ref().is_some_and(|cursor| {
            cursor.is_empty()
                || cursor.len() > TIMELINE_CURSOR_LIMIT
                || Some(cursor) == input.cursor.as_ref()
                || page.items.len() != input.limit as usize
        })
        || page.items.windows(2).any(|rows| {
            compare(
                &rows[0],
                &rows[1],
                matches!(input.view, TimelineView::Upcoming { .. }),
            ) != Ordering::Less
        })
        || (!matches!(input.view, TimelineView::Date { .. }) && page.future_count != 0)
    {
        return Err(malformed());
    }
    let mut seen = HashSet::new();
    for item in &page.items {
        let row = &item.header;
        if hypr_loofah_protocol::validate_session_id(&row.id).is_err()
            || !seen.insert(&row.id)
            || item
                .timestamp_ms
                .is_some_and(|stamp| !(-TIMELINE_DATE_LIMIT..=TIMELINE_DATE_LIMIT).contains(&stamp))
        {
            return Err(malformed());
        }
        let valid = match &input.view {
            TimelineView::Date { before_ms } => {
                item.timestamp_ms.is_some_and(|stamp| stamp < *before_ms)
            }
            TimelineView::Upcoming {
                after_ms,
                before_ms,
            } => item.timestamp_ms.is_some_and(|stamp| {
                stamp > *after_ms && before_ms.is_none_or(|before| stamp < before)
            }),
            TimelineView::Tag { path } => canonical_tags(row).contains(path),
            TimelineView::Untagged {} => canonical_tags(row).is_empty(),
            TimelineView::Undated {} => item.timestamp_ms.is_none(),
        };
        if !valid {
            return Err(malformed());
        }
    }
    Ok(())
}
pub(super) fn validate_facets(reply: &TimelineFacets, input: &TimelineFacetsQuery) -> Result<()> {
    if !valid_timeline_revision(&reply.revision)
        || input
            .expected_revision
            .as_ref()
            .is_some_and(|revision| revision != &reply.revision)
        || reply.nodes.len() > TIMELINE_FACET_LIMIT
    {
        return Err(malformed());
    }
    let nodes: HashMap<_, _> = reply
        .nodes
        .iter()
        .map(|node| (node.path.as_str(), node))
        .collect();
    if nodes.len() != reply.nodes.len() {
        return Err(malformed());
    }
    for node in &reply.nodes {
        if !valid_timeline_tag_path(&node.path)
            || node.direct_count > node.subtree_count
            || node.subtree_count == 0
        {
            return Err(malformed());
        }
        let (parent, label) = node
            .path
            .rsplit_once('/')
            .map_or((None, node.path.as_str()), |(parent, label)| {
                (Some(parent), label)
            });
        if node.parent_path.as_deref() != parent || node.label != label {
            return Err(malformed());
        }
        if let Some(parent) = parent
            && nodes
                .get(parent)
                .is_none_or(|ancestor| ancestor.subtree_count < node.subtree_count)
        {
            return Err(malformed());
        }
    }
    Ok(())
}
