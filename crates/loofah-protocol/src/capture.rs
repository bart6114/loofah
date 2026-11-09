#[path = "capture_ingestion.rs"]
mod ingestion;
pub use ingestion::{CaptureIngestRequest, CaptureIngestStatus};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureScope {
    pub server: String,
    pub member_id: String,
    pub vault_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureTarget {
    ExistingSession {
        session_id: String,
        expected_audio_sha256: Option<String>,
    },
    AppendSession {
        session_id: String,
        admission: crate::CaptureAppendAdmission,
    },
    PendingSession {
        create_request_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureIdentity {
    pub scope: CaptureScope,
    pub capture_id: String,
    pub request_id: String,
    pub target: CaptureTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureChannel {
    Microphone,
    SystemAudio,
    Mixed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureEncoding {
    PcmF32Le,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureSeal {
    pub capture_fingerprint: String,
    pub chunk_count: u64,
    pub frames: u64,
    pub pcm_bytes: u64,
    pub chain_digest: String,
    pub source_sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CapturePublication {
    pub source: CaptureSeal,
    pub session_id: String,
    pub canonical_audio_sha256: String,
    pub canonical_audio_bytes: u64,
}

pub fn validate_capture_format(format: &CaptureFormat) -> Result<(), &'static str> {
    if format.sample_rate != 16_000
        || format.channels.is_empty()
        || format.channels.len() > 2
        || format.channels.len() == 2
            && format.channels != [CaptureChannel::Microphone, CaptureChannel::SystemAudio]
    {
        return Err(
            "Capture requires 16 kHz PCM f32 little-endian with an explicit mono or microphone/system channel map",
        );
    }
    Ok(())
}

pub fn capture_fingerprint(identity: &CaptureIdentity, format: &CaptureFormat) -> String {
    hash(
        serde_json::to_vec(&("loofah-capture-v1", identity, format))
            .expect("capture identity serializes"),
    )
}

pub fn capture_chain_seed(fingerprint: &str) -> String {
    hash(format!("loofah-capture-chain-v1:{fingerprint}"))
}

pub fn capture_chunk_digest(descriptor: &CaptureChunkDescriptor) -> String {
    hash(
        serde_json::to_vec(&(
            "loofah-capture-chunk-v1",
            &descriptor.capture_fingerprint,
            descriptor.sequence,
            descriptor.first_frame,
            descriptor.pcm_offset,
            descriptor.frames,
            descriptor.bytes,
            &descriptor.sha256,
            &descriptor.previous_digest,
        ))
        .expect("capture descriptor serializes"),
    )
}

fn hash(bytes: impl AsRef<[u8]>) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests;
