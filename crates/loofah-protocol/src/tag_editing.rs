pub use hypr_vault_read::normalize_tag_name;
use serde::{Deserialize, Serialize};

pub const TAG_QUERY_BYTES: usize = 4096;
pub const TAG_NAME_BYTES: usize = 4096;
pub const TAG_EXCLUSIONS_LIMIT: usize = 256;
pub const TAG_SUGGESTIONS_LIMIT: u32 = 200;
pub const TAG_SUGGESTIONS_TEXT_LIMIT: usize = 64 * 1024;
pub const TAG_SUGGESTIONS_RESPONSE_LIMIT: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TagSuggestionsRequest {
    pub query: String,
    pub excluded: Vec<String>,
    pub limit: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct EnsureTagRequest {
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TagSuggestionActionRequest {
    pub name: String,
    pub expected_revision: String,
}

pub fn validate_tag_suggestions_request(input: &TagSuggestionsRequest) -> Result<(), &'static str> {
    if input.query.len() > TAG_QUERY_BYTES
        || input.excluded.len() > TAG_EXCLUSIONS_LIMIT
        || !(1..=TAG_SUGGESTIONS_LIMIT).contains(&input.limit)
    {
        return Err("Invalid tag suggestion query");
    }
    Ok(())
}

pub fn validate_tag_name(name: &str) -> Result<(), &'static str> {
    if name.len() > TAG_NAME_BYTES {
        return Err("Tag name exceeds its request size limit");
    }
    if normalize_tag_name(name).is_none() {
        return Err("Tag name cannot be empty");
    }
    Ok(())
}

pub fn validate_tag_suggestion_action(
    input: &TagSuggestionActionRequest,
) -> Result<(), &'static str> {
    validate_tag_name(&input.name)?;
    if input.expected_revision.is_empty() || input.expected_revision.len() > 128 {
        return Err("Expected metadata revision is required");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn strict_contracts_preserve_raw_queries_and_bound_requests_without_ai_policy() {
        let input = TagSuggestionsRequest {
            query: "\u{feff}#\u{85}".into(),
            excluded: vec!["x".repeat(TAG_QUERY_BYTES + 1)],
            limit: 8,
        };
        assert!(validate_tag_suggestions_request(&input).is_ok());
        for input in [
            TagSuggestionsRequest {
                query: "x".repeat(TAG_QUERY_BYTES + 1),
                ..input.clone()
            },
            TagSuggestionsRequest {
                excluded: vec!["x".into(); TAG_EXCLUSIONS_LIMIT + 1],
                ..input.clone()
            },
            TagSuggestionsRequest {
                limit: 0,
                ..input.clone()
            },
            TagSuggestionsRequest {
                limit: 201,
                ..input.clone()
            },
        ] {
            assert!(validate_tag_suggestions_request(&input).is_err());
        }
        assert!(validate_tag_name(" #IMPORT/audio ").is_ok());
        assert!(validate_tag_name(" # ").is_err());
        assert!(validate_tag_name(&"é".repeat(TAG_NAME_BYTES / 2)).is_ok());
        assert!(validate_tag_name(&"é".repeat(TAG_NAME_BYTES / 2 + 1)).is_err());
        assert!(
            validate_tag_suggestion_action(&TagSuggestionActionRequest {
                name: "tag".into(),
                expected_revision: "".into()
            })
            .is_err()
        );
        assert!(
            serde_json::from_value::<TagSuggestionsRequest>(
                json!({"query":"","excluded":[],"limit":8,"path":"/other"})
            )
            .is_err()
        );
        assert!(
            serde_json::from_value::<EnsureTagRequest>(json!({"name":"tag","vault":"other"}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<TagSuggestionActionRequest>(
                json!({"name":"tag","expected_revision":"revision","force":true})
            )
            .is_err()
        );
    }
}
