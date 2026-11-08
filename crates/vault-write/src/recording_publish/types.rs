use crate::StoreError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingExpectation {
    Absent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordingPublicationRequest {
    pub vault_id: String,
    pub namespace: String,
    pub request_id: uuid::Uuid,
    pub session_id: String,
    pub source_binding: String,
    pub expected: RecordingExpectation,
    pub artifact_sha256: String,
    pub artifact_size_bytes: u64,
    pub layout: hypr_vault_read::AudioLayout,
    pub first_audio_observed_at_ms: u64,
    pub capture_ended_at_ms: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordingPublication {
    Published {
        filename: String,
        sha256: String,
        size_bytes: u64,
    },
    Changed,
    Busy,
    Uncertain,
}

impl RecordingPublicationRequest {
    pub(super) fn validate(&self) -> Result<(), StoreError> {
        crate::validate_session_id(&self.session_id)?;
        if [
            (&self.vault_id, 512),
            (&self.namespace, 512),
            (&self.source_binding, 8192),
        ]
        .into_iter()
        .any(|(value, limit)| {
            value.is_empty() || value.len() > limit || value.chars().any(char::is_control)
        }) || !super::io::valid_hash(&self.artifact_sha256)
            || self.artifact_size_bytes == 0
            || self.artifact_size_bytes > u32::MAX as u64
            || self.request_id.is_nil()
        {
            return Err(StoreError::Serialize(
                "Invalid recording publication request".into(),
            ));
        }
        super::io::timestamp(self.first_audio_observed_at_ms)?;
        if let Some(end) = self.capture_ended_at_ms {
            super::io::timestamp(end)?;
            if end < self.first_audio_observed_at_ms {
                return Err(StoreError::Serialize(
                    "Capture end precedes first audio observation".into(),
                ));
            }
        }
        Ok(())
    }
    pub(super) fn published(&self) -> RecordingPublication {
        RecordingPublication::Published {
            filename: "audio.wav".into(),
            sha256: self.artifact_sha256.clone(),
            size_bytes: self.artifact_size_bytes,
        }
    }
}
