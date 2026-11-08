use serde::{Deserialize, Serialize};

use crate::{AbsentTranscriptExpectation, CaptureChannel, TranscriptionPublication};

pub const CAPTURE_LIVE_CAPTION_LIMIT: usize = 64 * 1024;
pub const CAPTURE_LIVE_PAGE_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureLiveRequest {
    pub capture_fingerprint: String,
    pub expected_transcript: AbsentTranscriptExpectation,
    pub model: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureLiveState {
    Starting,
    Running,
    Finishing,
    Cancelling,
    AwaitingRecording,
    Publishing,
    Completed,
    Empty,
    Cancelled,
    Interrupted,
    RepairRequired,
    Conflict,
    Uncertain,
}

impl CaptureLiveState {
    pub fn is_terminal(self) -> bool {
        !matches!(
            self,
            Self::Starting
                | Self::Running
                | Self::Finishing
                | Self::Cancelling
                | Self::AwaitingRecording
                | Self::Publishing
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureLiveFailure {
    AccessDenied,
    Interrupted,
    EngineUnavailable,
    EngineFailed,
    ResourceLimit,
    InvalidOutput,
    SourceChanged,
    Backlog,
    Idle,
    Storage,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureLiveSnapshot {
    pub capture_id: String,
    pub capture_fingerprint: String,
    pub generation_id: String,
    pub session_id: String,
    pub state: CaptureLiveState,
    pub failure: Option<CaptureLiveFailure>,
    pub admitted_end_frame: u64,
    pub input_consumed_end_frame: u64,
    pub completed_through_frame: Option<u64>,
    pub next_caption_sequence: u64,
    pub result_available: bool,
    pub publication: Option<TranscriptionPublication>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureLiveCaption {
    pub generation_id: String,
    pub sequence: u64,
    pub physical_channel: u8,
    pub role: CaptureChannel,
    pub new_words: Vec<hypr_transcript::FinalizedWord>,
    #[serde(default)]
    pub partials: Vec<hypr_transcript::PartialWord>,
    pub replaced_ids: Vec<String>,
    pub languages: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureLiveCaptionPage {
    pub snapshot: CaptureLiveSnapshot,
    pub captions: Vec<CaptureLiveCaption>,
    pub next_cursor: Option<u64>,
}

pub fn validate_capture_live_request(value: &CaptureLiveRequest) -> Result<(), &'static str> {
    if !crate::is_transcription_sha256(&value.capture_fingerprint) {
        return Err("Invalid capture fingerprint");
    }
    crate::validate_transcription_job_request(&crate::TranscriptionJobRequest {
        session_id: "capture".into(),
        expected_recording_sha256: value.capture_fingerprint.clone(),
        expected_transcript: value.expected_transcript,
        model: value.model.clone(),
        languages: value.languages.clone(),
        keywords: value.keywords.clone(),
    })
}

pub fn validate_capture_live_caption(
    value: &CaptureLiveCaption,
    channels: &[CaptureChannel],
) -> Result<(), &'static str> {
    if value.new_words.len() > 2048
        || value.partials.len() > 2048
        || value.replaced_ids.len() > 2048
        || value.languages.len() > 16
        || value
            .languages
            .iter()
            .any(|s| s.len() > 64 || s.chars().any(char::is_control))
        || value.physical_channel as usize >= channels.len()
        || channels[value.physical_channel as usize] != value.role
        || value
            .replaced_ids
            .iter()
            .any(|id| id.is_empty() || id.len() > 256)
        || value.new_words.iter().any(|word| {
            word.id.is_empty()
                || word.id.len() > 256
                || word.text.len() > 16 * 1024
                || word.start_ms < 0
                || word.end_ms < word.start_ms
                || word.end_ms > (1_i64 << 53) - 1
                || !matches!(word.state, hypr_transcript::WordState::Final)
                || word.speaker_index.is_some_and(|speaker| speaker < 0)
                || (channels.len() == 1 && word.channel != 2)
                || (channels.len() == 2 && word.channel != i32::from(value.physical_channel))
        })
        || !(1..=2).contains(&channels.len())
        || value.partials.iter().any(|word| {
            word.text.len() > 16 * 1024
                || word.start_ms < 0
                || word.end_ms < word.start_ms
                || word.end_ms > (1_i64 << 53) - 1
                || word.speaker_index.is_some_and(|speaker| speaker < 0)
                || (channels.len() == 1 && word.channel != 2)
                || (channels.len() == 2 && ![0, 1].contains(&word.channel))
        })
    {
        return Err("Invalid live caption");
    }
    let mut ids = std::collections::HashSet::new();
    if value.new_words.iter().any(|word| !ids.insert(&word.id)) {
        return Err("Duplicate live word identity");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_request_is_bounded_and_has_no_client_path_or_principal() {
        let mut request = CaptureLiveRequest {
            capture_fingerprint: "a".repeat(64),
            expected_transcript: AbsentTranscriptExpectation::Absent,
            model: "cpu-base".into(),
            languages: vec!["en".into()],
            keywords: vec![],
        };
        assert!(validate_capture_live_request(&request).is_ok());
        for key in ["path", "member_id", "base_url", "api_key", "start_frame"] {
            let mut value = serde_json::to_value(&request).unwrap();
            value[key] = serde_json::json!("untrusted");
            assert!(serde_json::from_value::<CaptureLiveRequest>(value).is_err());
        }
        request.languages = vec!["en".into(); 17];
        assert!(validate_capture_live_request(&request).is_err());
        request.languages.clear();
        request.capture_fingerprint = "A".repeat(64);
        assert!(validate_capture_live_request(&request).is_err());
    }

    #[test]
    fn native_consumption_is_not_complete_coverage() {
        assert!(!CaptureLiveState::AwaitingRecording.is_terminal());
        assert!(!CaptureLiveState::Publishing.is_terminal());
        assert!(CaptureLiveState::RepairRequired.is_terminal());
        let value = serde_json::json!({
            "capture_id":"capture", "capture_fingerprint":"a".repeat(64),
            "generation_id":"generation", "session_id":"session", "state":"running",
            "failure":null, "admitted_end_frame":16000, "input_consumed_end_frame":16000,
            "completed_through_frame":null, "next_caption_sequence":0,
            "result_available":false, "publication":null
        });
        let snapshot: CaptureLiveSnapshot = serde_json::from_value(value).unwrap();
        assert_eq!(snapshot.completed_through_frame, None);
    }

    #[test]
    fn captions_reject_pending_words_wrong_roles_and_duplicate_ids() {
        let mut caption: CaptureLiveCaption = serde_json::from_value(serde_json::json!({
            "generation_id":"generation", "sequence":0,"physical_channel":0,"role":"microphone",
            "new_words":[{"id":"word","text":" hello","start_ms":0,"end_ms":1000,"channel":0,"state":"final"}],
            "replaced_ids":[],"languages":["en"]
        })).unwrap();
        assert!(
            validate_capture_live_caption(
                &caption,
                &[CaptureChannel::Microphone, CaptureChannel::SystemAudio]
            )
            .is_ok()
        );
        caption.new_words[0].state = hypr_transcript::WordState::Pending;
        assert!(
            validate_capture_live_caption(
                &caption,
                &[CaptureChannel::Microphone, CaptureChannel::SystemAudio]
            )
            .is_err()
        );
        caption.new_words[0].state = hypr_transcript::WordState::Final;
        caption.new_words.push(caption.new_words[0].clone());
        assert!(
            validate_capture_live_caption(
                &caption,
                &[CaptureChannel::Microphone, CaptureChannel::SystemAudio]
            )
            .is_err()
        );
        caption.new_words.pop();
        caption.role = CaptureChannel::SystemAudio;
        assert!(
            validate_capture_live_caption(
                &caption,
                &[CaptureChannel::Microphone, CaptureChannel::SystemAudio]
            )
            .is_err()
        );
    }
}
