use serde::{Deserialize, Serialize};

use crate::{CaptureChannel, CaptureFormat, CaptureScope};

/// Persist this identity before dispatch. Retrying preparation must use every
/// original field, even when its first response was lost.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureAppendRequest {
    pub scope: CaptureScope,
    pub session_id: String,
    pub capture_id: String,
    pub request_id: String,
    pub admission_id: String,
    #[serde(default)]
    pub processing: Option<crate::CaptureProcessingOptions>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureAppendBaseline {
    pub recording_selection: String,
    pub recording_sha256: String,
    pub transcript_present: bool,
    pub transcript_revision: String,
    pub normalized_sha256: String,
    pub normalized_bytes: u64,
    pub frames: u64,
    pub format: CaptureFormat,
}

/// A declaration is not write authority. Core must resolve the exact durable
/// admission under its original lease and compare the entire baseline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureAppendAdmission {
    pub request: CaptureAppendRequest,
    pub baseline: CaptureAppendBaseline,
}

pub fn validate_capture_append_request(value: &CaptureAppendRequest) -> Result<(), &'static str> {
    let url = url::Url::parse(&value.scope.server).map_err(|_| "Invalid append server")?;
    crate::validate_endpoint(&url, true)?;
    if url.as_str() != value.scope.server
        || url.path() != "/"
        || url.query().is_some()
        || [&value.scope.member_id, &value.scope.vault_id]
            .into_iter()
            .any(|value| {
                value.is_empty() || value.len() > 512 || value.chars().any(char::is_control)
            })
    {
        return Err("Invalid append scope");
    }
    crate::validate_session_id(&value.session_id).map_err(|_| "Invalid append session")?;
    if [&value.capture_id, &value.request_id, &value.admission_id]
        .into_iter()
        .any(|value| !canonical_uuid(value))
    {
        return Err("Append requires original canonical capture, request and admission UUIDs");
    }
    if let Some(options) = &value.processing {
        crate::validate_capture_processing_request(&options.request("0".repeat(64)))?;
    }
    Ok(())
}

pub fn validate_capture_append_admission(
    value: &CaptureAppendAdmission,
) -> Result<(), &'static str> {
    validate_capture_append_request(&value.request)?;
    let baseline = &value.baseline;
    crate::validate_capture_format(&baseline.format)?;
    let channels = &baseline.format.channels;
    if channels != &[CaptureChannel::Mixed]
        && channels != &[CaptureChannel::Microphone, CaptureChannel::SystemAudio]
    {
        return Err("Append requires the original mixed or microphone/system layout");
    }
    if ![
        "audio.wav",
        "audio.mp3",
        "audio.ogg",
        "audio/audio.wav",
        "audio/audio.mp3",
        "audio/audio.ogg",
    ]
    .contains(&baseline.recording_selection.as_str())
        || [
            &baseline.recording_sha256,
            &baseline.transcript_revision,
            &baseline.normalized_sha256,
        ]
        .into_iter()
        .any(|value| !hash(value))
        || baseline.frames == 0
        || baseline.frames > u32::MAX as u64
        || baseline
            .frames
            .checked_mul(channels.len() as u64 * 4)
            .is_none_or(|pcm| {
                pcm >= baseline.normalized_bytes || baseline.normalized_bytes > u32::MAX as u64
            })
    {
        return Err("Invalid authoritative append baseline");
    }
    Ok(())
}

fn canonical_uuid(value: &str) -> bool {
    value.len() == 36
        && value != "00000000-0000-0000-0000-000000000000"
        && value.bytes().enumerate().all(|(index, value)| {
            if [8, 13, 18, 23].contains(&index) {
                value == b'-'
            } else {
                value.is_ascii_digit() || (b'a'..=b'f').contains(&value)
            }
        })
}

fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn admission() -> CaptureAppendAdmission {
        CaptureAppendAdmission {
            request: CaptureAppendRequest {
                scope: CaptureScope {
                    server: "https://example.test/".into(),
                    member_id: "member".into(),
                    vault_id: "vault".into(),
                },
                session_id: "note".into(),
                capture_id: "11111111-1111-4111-8111-111111111111".into(),
                request_id: "22222222-2222-4222-8222-222222222222".into(),
                admission_id: "33333333-3333-4333-8333-333333333333".into(),
                processing: None,
            },
            baseline: CaptureAppendBaseline {
                recording_selection: "audio/audio.mp3".into(),
                recording_sha256: "1".repeat(64),
                transcript_present: true,
                transcript_revision: "2".repeat(64),
                normalized_sha256: "3".repeat(64),
                normalized_bytes: 128_044,
                frames: 16_000,
                format: CaptureFormat::stereo(),
            },
        }
    }
    #[test]
    fn append_identity_cannot_reinterpret_a_legacy_reserved_receipt() {
        let admission = admission();
        validate_capture_append_admission(&admission).unwrap();
        let mut identity = crate::CaptureIdentity {
            scope: admission.request.scope.clone(),
            capture_id: admission.request.capture_id.clone(),
            request_id: admission.request.request_id.clone(),
            target: crate::CaptureTarget::ExistingSession {
                session_id: "note".into(),
                expected_audio_sha256: Some(admission.baseline.recording_sha256.clone()),
            },
        };
        let legacy = crate::capture_fingerprint(&identity, &admission.baseline.format);
        identity.target = crate::CaptureTarget::AppendSession {
            session_id: "note".into(),
            admission: admission.clone(),
        };
        let current = crate::capture_fingerprint(&identity, &admission.baseline.format);
        assert_ne!(legacy, current);
        let crate::CaptureTarget::AppendSession {
            admission: bound, ..
        } = &mut identity.target
        else {
            unreachable!()
        };
        bound.baseline.frames += 1;
        assert_ne!(
            current,
            crate::capture_fingerprint(&identity, &admission.baseline.format)
        );
    }
    #[test]
    fn rejects_ambiguous_layout_and_unbounded_or_traversing_baseline() {
        for field in 0..5 {
            let mut value = admission();
            match field {
                0 => value.baseline.format.channels = vec![CaptureChannel::Microphone],
                1 => value.baseline.frames = u64::MAX,
                2 => value.baseline.recording_selection = "../audio.wav".into(),
                3 => value.request.admission_id = "00000000-0000-0000-0000-000000000000".into(),
                _ => value.baseline.normalized_bytes = 1,
            }
            assert!(validate_capture_append_admission(&value).is_err());
        }
    }
}
