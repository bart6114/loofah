use std::{fs::File, path::Path, sync::Arc};

use serde::{Deserialize, Serialize};

use crate::{SessionStore, StoreError};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppendIdentity {
    pub server: String,
    pub vault_id: String,
    pub member_id: String,
    pub capture_id: uuid::Uuid,
    pub request_id: uuid::Uuid,
    pub admission_id: uuid::Uuid,
}

impl AppendIdentity {
    pub(super) fn validate(&self) -> Result<(), StoreError> {
        if [&self.server, &self.vault_id, &self.member_id]
            .into_iter()
            .any(|value| {
                value.is_empty() || value.len() > 2048 || value.chars().any(char::is_control)
            })
            || [self.capture_id, self.request_id, self.admission_id]
                .into_iter()
                .any(|id| id.is_nil())
        {
            return Err(super::io::invalid("Invalid append identity"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingAppendArtifact {
    pub sha256: String,
    pub size_bytes: u64,
    pub frames: u64,
    pub layout: hypr_vault_read::AudioLayout,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordingAppendOutcome {
    Published {
        sha256: String,
        size_bytes: u64,
        frames: u64,
        transcript_revision: String,
        transcript_present: bool,
    },
    AlreadyCommitted {
        sha256: String,
        size_bytes: u64,
        frames: u64,
        transcript_revision: String,
        transcript_present: bool,
    },
    Changed,
    Busy,
    RequiresReview,
}

#[derive(Clone)]
pub struct AppendSource(pub(super) Arc<super::source::Source>);
impl AppendSource {
    pub fn recording_file(&self) -> &File {
        &self.0.recording
    }
    pub fn recording_selection(&self) -> &str {
        &self.0.receipt.selection
    }
    pub fn recording_sha256(&self) -> &str {
        &self.0.receipt.recording_sha256
    }
    pub fn transcript_revision(&self) -> &str {
        &self.0.receipt.transcript_revision
    }
    pub fn raw_transcript(&self) -> Option<&[u8]> {
        self.0.transcript.as_deref()
    }
    pub fn audio(&self) -> hypr_vault_read::SessionAudio {
        self.0.receipt.audio
    }
    pub fn directory(&self) -> &Path {
        &self.0.directory
    }
    pub async fn recheck(&self) -> Result<bool, StoreError> {
        super::source::recheck(self.clone()).await
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SourceReceipt {
    pub schema_version: u32,
    pub identity: AppendIdentity,
    pub session_id: String,
    pub selection: String,
    pub recording_sha256: String,
    pub recording_bytes: u64,
    pub transcript_revision: String,
    pub transcript_present: bool,
    pub audio: hypr_vault_read::SessionAudio,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResultReceipt {
    pub schema_version: u32,
    pub source: SourceReceipt,
    pub artifact: RecordingAppendArtifact,
    pub transcript_revision: String,
    pub transcript_present: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Intent {
    pub schema_version: u32,
    pub result: ResultReceipt,
    pub state: IntentState,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum IntentState {
    Pending,
    Committed,
    Review,
}

#[derive(Clone)]
pub struct PendingRecordingAppend {
    pub(super) store: SessionStore,
    pub(super) directory: std::path::PathBuf,
    pub(super) intent: Intent,
}
impl PendingRecordingAppend {
    pub fn identity(&self) -> &AppendIdentity {
        &self.intent.result.source.identity
    }
    pub fn session_id(&self) -> &str {
        &self.intent.result.source.session_id
    }
    pub fn requires_review(&self) -> bool {
        self.intent.state == IntentState::Review
    }
}
