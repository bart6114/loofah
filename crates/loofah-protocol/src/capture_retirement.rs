use serde::{Deserialize, Serialize};

use crate::{
    CaptureFormat, CaptureIdentity, CaptureProcessingRequest, CapturePublication, CaptureSeal,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureRetirementRequest {
    pub cleanup_request_id: String,
    pub identity: CaptureIdentity,
    pub format: CaptureFormat,
    pub source: CaptureSeal,
    pub publication: CapturePublication,
    pub processing: Option<CaptureProcessingRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureRetirementState {
    Declared,
    Pending,
    Retired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureRetirementSnapshot {
    pub request: CaptureRetirementRequest,
    pub state: CaptureRetirementState,
    pub remaining_chunks: u64,
}

pub fn validate_capture_retirement_request(
    input: &CaptureRetirementRequest,
) -> Result<(), &'static str> {
    if !canonical_uuid(&input.cleanup_request_id)
        || crate::validate_capture_format(&input.format).is_err()
        || input.source.capture_fingerprint
            != crate::capture_fingerprint(&input.identity, &input.format)
        || input.source != input.publication.source
        || input.source.chunk_count == 0
        || input.publication.canonical_audio_bytes == 0
        || input.publication.canonical_audio_sha256.len() != 64
        || !input
            .publication
            .canonical_audio_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || input.processing.as_ref().is_some_and(|request| {
            request.capture_fingerprint != input.source.capture_fingerprint
                || crate::validate_capture_processing_request(request).is_err()
        })
    {
        return Err("Invalid capture retirement identity or publication");
    }
    Ok(())
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value != "00000000-0000-0000-0000-000000000000"
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
            }
        })
}
