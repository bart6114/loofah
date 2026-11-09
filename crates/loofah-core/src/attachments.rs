use std::{io::Read, sync::Arc};

use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, FileMetadata, MutationOutcome, RemoveAttachmentRequest,
};
use hypr_vault_write::{StoreError, attachment_remove::AttachmentRemoval};

use crate::{
    VaultContext,
    media::{MediaSource, media_with_metadata},
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn attachment_metadata(
        self: &Arc<Self>,
        session_id: &str,
        attachment_id: &str,
    ) -> Result<Option<FileMetadata>, ApplicationError> {
        self.read_media(
            session_id,
            MediaSource::Attachment(attachment_id.into()),
            |source| {
                media_with_metadata(source, |filename, content_type, file, size| {
                    hypr_fs_sync_core::media::metadata_from_reader(
                        filename,
                        content_type,
                        file.take(size.saturating_add(1)),
                    )
                    .map(|metadata| (metadata, ()))
                    .map_err(|cause| {
                        tracing::error!(%cause, "attachment metadata read failed");
                        error(ErrorCode::Internal, "Cannot read attachment metadata")
                    })
                })
                .map(|(_, metadata, _, ())| metadata)
            },
        )
        .await
    }

    pub async fn remove_attachment(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        session_id: &str,
        attachment_id: &str,
        input: RemoveAttachmentRequest,
    ) -> Result<bool, ApplicationError> {
        self.remove_attachment_authorized(
            namespace,
            request_id,
            session_id,
            attachment_id,
            input,
            || async { Ok(()) },
        )
        .await
    }

    pub async fn remove_attachment_authorized<
        G: Send + 'static,
        F: std::future::Future<Output = Result<G, ApplicationError>> + Send + 'static,
    >(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        session_id: &str,
        attachment_id: &str,
        input: RemoveAttachmentRequest,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<bool, ApplicationError> {
        self.validate_media(session_id, &MediaSource::Attachment(attachment_id.into()))
            .map_err(not_committed)?;
        let request_id = uuid::Uuid::parse_str(&request_id)
            .map_err(|_| rejected(ErrorCode::Validation, "A UUID request ID is required"))?;
        if input.expected_sha256.len() != 64
            || !input
                .expected_sha256
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(rejected(
                ErrorCode::Validation,
                "A lowercase SHA-256 attachment revision is required",
            ));
        }
        let root = self
            .operations
            .as_ref()
            .ok_or_else(|| {
                rejected(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                )
            })?
            .root
            .join("attachment-removals");
        let permit = self
            .media_readers
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                rejected(
                    ErrorCode::Unavailable,
                    "Media readers are busy; retry shortly",
                )
            })?;
        let context = self.clone();
        let session = session_id.to_owned();
        let attachment = attachment_id.to_owned();
        tokio::spawn(async move {
            let _permit = permit;
            let prepared = context
                .store
                .prepare_attachment_removal(
                    &root,
                    &namespace,
                    request_id,
                    &session,
                    &attachment,
                    &input.expected_sha256,
                )
                .await;
            // Check current access even for completed receipts and keep it through commit.
            let _grant = authorize().await.map_err(not_committed)?;
            let plan = prepared
                .map_err(removal_error)?
                .ok_or_else(|| rejected(ErrorCode::NotFound, "Session not found"))?;
            match plan.commit().await.map_err(removal_error)? {
                AttachmentRemoval::Removed => Ok(true),
                AttachmentRemoval::Absent => Ok(false),
                AttachmentRemoval::Changed => Err(rejected(
                    ErrorCode::Conflict,
                    "The attachment changed; review it again before removing",
                )),
                AttachmentRemoval::Busy => Err(rejected(
                    ErrorCode::Unavailable,
                    "The attachment is busy; retry when it finishes",
                )),
                AttachmentRemoval::Rejected => Err(rejected(
                    ErrorCode::Conflict,
                    "This removal was rejected; review the attachment again",
                )),
                AttachmentRemoval::Uncertain => {
                    let mut cause = error(
                        ErrorCode::Internal,
                        "Removal could not be confirmed; review the attachment and Trash",
                    );
                    cause.mutation_outcome = Some(MutationOutcome::Unknown);
                    cause.retryable = false;
                    Err(cause)
                }
            }
        })
        .await
        .map_err(|cause| {
            tracing::error!(%cause, "attachment removal stopped");
            let mut cause = error(ErrorCode::Internal, "Attachment removal stopped");
            cause.mutation_outcome = Some(MutationOutcome::Unknown);
            cause
        })?
    }
}

