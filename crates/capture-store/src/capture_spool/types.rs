use serde::{Deserialize, Serialize};

pub use hypr_loofah_protocol::{
    CaptureChannel, CaptureChunkDescriptor, CaptureEncoding, CaptureFormat, CaptureIdentity,
    CapturePrefixReceipt, CapturePublication, CaptureScope, CaptureSeal, CaptureTarget,
};

pub use hypr_loofah_protocol::CaptureProcessingOptions;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureProcessingDeclaration {
    pub request: hypr_loofah_protocol::CaptureProcessingRequest,
    pub cancel_requested: bool,
}

pub struct CaptureChunk {
    pub descriptor: CaptureChunkDescriptor,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSnapshot {
    pub identity: CaptureIdentity,
    pub format: CaptureFormat,
    pub capture_fingerprint: String,
    pub created_at_ms: u64,
    pub session_id: Option<String>,
    pub chunk_count: u64,
    pub frames: u64,
    pub pcm_bytes: u64,
    pub chain_digest: String,
    pub acknowledged: Option<CapturePrefixReceipt>,
    pub seal: Option<CaptureSeal>,
    pub publication: Option<CapturePublication>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retirement: Option<hypr_loofah_protocol::CaptureRetirementSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing: Option<CaptureProcessingDeclaration>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "observation"
    )]
    pub first_admitted_audio_observed_at_ms: Option<u64>,
}

fn observation<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<u64>, D::Error> {
    u64::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone)]
pub struct CaptureSpoolLimits {
    /// Serialized owned and retained files, including temporary-file headroom.
    pub max_bytes: u64,
    pub max_captures: usize,
    pub max_chunks: usize,
    pub max_chunk_bytes: usize,
    pub max_directory_entries: usize,
}

impl Default for CaptureSpoolLimits {
    fn default() -> Self {
        Self {
            max_bytes: 2 * 1024 * 1024 * 1024,
            max_captures: 64,
            max_chunks: 32_768,
            max_chunk_bytes: 1024 * 1024,
            max_directory_entries: 65_536,
        }
    }
}
