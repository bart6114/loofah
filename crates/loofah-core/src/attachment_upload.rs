use std::{io::Read, sync::Arc};

use hypr_loofah_protocol::{
    ApplicationError, AttachmentUploadStatus, ErrorCode, FileMetadata, MutationOutcome,
    UploadAttachmentRequest, validate_upload_attachment_request,
};
use hypr_vault_write::{StoreError, attachment_upload::AttachmentPublication};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

impl VaultContext {
    pub async fn upload_attachment(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        session: &str,
        input: UploadAttachmentRequest,
        read: impl Read + Send + 'static,
    ) -> Result<FileMetadata, ApplicationError> {
        self.upload_attachment_authorized(namespace, request_id, session, input, read, || async {
            Ok(())
        })
        .await
    }

    pub async fn upload_attachment_authorized<
        G: Send + 'static,
        F: std::future::Future<Output = Result<G, ApplicationError>> + Send + 'static,
    >(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        session: &str,
        input: UploadAttachmentRequest,
        read: impl Read + Send + 'static,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<FileMetadata, ApplicationError> {
        let (root, request_id) = self.upload_parameters(session, &request_id, &input)?;
        let permit = self
            .upload_writers
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                rejected(
                    ErrorCode::Unavailable,
                    "Attachment writers are busy; retry shortly",
                )
            })?;
        let context = self.clone();
        let session = session.to_owned();
        tokio::spawn(async move {
            let _permit = permit;
            let prepared = async {
                if let Some(status) = context
                    .store
                    .prepare_attachment_upload_status(
                        &root,
                        &namespace,
                        request_id,
                        &session,
                        input.clone(),
                    )
                    .await
                    .map_err(upload_error)?
                    && !status.can_retry()
                {
                    return Ok(status);
                }
                let size = input.size_bytes;
                let stage = context
                    .store
                    .stage_attachment(&session, &input.filename, move |file| {
                        std::io::copy(&mut read.take(size.saturating_add(1)), file).map(|_| ())
                    })
                    .await
                    .map_err(upload_error)?
                    .ok_or_else(|| rejected(ErrorCode::NotFound, "Session not found"))?;
                stage
                    .prepare_publication(&root, &namespace, request_id, input.clone())
                    .await
                    .map_err(upload_error)
            }
            .await;
            let _grant = authorize().await.map_err(not_committed)?;
            let plan = prepared?;
            match plan.commit().await.map_err(upload_error)? {
                AttachmentPublication::Published(filename) => {
                    Ok(uploaded_metadata(filename, &input))
                }
                outcome => Err(publication_error(outcome)),
            }
        })
        .await
        .map_err(stopped)?
    }

    pub async fn attachment_upload_status(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        session: &str,
        input: UploadAttachmentRequest,
    ) -> Result<AttachmentUploadStatus, ApplicationError> {
        self.attachment_upload_status_authorized(namespace, request_id, session, input, || async {
            Ok(())
        })
        .await
    }

    pub async fn attachment_upload_status_authorized<
        G: Send + 'static,
        F: std::future::Future<Output = Result<G, ApplicationError>> + Send + 'static,
    >(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        session: &str,
        input: UploadAttachmentRequest,
        authorize: impl FnOnce() -> F + Send + 'static,
    ) -> Result<AttachmentUploadStatus, ApplicationError> {
        let (root, request_id) = self.upload_parameters(session, &request_id, &input)?;
        let permit = self
            .media_readers
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                error(
                    ErrorCode::Unavailable,
                    "Media readers are busy; retry shortly",
                )
            })?;
        let context = self.clone();
        let session = session.to_owned();
        tokio::spawn(async move {
            let _permit = permit;
            let prepared = context
                .store
                .prepare_attachment_upload_status(
                    &root,
                    &namespace,
                    request_id,
                    &session,
                    input.clone(),
                )
                .await;
            let _grant = authorize().await.map_err(not_committed)?;
            let Some(plan) = prepared.map_err(upload_error)? else {
                return Ok(AttachmentUploadStatus::Missing);
            };
            match plan.commit().await.map_err(upload_error)? {
                AttachmentPublication::Published(filename) => Ok(
                    AttachmentUploadStatus::Committed(uploaded_metadata(filename, &input)),
                ),
                AttachmentPublication::Rejected => Ok(AttachmentUploadStatus::Rejected),
                AttachmentPublication::Uncertain | AttachmentPublication::Changed => {
                    Ok(AttachmentUploadStatus::Uncertain)
                }
                outcome => Err(publication_error(outcome)),
            }
        })
        .await
        .map_err(stopped)?
    }

    fn upload_parameters(
        &self,
        session: &str,
        request_id: &str,
        input: &UploadAttachmentRequest,
    ) -> Result<(std::path::PathBuf, uuid::Uuid), ApplicationError> {
        self.validate(session).map_err(not_committed)?;
        validate_upload_attachment_request(input)
            .map_err(|message| rejected(ErrorCode::Validation, message))?;
        let id = uuid::Uuid::parse_str(request_id)
            .map_err(|_| rejected(ErrorCode::Validation, "A UUID request ID is required"))?;
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
            .join("attachment-uploads");
        Ok((root, id))
    }
}

fn uploaded_metadata(filename: String, input: &UploadAttachmentRequest) -> FileMetadata {
    FileMetadata {
        content_type: mime_guess::from_path(&filename)
            .first_or_octet_stream()
            .essence_str()
            .into(),
        filename,
        size_bytes: input.size_bytes,
        sha256: input.sha256.clone(),
    }
}
fn publication_error(outcome: AttachmentPublication) -> ApplicationError {
    match outcome {
        AttachmentPublication::Changed => rejected(
            ErrorCode::Conflict,
            "Attachment publication changed; review the upload again",
        ),
        AttachmentPublication::Rejected => rejected(
            ErrorCode::Internal,
            "Attachment publication failed before any file was published",
        ),
        AttachmentPublication::Busy => {
            let mut cause = error(
                ErrorCode::Unavailable,
                "Attachment verification is busy; check its status again",
            );
            cause.mutation_outcome = Some(MutationOutcome::Unknown);
            cause
        }
        _ => {
            let mut cause = error(
                ErrorCode::Internal,
                "Upload could not be confirmed; check its status before trying again",
            );
            cause.mutation_outcome = Some(MutationOutcome::Unknown);
            cause
        }
    }
}
fn rejected(code: ErrorCode, message: &str) -> ApplicationError {
    not_committed(error(code, message))
}
fn not_committed(mut cause: ApplicationError) -> ApplicationError {
    cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
    cause
}
fn upload_error(cause: StoreError) -> ApplicationError {
    match cause {
        StoreError::Conflict(_) => rejected(
            ErrorCode::Conflict,
            "Upload bytes or request do not match the reviewed attachment",
        ),
        StoreError::Serialize(_) => {
            rejected(ErrorCode::Validation, "Invalid attachment upload request")
        }
        cause => storage_error(cause, true),
    }
}
fn stopped(cause: tokio::task::JoinError) -> ApplicationError {
    tracing::error!(%cause, "attachment upload stopped");
    let mut cause = error(ErrorCode::Internal, "Attachment upload stopped");
    cause.mutation_outcome = Some(MutationOutcome::Unknown);
    cause
}

#[cfg(test)]
mod tests;
