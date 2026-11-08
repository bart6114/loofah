use serde::{Deserialize, Serialize};

pub const TRANSCRIPTION_RESULT_LIMIT: usize = 64 * 1024 * 1024;
pub const TRANSCRIPTION_SNAPSHOT_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum AbsentTranscriptExpectation {
    Absent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionJobRequest {
    pub session_id: String,
    pub expected_recording_sha256: String,
    pub expected_transcript: AbsentTranscriptExpectation,
    pub model: String,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionJobState {
    Queued,
    Running,
    Cancelling,
    Cancelled,
    Succeeded,
    Failed,
    Interrupted,
    Conflict,
    Empty,
    Uncertain,
}

impl TranscriptionJobState {
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::Queued | Self::Running | Self::Cancelling)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionPhase {
    Preparing,
    Transcribing,
    Publishing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionFailure {
    AccessDenied,
    SourceUnavailable,
    ResourceLimit,
    EngineUnavailable,
    EngineFailed,
    InvalidOutput,
    Storage,
    Interrupted,
}

/// A durable historical decision, not confirmation that these bytes are still current.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionPublication {
    pub revision: String,
    pub transcript_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct TranscriptionJobSnapshot {
    pub id: String,
    pub session_id: String,
    pub state: TranscriptionJobState,
    pub phase: Option<TranscriptionPhase>,
    pub created_at: String,
    pub updated_at: String,
    pub result_available: bool,
    pub failure: Option<TranscriptionFailure>,
    pub publication: Option<TranscriptionPublication>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct ListTranscriptionJobsInput {
    pub limit: Option<u32>,
    pub after: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
pub struct TranscriptionJobPage {
    pub jobs: Vec<TranscriptionJobSnapshot>,
    pub next_cursor: Option<String>,
}

pub fn validate_transcription_job_request(
    value: &TranscriptionJobRequest,
) -> Result<(), &'static str> {
    crate::validate_session_id(&value.session_id).map_err(|_| "Invalid session identifier")?;
    if !is_transcription_sha256(&value.expected_recording_sha256) {
        return Err("A lowercase SHA-256 recording revision is required");
    }
    if !bounded_text(&value.model, 512)
        || value.languages.len() > 16
        || value
            .languages
            .iter()
            .any(|language| !bounded_text(language, 64))
        || value.keywords.len() > 64
        || value
            .keywords
            .iter()
            .any(|keyword| !bounded_text(keyword, 512))
        || value.keywords.iter().map(String::len).sum::<usize>() > 16 * 1024
    {
        return Err("Transcription options exceed their size limits");
    }
    Ok(())
}

pub fn is_transcription_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn bounded_text(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request() -> TranscriptionJobRequest {
        serde_json::from_value(json!({
            "session_id":"note", "expected_recording_sha256":"a".repeat(64),
            "expected_transcript":"absent", "model":"cpu-base"
        }))
        .unwrap()
    }

    #[test]
    fn initial_contract_is_absent_only_without_paths_or_client_metadata() {
        let valid = request();
        assert!(validate_transcription_job_request(&valid).is_ok());
        assert!(valid.languages.is_empty());
        for field in ["file_path", "base_url", "api_key", "user_id", "memo_md"] {
            let mut input = serde_json::to_value(&valid).unwrap();
            input[field] = json!("untrusted");
            assert!(serde_json::from_value::<TranscriptionJobRequest>(input).is_err());
        }
        let mut input = serde_json::to_value(&valid).unwrap();
        input["expected_transcript"] = json!("replace");
        assert!(serde_json::from_value::<TranscriptionJobRequest>(input).is_err());
    }

    #[test]
    fn options_and_recording_revisions_are_bounded_without_reinterpreting_strings() {
        let mut value = request();
        value.languages = vec!["en-GB".into(), "nl".into()];
        value.keywords = vec!["Loofah".into(), "café project".into()];
        assert!(validate_transcription_job_request(&value).is_ok());
        for model in ["", " cpu", "cpu\n", &"m".repeat(513)] {
            value.model = model.into();
            assert!(validate_transcription_job_request(&value).is_err());
        }
        value = request();
        value.expected_recording_sha256 = "A".repeat(64);
        assert!(validate_transcription_job_request(&value).is_err());
        value = request();
        value.languages = vec!["en".into(); 17];
        assert!(validate_transcription_job_request(&value).is_err());
        value = request();
        value.keywords = vec!["k".repeat(512); 33];
        assert!(validate_transcription_job_request(&value).is_err());
    }

    #[test]
    fn explicit_empty_options_and_omitted_options_have_identical_canonical_requests() {
        let value = request();
        let serialized = serde_json::to_string(&value).unwrap();
        assert_eq!(
            serde_json::from_str::<TranscriptionJobRequest>(&serialized).unwrap(),
            value
        );
        assert!(TranscriptionJobState::Uncertain.is_terminal());
        assert!(!TranscriptionJobState::Cancelling.is_terminal());
    }
}
