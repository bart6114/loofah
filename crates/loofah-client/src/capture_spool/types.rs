use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureScope {
    pub server: String,
    pub member_id: String,
    pub vault_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureTarget {
    ExistingSession {
        session_id: String,
        expected_audio_sha256: Option<String>,
    },
    PendingSession {
        create_request_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureIdentity {
    pub scope: CaptureScope,
    pub capture_id: String,
    pub request_id: String,
    pub target: CaptureTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureChannel {
    Microphone,
    SystemAudio,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureEncoding {
    PcmF32Le,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureFormat {
    pub encoding: CaptureEncoding,
    pub sample_rate: u32,
    pub channels: Vec<CaptureChannel>,
}

impl CaptureFormat {
    pub fn stereo() -> Self {
        Self {
            encoding: CaptureEncoding::PcmF32Le,
            sample_rate: 16_000,
            channels: vec![CaptureChannel::Microphone, CaptureChannel::SystemAudio],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureChunkDescriptor {
    pub capture_fingerprint: String,
    pub sequence: u64,
    pub first_frame: u64,
    pub pcm_offset: u64,
    pub frames: u32,
    pub bytes: u32,
    pub sha256: String,
    pub previous_digest: String,
    pub digest: String,
}

pub struct CaptureChunk {
    pub descriptor: CaptureChunkDescriptor,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSeal {
    pub capture_fingerprint: String,
    pub chunk_count: u64,
    pub frames: u64,
    pub pcm_bytes: u64,
    pub chain_digest: String,
    pub source_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePrefixReceipt {
    pub capture_fingerprint: String,
    pub chunk_count: u64,
    pub frames: u64,
    pub pcm_bytes: u64,
    pub chain_digest: String,
}

/// An authenticated verifier must compare the finalized service receipt with an
/// independent canonical artifact read before supplying this observation.
/// Recording it never authorizes deletion of the device's audio.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturePublication {
    pub source: CaptureSeal,
    pub session_id: String,
    pub canonical_audio_sha256: String,
    pub canonical_audio_bytes: u64,
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
