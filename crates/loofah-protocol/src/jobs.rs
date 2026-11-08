use serde::{Deserialize, Serialize};

use crate::DocumentTarget;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TitlePolicy {
    #[default]
    Preserve,
    GenerateIfEmpty,
}

impl TitlePolicy {
    fn is_preserve(&self) -> bool {
        matches!(self, Self::Preserve)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SummaryJobRequest {
    pub session_id: String,
    pub target: DocumentTarget,
    pub expected_revision: String,
    // Specta rc.22 requires default last to export this custom-skipped field as optional.
    #[serde(skip_serializing_if = "TitlePolicy::is_preserve", default)]
    pub title_policy: TitlePolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingJobState {
    Queued,
    Running,
    Cancelling,
    Cancelled,
    Succeeded,
    Failed,
    Interrupted,
    Conflict,
}

impl ProcessingJobState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running | Self::Cancelling)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingPhase {
    Preparing,
    Generating,
    Applying,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingJobFailure {
    AccessDenied,
    SourceUnavailable,
    ResourceLimit,
    ProviderUnavailable,
    ProviderFailed,
    InvalidOutput,
    Storage,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProcessingJobSnapshot {
    pub id: String,
    pub session_id: String,
    pub target: DocumentTarget,
    pub state: ProcessingJobState,
    pub phase: Option<ProcessingPhase>,
    pub failure: Option<ProcessingJobFailure>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(default, deny_unknown_fields)]
pub struct ListProcessingJobsInput {
    pub limit: Option<u32>,
    pub after: Option<String>,
    pub session_id: Option<String>,
    pub active_only: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProcessingJobPage {
    pub items: Vec<ProcessingJobSnapshot>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ProcessingTagSuggestion {
    pub name: String,
    pub confidence: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum ProcessingTitleStatus {
    Generating,
    Applying,
    Applied,
    Empty,
    AlreadyNamed,
    Unsupported,
    Failed,
    Cancelled,
    Conflict,
    Interrupted,
    Unconfirmed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct ProcessingTitleResult {
    pub status: ProcessingTitleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<ProcessingJobFailure>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ProcessingJobResult {
    pub vault_id: String,
    pub job_id: String,
    pub session_id: String,
    pub target: DocumentTarget,
    pub markdown: String,
    #[serde(default)]
    pub suggested_tags: Option<Vec<ProcessingTagSuggestion>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<ProcessingTitleResult>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_and_explicit_preserve_requests_keep_the_exact_original_bytes() {
        let old =
            br#"{"session_id":"note","target":{"kind":"summary"},"expected_revision":"revision"}"#;
        let request: SummaryJobRequest = serde_json::from_slice(old).unwrap();
        assert_eq!(request.title_policy, TitlePolicy::Preserve);
        assert_eq!(serde_json::to_vec(&request).unwrap(), old);
        let mut types = specta::TypeCollection::default();
        types.register::<SummaryJobRequest>();
        let exported = specta_typescript::Typescript::default()
            .export(&types)
            .unwrap();
        assert!(exported.contains("title_policy?: TitlePolicy"));
        let explicit = br#"{"session_id":"note","target":{"kind":"summary"},"expected_revision":"revision","title_policy":"preserve"}"#;
        let request: SummaryJobRequest = serde_json::from_slice(explicit).unwrap();
        assert_eq!(serde_json::to_vec(&request).unwrap(), old);
        let mut opted_in = request.clone();
        opted_in.title_policy = TitlePolicy::GenerateIfEmpty;
        assert_ne!(opted_in, request);
        let bytes = serde_json::to_vec(&opted_in).unwrap();
        assert_eq!(
            serde_json::from_slice::<SummaryJobRequest>(&bytes).unwrap(),
            opted_in
        );
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["title_policy"],
            "generate_if_empty"
        );
    }

    #[test]
    fn unknown_or_duplicate_title_policies_are_rejected() {
        for policy in [r#""overwrite""#, "null", "true"] {
            let input = format!(
                r#"{{"session_id":"note","target":{{"kind":"summary"}},"expected_revision":"revision","title_policy":{policy}}}"#
            );
            assert!(serde_json::from_str::<SummaryJobRequest>(&input).is_err());
        }
        assert!(serde_json::from_str::<SummaryJobRequest>(r#"{"session_id":"note","target":{"kind":"summary"},"expected_revision":"revision","title_policy":"preserve","title_policy":"generate_if_empty"}"#).is_err());
    }

    #[test]
    fn optional_title_results_preserve_old_response_bytes_and_accept_additive_fields() {
        let old = br##"{"vault_id":"vault","job_id":"job","session_id":"note","target":{"kind":"summary"},"markdown":"# Saved body","suggested_tags":null}"##;
        let mut result: ProcessingJobResult = serde_json::from_slice(old).unwrap();
        assert!(result.title.is_none());
        assert_eq!(serde_json::to_vec(&result).unwrap(), old);
        result.title = Some(ProcessingTitleResult {
            status: ProcessingTitleStatus::Conflict,
            candidate: Some("Retained title".into()),
            failure: None,
        });
        let mut value = serde_json::to_value(&result).unwrap();
        value["future_field"] = serde_json::json!(true);
        value["title"]["future_detail"] = serde_json::json!(42);
        assert_eq!(
            serde_json::from_value::<ProcessingJobResult>(value).unwrap(),
            result
        );
    }

    #[test]
    fn job_response_fields_are_additive_while_requests_reject_unknown_inputs() {
        let snapshot = serde_json::json!({"id":"job","session_id":"note","target":{"kind":"summary"},"state":"queued","phase":null,"failure":null,"created_at":"time","updated_at":"time","future_optional_field":42});
        assert!(serde_json::from_value::<ProcessingJobSnapshot>(snapshot).is_ok());
        let request = serde_json::json!({"session_id":"note","target":{"kind":"summary"},"expected_revision":"revision","api_key":"must-not-be-accepted"});
        assert!(serde_json::from_value::<SummaryJobRequest>(request).is_err());
        assert!(
            serde_json::from_value::<ListProcessingJobsInput>(serde_json::json!({"owner":"other"}))
                .is_err()
        );
    }
}
