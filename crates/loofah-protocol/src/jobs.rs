use serde::{Deserialize, Serialize};

use crate::DocumentTarget;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct SummaryJobRequest {
    pub session_id: String,
    pub target: DocumentTarget,
    pub expected_revision: String,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct ProcessingJobResult {
    pub vault_id: String,
    pub job_id: String,
    pub session_id: String,
    pub target: DocumentTarget,
    pub markdown: String,
    #[serde(default)]
    pub suggested_tags: Option<Vec<ProcessingTagSuggestion>>,
}

#[cfg(test)]
mod tests {
    use super::*;

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
