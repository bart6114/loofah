use serde::{Deserialize, Serialize};

use crate::{DocumentTarget, TitlePolicy, TranscriptionPublication};

pub const CAPTURE_PROCESSING_RESPONSE_LIMIT: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureProcessingSummary {
    pub target: DocumentTarget,
    #[serde(default)]
    pub title_policy: TitlePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureProcessingRequest {
    pub capture_fingerprint: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub summary: Option<CaptureProcessingSummary>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureProcessingState {
    Waiting,
    Live,
    WaitingForPublication,
    Repairing,
    Summarizing,
    Succeeded,
    Empty,
    Cancelled,
    Conflict,
    Interrupted,
    Uncertain,
    Failed,
}

impl CaptureProcessingState {
    pub fn is_terminal(self) -> bool {
        !matches!(
            self,
            Self::Waiting
                | Self::Live
                | Self::WaitingForPublication
                | Self::Repairing
                | Self::Summarizing
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum CaptureProcessingFailure {
    AccessDenied,
    ModelUnavailable,
    ModelChanged,
    SourceChanged,
    SourceUnavailable,
    ChildFailed,
    Storage,
    Interrupted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureProcessingSnapshot {
    pub capture_id: String,
    pub capture_fingerprint: String,
    pub intent_id: String,
    pub session_id: Option<String>,
    pub model: String,
    pub request: CaptureProcessingRequest,
    pub state: CaptureProcessingState,
    pub live_generation_id: Option<String>,
    pub repair_job_id: Option<String>,
    pub summary_job_id: Option<String>,
    pub transcript_publication: Option<TranscriptionPublication>,
    pub failure: Option<CaptureProcessingFailure>,
    pub created_at: String,
    pub updated_at: String,
}

pub fn validate_capture_processing_request(
    value: &CaptureProcessingRequest,
) -> Result<(), &'static str> {
    crate::validate_capture_live_request(&crate::CaptureLiveRequest {
        capture_fingerprint: value.capture_fingerprint.clone(),
        expected_transcript: crate::AbsentTranscriptExpectation::Absent,
        model: value
            .model
            .clone()
            .unwrap_or_else(|| "operator-default".into()),
        languages: value.languages.clone(),
        keywords: value.keywords.clone(),
    })?;
    if let Some(CaptureProcessingSummary {
        target: DocumentTarget::TemplateOutput { document_id },
        ..
    }) = &value.summary
    {
        crate::validate_session_id(document_id).map_err(|_| "Invalid summary output identifier")?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct CaptureProcessingOptions {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub summary: Option<crate::CaptureProcessingSummary>,
}

impl CaptureProcessingOptions {
    pub fn request(&self, capture_fingerprint: String) -> crate::CaptureProcessingRequest {
        crate::CaptureProcessingRequest {
            capture_fingerprint,
            model: self.model.clone(),
            languages: self.languages.clone(),
            keywords: self.keywords.clone(),
            summary: self.summary.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn defaults_are_explicit_host_resolution_and_no_summary() {
        let request: CaptureProcessingRequest =
            serde_json::from_value(serde_json::json!({"capture_fingerprint":"a".repeat(64)}))
                .unwrap();
        assert_eq!(request.model, None);
        assert_eq!(request.summary, None);
        assert!(request.languages.is_empty());
        assert!(validate_capture_processing_request(&request).is_ok());
        assert_eq!(
            serde_json::from_value::<CaptureProcessingRequest>(
                serde_json::to_value(&request).unwrap()
            )
            .unwrap(),
            request
        );
    }
    #[test]
    fn intent_never_accepts_client_paths_principals_or_provider_credentials() {
        for field in ["model_path", "user_id", "member_id", "api_key", "memo_md"] {
            let mut request = serde_json::json!({"capture_fingerprint":"a".repeat(64)});
            request[field] = serde_json::json!("untrusted");
            assert!(serde_json::from_value::<CaptureProcessingRequest>(request).is_err());
        }
    }
    #[test]
    fn models_options_and_summary_targets_reuse_existing_portable_limits() {
        let mut request:CaptureProcessingRequest=serde_json::from_value(serde_json::json!({"capture_fingerprint":"a".repeat(64),"model":"operator-base","languages":["nl"],"summary":{"target":{"kind":"summary"},"title_policy":"generate_if_empty"}})).unwrap();
        assert!(validate_capture_processing_request(&request).is_ok());
        for model in ["", " alias", "x\n", &"x".repeat(513)] {
            request.model = Some(model.into());
            assert!(validate_capture_processing_request(&request).is_err());
        }
        request.model = None;
        request.keywords = vec!["x".repeat(513)];
        assert!(validate_capture_processing_request(&request).is_err());
        request.keywords.clear();
        request.summary.as_mut().unwrap().target = DocumentTarget::TemplateOutput {
            document_id: "CON".into(),
        };
        assert!(validate_capture_processing_request(&request).is_err());
    }
    #[test]
    fn unknown_state_stays_terminal_and_does_not_claim_safe_cleanup() {
        assert!(CaptureProcessingState::Uncertain.is_terminal());
        assert!(!CaptureProcessingState::Repairing.is_terminal());
        assert_ne!(
            CaptureProcessingState::Uncertain,
            CaptureProcessingState::Succeeded
        );
    }
}
