#![forbid(unsafe_code)]

use hypr_loofah_protocol::{ApplicationError, ErrorCode};

pub mod capture_spool;
pub mod wave;
pub use capture_spool::*;

pub type Result<T> = std::result::Result<T, ApplicationError>;

fn failure(code: ErrorCode, message: &str) -> ApplicationError {
    ApplicationError {
        code,
        message: message.into(),
        request_id: None,
        retryable: false,
        current_revision: None,
        mutation_outcome: None,
    }
}

fn validate_endpoint(url: &url::Url, allow_insecure_loopback: bool) -> Result<()> {
    hypr_loofah_protocol::validate_endpoint(url, allow_insecure_loopback)
        .map_err(|message| failure(ErrorCode::Validation, message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn endpoint_rejection_preserves_error_envelope_without_a_mutation_outcome() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap().join("capture");
        let spool = CaptureSpool::open(root, CaptureSpoolLimits::default())
            .await
            .unwrap();
        let error = spool
            .inventory(CaptureScope {
                server: "http://localhost/".into(),
                member_id: "member".into(),
                vault_id: "vault".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(
            error.message,
            "Server and identity endpoints must use HTTPS"
        );
        assert_eq!(error.mutation_outcome, None);
        assert!(!error.retryable);
        assert_eq!(error.request_id, None);
        assert_eq!(error.current_revision, None);
        let error = spool
            .inventory(CaptureScope {
                server: "https://capture.example/path?query=1".into(),
                member_id: "member".into(),
                vault_id: "vault".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(
            error.message,
            "Capture scope requires the canonical server origin"
        );
        assert_eq!(
            error.mutation_outcome,
            Some(hypr_loofah_protocol::MutationOutcome::NotCommitted)
        );
    }
}
