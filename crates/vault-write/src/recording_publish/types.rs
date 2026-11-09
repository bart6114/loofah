use crate::StoreError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordingExpectation {
    Absent,
    AbsentImport,
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
            || (self.expected == RecordingExpectation::Absent
                && self.artifact_size_bytes > u32::MAX as u64)
            || self.request_id.is_nil()
        {
            return Err(StoreError::Serialize(
                "Invalid recording publication request".into(),
            ));
        }
        if self.expected == RecordingExpectation::AbsentImport {
            if self.layout != hypr_vault_read::AudioLayout::Mixed
                || self.first_audio_observed_at_ms != 0
                || self.capture_ended_at_ms.is_some()
            {
                return Err(StoreError::Serialize(
                    "Imported recording cannot declare capture timing".into(),
                ));
            }
            return Ok(());
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
    pub(super) fn metadata_limit(&self, store: &crate::SessionStore) -> usize {
        if self.expected == RecordingExpectation::AbsentImport {
            store.content_limits.metadata_bytes
        } else {
            store.content_limits.metadata_bytes.min(8 * 1024 * 1024)
        }
    }
    pub(super) fn filename(&self) -> &'static str {
        match self.expected {
            RecordingExpectation::Absent => "audio.wav",
            RecordingExpectation::AbsentImport => "audio.mp3",
        }
    }
    pub(super) fn published(&self) -> RecordingPublication {
        RecordingPublication::Published {
            filename: self.filename().into(),
            sha256: self.artifact_sha256.clone(),
            size_bytes: self.artifact_size_bytes,
        }
    }
}
