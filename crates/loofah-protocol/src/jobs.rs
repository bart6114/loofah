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
    ProviderUnavailable,
    ProviderFailed,
    InvalidOutput,
    Storage,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
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
