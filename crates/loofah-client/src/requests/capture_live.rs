use super::*;
use crate::events::{validate_live_binding, validate_live_page, validate_live_snapshot};

pub(super) fn is_operation(operation: &RemoteOperation) -> bool {
    matches!(
        operation,
        RemoteOperation::StartCaptureLive { .. }
            | RemoteOperation::CaptureLiveStatus { .. }
            | RemoteOperation::CaptureLiveCaptions { .. }
            | RemoteOperation::CaptureLiveResult { .. }
            | RemoteOperation::CancelCaptureLive { .. }
    )
}
pub(super) fn is_write(operation: &RemoteOperation) -> bool {
    matches!(
        operation,
        RemoteOperation::StartCaptureLive { .. } | RemoteOperation::CancelCaptureLive { .. }
    )
}
pub(super) fn validate_operation(operation: &RemoteOperation, request_id: &str) -> Result<()> {
    let (vault, binding) = match operation {
        RemoteOperation::StartCaptureLive {
            vault_id,
            binding,
            input,
        } => {
            validate_capture_live_request(input).map_err(invalid)?;
            if binding.generation_id != request_id
                || binding.capture_fingerprint != input.capture_fingerprint
            {
                return Err(invalid(
                    "Live generation request identity does not match its binding",
                ));
            }
            (vault_id, binding)
        }
        RemoteOperation::CaptureLiveStatus { vault_id, binding }
        | RemoteOperation::CaptureLiveResult { vault_id, binding }
        | RemoteOperation::CancelCaptureLive { vault_id, binding } => (vault_id, binding),
        RemoteOperation::CaptureLiveCaptions {
            vault_id,
            binding,
            after,
            limit,
        } => {
            if !(1..=128).contains(limit)
                || after.is_some_and(|after| after > 9_007_199_254_740_991)
            {
                return Err(invalid("Invalid live caption page"));
            }
            (vault_id, binding)
        }
        _ => return Ok(()),
    };
    if !uuid::Uuid::parse_str(vault).is_ok_and(|id| !id.is_nil() && id.to_string() == *vault) {
        return Err(invalid("A canonical vault UUID is required"));
    }
    validate_live_binding(binding)
}
fn malformed() -> ApplicationError {
    failure(
        ErrorCode::IncompatibleVersion,
        "Invalid live capture result",
    )
}

pub(super) async fn decode(
    response: reqwest::Response,
    operation: &RemoteOperation,
) -> Result<RemoteReply> {
    match operation {
        RemoteOperation::CaptureLiveCaptions {
            binding,
            after,
            limit,
            ..
        } => {
            let page: CaptureLiveCaptionPage =
                json_body(response, 2 * CAPTURE_LIVE_PAGE_LIMIT).await?;
            validate_live_page(&page, binding, *after, *limit)?;
            Ok(RemoteReply::CaptureLiveCaptions(page))
        }
        RemoteOperation::CaptureLiveResult { binding, .. } => {
            let value: Option<TranscriptionJobResult> =
                json_body(response, TRANSCRIPTION_RESULT_RESPONSE_LIMIT).await?;
            if let Some(value) = &value {
                super::transcription::validate_result(value, &binding.generation_id)?;
                if value.session_id != binding.session_id {
                    return Err(malformed());
                }
            }
            Ok(RemoteReply::CaptureLiveResult(value))
        }
        RemoteOperation::StartCaptureLive { binding, .. }
        | RemoteOperation::CaptureLiveStatus { binding, .. }
        | RemoteOperation::CancelCaptureLive { binding, .. } => {
            let value: CaptureLiveSnapshot = json_body(response, 64 * 1024).await?;
            validate_live_snapshot(&value, binding)?;
            Ok(RemoteReply::CaptureLiveSnapshot(value))
        }
        _ => Err(malformed()),
    }
}
