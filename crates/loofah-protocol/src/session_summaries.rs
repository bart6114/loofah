use serde::{Deserialize, Serialize};

pub const SESSION_SUMMARY_TEXT_LIMIT: usize = 1024 * 1024;
pub const SESSION_SUMMARY_RESPONSE_LIMIT: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SessionSummaryOrder {
    NewestFirst,
    OldestFirst,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionSummaryQuery {
    List {
        limit: u32,
        order: SessionSummaryOrder,
        #[serde(default)]
        titled_only: bool,
    },
    Title {
        query: String,
        limit: u32,
    },
    Ids {
        ids: Vec<String>,
    },
}

pub fn validate_session_summary_query(input: &SessionSummaryQuery) -> Result<(), &'static str> {
    match input {
        SessionSummaryQuery::List { limit, .. } | SessionSummaryQuery::Title { limit, .. }
            if !(1..=200).contains(limit) =>
        {
            Err("Session summary limit must be between 1 and 200")
        }
        SessionSummaryQuery::Title { query, .. }
            if query.len() > 4096
                || query.trim().is_empty()
                || query.chars().any(char::is_control) =>
        {
            Err("Invalid session title query")
        }
        SessionSummaryQuery::Ids { ids } => {
            if ids.len() > 100 {
                return Err("At most 100 session summary IDs are allowed");
            }
            for id in ids {
                hypr_vault_read::paths::validate_session_id(id)
                    .map_err(|_| "Invalid session summary ID")?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queries_bound_work_and_reject_paths_controls_and_unknown_fields() {
        let list: SessionSummaryQuery = serde_json::from_value(
            serde_json::json!({"kind":"list","order":"newest_first","limit":200}),
        )
        .unwrap();
        assert!(matches!(
            list,
            SessionSummaryQuery::List {
                titled_only: false,
                ..
            }
        ));
        assert!(validate_session_summary_query(&list).is_ok());
        for input in [
            SessionSummaryQuery::List {
                limit: 0,
                order: SessionSummaryOrder::OldestFirst,
                titled_only: false,
            },
            SessionSummaryQuery::Title {
                query: "x".repeat(4097),
                limit: 1,
            },
            SessionSummaryQuery::Title {
                query: "a\nb".into(),
                limit: 1,
            },
            SessionSummaryQuery::Ids {
                ids: vec!["a".into(); 101],
            },
            SessionSummaryQuery::Ids {
                ids: vec!["../a".into()],
            },
        ] {
            assert!(validate_session_summary_query(&input).is_err());
        }
        assert!(validate_session_summary_query(&SessionSummaryQuery::Ids { ids: vec![] }).is_ok());
        assert!(
            validate_session_summary_query(&SessionSummaryQuery::Title {
                query: "é".repeat(2048),
                limit: 1
            })
            .is_ok()
        );
        assert!(
            serde_json::from_value::<SessionSummaryQuery>(
                serde_json::json!({"kind":"list","limit":1,"order":"newest_first","path":"private"})
            )
            .is_err()
        );
    }
}
