use super::{ApplicationError, CaptureSpool, ErrorCode, VaultContext, rejected};
use hypr_loofah_protocol::{CaptureIdentity, CaptureTarget};

pub(super) fn target_session_id(identity: &CaptureIdentity) -> Result<String, ApplicationError> {
    match &identity.target {
        CaptureTarget::ExistingSession { session_id, .. }
        | CaptureTarget::AppendSession { session_id, .. } => Ok(session_id.clone()),
        CaptureTarget::PendingSession { create_request_id } => {
            let request_id = uuid::Uuid::parse_str(create_request_id)
                .ok()
                .filter(|id| !id.is_nil())
                .ok_or_else(|| {
                    rejected(ErrorCode::Validation, "Invalid capture creation identifier")
                })?;
            Ok(crate::operations::creation_identity(&identity.scope.member_id, request_id).1)
        }
    }
}

impl VaultContext {
    /// Call only while holding exclusive session lifecycle admission. The private
    /// raw inventory survives uploader/native task loss and covers other members.
    pub(crate) async fn ensure_capture_session_lifecycle_idle(
        &self,
        session: &str,
    ) -> Result<(), ApplicationError> {
        self.validate(session).map_err(super::not_committed)?;
        if let Some(operations) = &self.operations {
            operations
                .ensure_creation_idle(session, self.store.content_limits().recovery_bytes)
                .await
                .map_err(super::not_committed)?;
        }
        self.ensure_recording_publication_ready(session)?;
        self.ensure_capture_intent_idle(session, false)
            .await
            .map_err(super::not_committed)?;
        self.ensure_capture_live_lifecycle_idle(session)
            .await
            .map_err(super::not_committed)?;
        if let Some(settings) = self.capture_ingestion.get() {
            let root = self
                .operations
                .as_ref()
                .ok_or_else(blocked)?
                .root
                .join("captures");
            let spool = settings
                .spool
                .get_or_try_init(|| CaptureSpool::open(root, settings.limits.clone()))
                .await
                .map_err(|_| blocked())?;
            let inventory = spool.recovery_inventory().await.map_err(|_| blocked())?;
            for snapshot in inventory {
                if snapshot.publication.is_none()
                    && (snapshot.session_id.as_deref() == Some(session)
                        || target_session_id(&snapshot.identity).map_err(|_| blocked())? == session)
                {
                    return Err(blocked());
                }
            }
        }
        self.ensure_recording_publication_ready(session)?;
        self.ensure_capture_processing_ready(session)
            .map_err(super::not_committed)
    }
}

fn blocked() -> ApplicationError {
    let mut cause = rejected(
        ErrorCode::Unavailable,
        "Capture staging requires completion, recovery or review before changing this session",
    );
    cause.retryable = true;
    cause
}
