use super::*;

pub(super) fn is_operation(operation: &RemoteOperation) -> bool {
    matches!(
        operation,
        RemoteOperation::DeclareCaptureProcessing { .. }
            | RemoteOperation::CaptureProcessingStatus { .. }
            | RemoteOperation::CancelCaptureProcessing { .. }
    )
}

pub(super) fn is_write(operation: &RemoteOperation) -> bool {
    matches!(
        operation,
        RemoteOperation::DeclareCaptureProcessing { .. }
            | RemoteOperation::CancelCaptureProcessing { .. }
    )
}

fn binding(operation: &RemoteOperation) -> Option<(&CaptureIdentity, &CaptureProcessingRequest)> {
    match operation {
        RemoteOperation::DeclareCaptureProcessing { identity, input }
        | RemoteOperation::CaptureProcessingStatus { identity, input }
        | RemoteOperation::CancelCaptureProcessing { identity, input } => Some((identity, input)),
        _ => None,
    }
}

fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| !id.is_nil() && id.to_string() == value)
}

pub(super) fn validate_operation(
    client: &RemoteClient,
    operation: &RemoteOperation,
    request_id: &str,
) -> Result<()> {
    let Some((identity, input)) = binding(operation) else {
        return Ok(());
    };
    if identity.scope.server != client.address.as_str()
        || !uuid(&identity.scope.vault_id)
        || !uuid(&identity.scope.member_id)
        || !uuid(&identity.capture_id)
        || !uuid(&identity.request_id)
        || (matches!(operation, RemoteOperation::DeclareCaptureProcessing { .. })
            && identity.request_id != request_id)
    {
        return Err(invalid(
            "Capture processing belongs to another connection or request",
        ));
    }
    match &identity.target {
        CaptureTarget::ExistingSession {
            session_id,
            expected_audio_sha256,
        } => {
            validate_session_id(session_id).map_err(|_| invalid("Invalid capture session"))?;
            if expected_audio_sha256.is_some() {
                return Err(invalid(
                    "Capture processing does not support appending recordings",
                ));
            }
        }
        CaptureTarget::PendingSession { create_request_id } => {
            if !uuid(create_request_id) {
                return Err(invalid("A canonical creation UUID is required"));
            }
        }
    }
    validate_capture_processing_request(input).map_err(invalid)
}

fn malformed() -> ApplicationError {
    failure(
        ErrorCode::IncompatibleVersion,
        "Invalid capture processing response",
    )
}

fn text(value: &str, limit: usize) -> bool {
    !value.is_empty()
        && value.len() <= limit
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn validate_snapshot(
    value: &CaptureProcessingSnapshot,
    identity: &CaptureIdentity,
    input: &CaptureProcessingRequest,
) -> Result<()> {
    if value.capture_id != identity.capture_id
        || value.capture_fingerprint != input.capture_fingerprint
        || value.intent_id != identity.request_id
        || value.request != *input
        || !text(&value.model, 512)
        || input.model.as_ref().is_some_and(|model| model != &value.model)
        || !text(&value.created_at, 128)
        || !text(&value.updated_at, 128)
        || value.session_id.as_deref().is_some_and(|id| {
            validate_session_id(id).is_err()
                || matches!(&identity.target, CaptureTarget::ExistingSession { session_id, .. } if session_id != id)
        })
        || value.live_generation_id.as_ref().is_some_and(|id| id != &identity.request_id)
        || value.repair_job_id.as_deref().is_some_and(|id| !uuid(id))
        || value.summary_job_id.as_deref().is_some_and(|id| !uuid(id))
        || (value.summary_job_id.is_some() && input.summary.is_none())
        || value.transcript_publication.as_ref().is_some_and(|publication| {
            value.session_id.is_none()
                || !uuid(&publication.transcript_id)
                || !is_transcription_sha256(&publication.revision)
        })
        || (value.state == CaptureProcessingState::Succeeded
            && value.transcript_publication.is_none())
        || (value.state == CaptureProcessingState::Empty
            && value.transcript_publication.is_some())
        || (matches!(value.state, CaptureProcessingState::Succeeded | CaptureProcessingState::Empty)
            && input.summary.is_some()
            && value.summary_job_id.is_none())
    {
        return Err(malformed());
    }
    Ok(())
}

pub(super) async fn decode(
    response: reqwest::Response,
    operation: &RemoteOperation,
) -> Result<RemoteReply> {
    let (identity, input) = binding(operation).ok_or_else(malformed)?;
    let value: CaptureProcessingSnapshot =
        json_body(response, CAPTURE_PROCESSING_RESPONSE_LIMIT).await?;
    validate_snapshot(&value, identity, input)?;
    Ok(RemoteReply::CaptureProcessing(value))
}

impl RemoteClient {
    pub async fn declare_capture_processing(
        self: &Arc<Self>,
        identity: &CaptureIdentity,
        input: &CaptureProcessingRequest,
        cancel: CancellationToken,
    ) -> Result<CaptureProcessingSnapshot> {
        self.execute_capture_processing(
            RemoteOperation::DeclareCaptureProcessing {
                identity: identity.clone(),
                input: input.clone(),
            },
            &identity.request_id,
            cancel,
        )
        .await
    }

    pub async fn capture_processing_status(
        self: &Arc<Self>,
        identity: &CaptureIdentity,
        input: &CaptureProcessingRequest,
        cancel: CancellationToken,
    ) -> Result<CaptureProcessingSnapshot> {
        self.execute_capture_processing(
            RemoteOperation::CaptureProcessingStatus {
                identity: identity.clone(),
                input: input.clone(),
            },
            &identity.request_id,
            cancel,
        )
        .await
    }

    pub async fn cancel_capture_processing(
        self: &Arc<Self>,
        identity: &CaptureIdentity,
        input: &CaptureProcessingRequest,
        cancel: CancellationToken,
    ) -> Result<CaptureProcessingSnapshot> {
        self.execute_capture_processing(
            RemoteOperation::CancelCaptureProcessing {
                identity: identity.clone(),
                input: input.clone(),
            },
            &identity.request_id,
            cancel,
        )
        .await
    }

    async fn execute_capture_processing(
        self: &Arc<Self>,
        operation: RemoteOperation,
        request_id: &str,
        cancel: CancellationToken,
    ) -> Result<CaptureProcessingSnapshot> {
        let write = is_write(&operation);
        match self.execute(operation, request_id, cancel).await? {
            RemoteReply::CaptureProcessing(value) => Ok(value),
            _ => {
                let mut cause = malformed();
                cause.request_id = Some(request_id.into());
                if write {
                    cause.mutation_outcome = Some(MutationOutcome::Unknown);
                }
                Err(cause)
            }
        }
    }
}