fn rejected(code: ErrorCode, message: &str) -> ApplicationError {
    not_committed(error(code, message))
}
fn not_committed(mut cause: ApplicationError) -> ApplicationError {
    cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
    cause
}
fn removal_error(cause: StoreError) -> ApplicationError {
    if matches!(cause, StoreError::Conflict(_)) {
        return rejected(
            ErrorCode::Conflict,
            "Removal request does not match its receipt",
        );
    }
    storage_error(cause, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{search::SearchService, settings::ConfigState, startup::StartupState};
    use hypr_vault_write::SessionStore;

    async fn fixture() -> (tempfile::TempDir, tempfile::TempDir, Arc<VaultContext>) {
        let root = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let store = Arc::new(SessionStore::new(root.path().into()));
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Title","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
        store
            .create_note_session(&meta, "Canonical notes")
            .await
            .unwrap();
        store
            .save_attachment("note", "file.pdf", b"original".to_vec())
            .await
            .unwrap();
        let context = Arc::new(
            VaultContext::new(
                "vault".into(),
                store,
                StartupState::new(Some(root.path())),
                Arc::new(ConfigState::load_or_default(root.path())),
                Arc::new(SearchService::default()),
            )
            .with_operation_storage(runtime.path().into()),
        );
        (root, runtime, context)
    }
    async fn revision(context: &Arc<VaultContext>) -> RemoveAttachmentRequest {
        RemoveAttachmentRequest {
            expected_sha256: context
                .attachment_metadata("note", "file.pdf")
                .await
                .unwrap()
                .unwrap()
                .sha256,
        }
    }
    #[tokio::test]
    async fn metadata_and_checked_removal_preserve_notes_and_receipt_replay_preserves_restored_content()
     {
        let (root, _runtime, context) = fixture().await;
        let info = context
            .attachment_metadata("note", "file.pdf")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(info.filename, "file.pdf");
        assert_eq!(info.content_type, "application/pdf");
        assert_eq!(info.size_bytes, 8);
        assert_eq!(info.sha256.len(), 64);
        assert!(
            context
                .attachment_metadata("missing", "file.pdf")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            context
                .attachment_metadata("note", "absent.pdf")
                .await
                .unwrap()
                .is_none()
        );
        let input = revision(&context).await;
        let id = uuid::Uuid::new_v4().to_string();
        assert!(
            context
                .remove_attachment(
                    "alice".into(),
                    id.clone(),
                    "note",
                    "file.pdf",
                    input.clone()
                )
                .await
                .unwrap()
        );
        context
            .store
            .save_attachment("note", "file.pdf", b"restored".to_vec())
            .await
            .unwrap();
        assert!(
            context
                .remove_attachment("alice".into(), id, "note", "file.pdf", input)
                .await
                .unwrap()
        );
        assert_eq!(
            std::fs::read(root.path().join("sessions/note/attachments/file.pdf")).unwrap(),
            b"restored"
        );
        assert_eq!(
            std::fs::read(root.path().join("sessions/note/notes.md")).unwrap(),
            b"Canonical notes"
        );
    }
    #[tokio::test]
    async fn authorization_is_rechecked_after_hashing_and_before_replaying_a_receipt() {
        let (root, _runtime, context) = fixture().await;
        let input = revision(&context).await;
        let id = uuid::Uuid::new_v4().to_string();
        let denied = || async { Err::<(), _>(error(ErrorCode::Forbidden, "Access removed")) };
        let cause = context
            .remove_attachment_authorized(
                "alice".into(),
                id.clone(),
                "note",
                "file.pdf",
                input.clone(),
                denied,
            )
            .await
            .unwrap_err();
        assert_eq!(cause.code, ErrorCode::Forbidden);
        assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
        assert!(
            root.path()
                .join("sessions/note/attachments/file.pdf")
                .exists()
        );
        assert!(
            context
                .remove_attachment(
                    "alice".into(),
                    id.clone(),
                    "note",
                    "file.pdf",
                    input.clone()
                )
                .await
                .unwrap()
        );
        let cause = context
            .remove_attachment_authorized("alice".into(), id, "note", "file.pdf", input, denied)
            .await
            .unwrap_err();
        assert_eq!(cause.code, ErrorCode::Forbidden);
    }
    #[tokio::test]
    async fn content_is_rechecked_after_authorization_without_holding_the_vault_write_lock() {
        let (root, _runtime, context) = fixture().await;
        let input = revision(&context).await;
        let source = root.path().join("sessions/note/attachments/file.pdf");
        let changed = source.clone();
        let store = context.store.clone();
        let cause = context
            .remove_attachment_authorized(
                "alice".into(),
                uuid::Uuid::new_v4().to_string(),
                "note",
                "file.pdf",
                input,
                move || async move {
                    let _snapshot = store.lock_snapshot().await;
                    std::fs::rename(&changed, changed.with_extension("old")).unwrap();
                    std::fs::write(changed, b"changed after hashing").unwrap();
                    Ok(())
                },
            )
            .await
            .unwrap_err();
        assert_eq!(cause.code, ErrorCode::Conflict);
        assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
        assert_eq!(std::fs::read(source).unwrap(), b"changed after hashing");
    }
    #[tokio::test]
    async fn invalid_revisions_and_names_never_start_removal_and_bad_receipts_report_unknown() {
        let (root, runtime, context) = fixture().await;
        for (id, name, hash) in [
            ("not-uuid", "file.pdf", "0".repeat(64)),
            (
                "00000000-0000-0000-0000-000000000001",
                "../file.pdf",
                "0".repeat(64),
            ),
            (
                "00000000-0000-0000-0000-000000000001",
                "file.pdf",
                "ABC".into(),
            ),
        ] {
            let cause = context
                .remove_attachment(
                    "alice".into(),
                    id.into(),
                    "note",
                    name,
                    RemoveAttachmentRequest {
                        expected_sha256: hash,
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(cause.code, ErrorCode::Validation);
            assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
        }
        let input = revision(&context).await;
        let id = uuid::Uuid::new_v4().to_string();
        context
            .remove_attachment(
                "alice".into(),
                id.clone(),
                "note",
                "file.pdf",
                input.clone(),
            )
            .await
            .unwrap();
        let path = std::fs::read_dir(runtime.path().join("attachment-removals"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        std::fs::write(path, "damaged receipt").unwrap();
        let cause = context
            .remove_attachment("alice".into(), id, "note", "file.pdf", input)
            .await
            .unwrap_err();
        assert_eq!(cause.mutation_outcome, Some(MutationOutcome::Unknown));
        assert!(!cause.retryable);
        assert!(
            !root
                .path()
                .join("sessions/note/attachments/file.pdf")
                .exists()
        );
    }
}
