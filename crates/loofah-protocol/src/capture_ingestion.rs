use serde::{Deserialize, Serialize};

use super::{
    CaptureFormat, CaptureIdentity, CapturePrefixReceipt, CapturePublication, CaptureSeal,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureIngestRequest {
    pub identity: CaptureIdentity,
    pub format: CaptureFormat,
    pub first_admitted_audio_observed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureIngestStatus {
    pub identity: CaptureIdentity,
    pub format: CaptureFormat,
    pub prefix: CapturePrefixReceipt,
    pub seal: Option<CaptureSeal>,
    pub first_admitted_audio_observed_at_ms: u64,
    pub session_id: Option<String>,
    pub publication: Option<CapturePublication>,
}
