use serde::{Deserialize, Serialize};

pub const TIMELINE_PAGE_LIMIT: u32 = 200;
pub const TIMELINE_RESPONSE_LIMIT: usize = 1024 * 1024;
pub const TIMELINE_FACET_LIMIT: usize = 4096;
pub const TIMELINE_CURSOR_LIMIT: usize = 32 * 1024;
pub const TIMELINE_DATE_LIMIT: i64 = 8_640_000_000_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SessionListHeader {
    pub id: String,
    pub title: String,
    pub created_at: String,
    pub folder: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub has_transcript_words: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TimelineHeader {
    pub header: SessionListHeader,
    /// RFC3339 with an offset, or ISO date-only at UTC midnight. Other raw
    /// strings remain visible through Undated and are never rewritten.
    pub timestamp_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
/// Date is newest-first with an exclusive upper bound. Upcoming is oldest-first
/// with exclusive lower/optional upper bounds. Undated never substitutes server time.
pub enum TimelineView {
    Date {
        before_ms: i64,
    },
    Upcoming {
        after_ms: i64,
        before_ms: Option<i64>,
    },
    Tag {
        path: String,
    },
    Untagged {},
    Undated {},
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TimelineQuery {
    pub view: TimelineView,
    pub limit: u32,
    pub cursor: Option<String>,
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TimelinePage {
    pub items: Vec<TimelineHeader>,
    pub next_cursor: Option<String>,
    pub revision: String,
    /// Exact matching count over the whole index, including preceding pages.
    pub total_count: u32,
    /// Date only: all dated sessions at/after its cutoff. Zero for other views.
    pub future_count: u32,
    /// All undated sessions in the index, independent of the selected view.
    pub undated_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TimelineFacetsQuery {
    pub expected_revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TimelineTagFacet {
    pub path: String,
    pub parent_path: Option<String>,
    pub label: String,
    pub direct_count: u32,
    pub subtree_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TimelineFacets {
    pub revision: String,
    /// Complete tree, never truncated. Clients retain locale-specific sibling
    /// ordering; only expanded nodes need their direct session pages.
    pub nodes: Vec<TimelineTagFacet>,
    pub untagged_count: u32,
}

pub fn valid_timeline_revision(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, ch)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                ch == b'-'
            } else {
                ch.is_ascii_digit() || (b'a'..=b'f').contains(&ch)
            }
        })
}

pub fn valid_timeline_tag_path(path: &str) -> bool {
    !path.is_empty() && path.len() <= 4096 && path.split('/').all(|part| !part.is_empty())
}

pub fn validate_timeline_query(input: &TimelineQuery) -> Result<(), &'static str> {
    if !(1..=TIMELINE_PAGE_LIMIT).contains(&input.limit) {
        return Err("Timeline limit must be between 1 and 200");
    }
    if input
        .cursor
        .as_ref()
        .is_some_and(|v| v.is_empty() || v.len() > TIMELINE_CURSOR_LIMIT)
    {
        return Err("Invalid timeline cursor");
    }
    if input
        .expected_revision
        .as_deref()
        .is_some_and(|v| !valid_timeline_revision(v))
    {
        return Err("Invalid timeline revision");
    }
    match &input.view {
        TimelineView::Date { before_ms }
            if !(-TIMELINE_DATE_LIMIT..=TIMELINE_DATE_LIMIT).contains(before_ms) =>
        {
            Err("Invalid timeline cutoff")
        }
        TimelineView::Upcoming {
            after_ms,
            before_ms,
        } if !(-TIMELINE_DATE_LIMIT..=TIMELINE_DATE_LIMIT).contains(after_ms)
            || before_ms.is_some_and(|before| {
                !(-TIMELINE_DATE_LIMIT..=TIMELINE_DATE_LIMIT).contains(&before)
                    || before <= *after_ms
            }) =>
        {
            Err("Invalid upcoming timeline window")
        }
        TimelineView::Tag { path } if !valid_timeline_tag_path(path) => {
            Err("Invalid timeline tag path")
        }
        _ => Ok(()),
    }
}

pub fn validate_timeline_facets_query(input: &TimelineFacetsQuery) -> Result<(), &'static str> {
    if input
        .expected_revision
        .as_deref()
        .is_some_and(|v| !valid_timeline_revision(v))
    {
        Err("Invalid timeline revision")
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeline_contract_bounds_requests_and_rejects_future_fields() {
        for view in [
            TimelineView::Date { before_ms: 0 },
            TimelineView::Upcoming {
                after_ms: 0,
                before_ms: None,
            },
            TimelineView::Tag {
                path: " A /b".into(),
            },
            TimelineView::Untagged {},
            TimelineView::Undated {},
        ] {
            let query = TimelineQuery {
                view,
                limit: 200,
                cursor: None,
                expected_revision: None,
            };
            assert!(validate_timeline_query(&query).is_ok());
            assert_eq!(
                serde_json::from_value::<TimelineQuery>(serde_json::to_value(&query).unwrap())
                    .unwrap(),
                query
            );
        }
        for value in [
            serde_json::json!({"kind":"untagged","path":"secret"}),
            serde_json::json!({"kind":"undated","future":true}),
            serde_json::json!({"kind":"date","before_ms":0,"timezone":"UTC"}),
        ] {
            assert!(serde_json::from_value::<TimelineView>(value).is_err());
        }
        for path in ["", "/a", "a//b", "a/"] {
            assert!(!valid_timeline_tag_path(path));
        }
        let mut query = TimelineQuery {
            view: TimelineView::Undated {},
            limit: 0,
            cursor: None,
            expected_revision: None,
        };
        assert!(validate_timeline_query(&query).is_err());
        query.limit = 201;
        assert!(validate_timeline_query(&query).is_err());
        query.limit = 1;
        query.cursor = Some("x".repeat(TIMELINE_CURSOR_LIMIT + 1));
        assert!(validate_timeline_query(&query).is_err());
        for view in [
            TimelineView::Upcoming {
                after_ms: 1,
                before_ms: Some(1),
            },
            TimelineView::Upcoming {
                after_ms: TIMELINE_DATE_LIMIT + 1,
                before_ms: None,
            },
            TimelineView::Date {
                before_ms: TIMELINE_DATE_LIMIT + 1,
            },
        ] {
            let query = TimelineQuery {
                view,
                limit: 1,
                cursor: None,
                expected_revision: None,
            };
            assert!(validate_timeline_query(&query).is_err());
        }
        assert!(!valid_timeline_revision("../vault"));
        assert!(valid_timeline_revision(
            "00000000-0000-0000-0000-000000000000"
        ));
    }
}
