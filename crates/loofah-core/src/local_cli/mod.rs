use crate::{
    VaultContext,
    vault::{error, storage_error},
};
use hypr_loofah_protocol::*;
use std::{io::Seek, sync::Arc};

mod artifact;
mod maintenance;
mod media;
pub use media::{LocalMediaFuture, LocalMediaHost};
mod mutations;
mod read_owner;
#[cfg(test)]
mod tests;
pub use artifact::LocalCliArtifact;
pub use maintenance::execute_local_maintenance;

pub const LOCAL_CLI_NAMESPACE: &str = "local-cli";

pub struct LocalCli {
    context: Arc<VaultContext>,
    read_only: bool,
    media: Option<Arc<dyn LocalMediaHost>>,
}
pub struct LocalCliOutput {
    pub reply: LocalCliReply,
    pub artifact: Option<LocalCliArtifact>,
}

impl LocalCli {
    pub fn new(context: Arc<VaultContext>) -> Result<Self, ApplicationError> {
        if context.lease_pin().is_none() {
            return Err(error(
                ErrorCode::Unavailable,
                "Local CLI operations require an owned vault",
            ));
        }
        Ok(Self {
            context,
            read_only: false,
            media: None,
        })
    }

    pub fn with_media_host(mut self, media: Arc<dyn LocalMediaHost>) -> Self {
        self.media = Some(media);
        self
    }

    pub fn operations(&self) -> Vec<LocalCliOperationKind> {
        use LocalCliOperationKind::*;
        if self.read_only {
            return read_owner::operations();
        }
        let mut operations = vec![
            AgentList,
            AgentSearch,
            AgentMeeting,
            AgentTranscript,
            AgentExport,
            AgentSegments,
            Metadata,
            SaveMetadata,
            Note,
            SaveNote,
            AppendNote,
            TagDelta,
            CreateSession,
            SessionLifecycle,
            DeleteSession,
            SessionPath,
            Tags,
            Attach,
            Attachment,
            AttachmentPage,
            Doctor,
            Initialize,
        ];
        if self.media.is_some() {
            operations.extend([Import, Transcribe]);
        }
        operations
    }

    pub async fn execute(
        self: &Arc<Self>,
        request_id: String,
        operation: LocalCliOperation,
        artifact: Option<LocalCliArtifact>,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> Result<LocalCliOutput, ApplicationError> {
        if !uuid::Uuid::parse_str(&request_id)
            .is_ok_and(|id| !id.is_nil() && id.to_string() == request_id)
        {
            return Err(error(
                ErrorCode::Validation,
                "A canonical request UUID is required",
            ));
        }
        if !self.operations().contains(&operation.kind()) {
            let mut cause = error(
                ErrorCode::Unavailable,
                "This local operation is not yet available through the vault owner",
            );
            if operation.is_mutation() {
                cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
            }
            return Err(cause);
        }
        if operation.has_blob() != artifact.is_some() {
            return Err(error(
                ErrorCode::Validation,
                "Local operation artifact does not match its declaration",
            ));
        }
        let dispatcher = self.clone();
        tokio::spawn(async move {
            if matches!(
                dispatcher.context.startup.snapshot().phase,
                StartupPhase::Stopping | StartupPhase::Stopped | StartupPhase::Failed { .. }
            ) {
                let mut cause = error(
                    ErrorCode::Unavailable,
                    "Vault owner is stopping or unavailable",
                );
                cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
                return Err(cause);
            }
            dispatcher
                .context
                .store
                .ensure_ready()
                .map_err(|_| error(ErrorCode::Unavailable, "Vault startup is still in progress"))?;
            dispatcher
                .dispatch(request_id, operation, artifact, progress)
                .await
        })
        .await
        .unwrap_or_else(|cause| {
            // The endpoint must retain ownership when admitted work loses its supervisor.
            // An application-level Unknown response cannot communicate that drain failure.
            if cause.is_panic() {
                std::panic::resume_unwind(cause.into_panic());
            }
            panic!("Local operation supervisor was cancelled: {cause}");
        })
    }

    async fn dispatch(
        &self,
        request_id: String,
        operation: LocalCliOperation,
        artifact: Option<LocalCliArtifact>,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> Result<LocalCliOutput, ApplicationError> {
        let context = &self.context;
        let reply = match operation {
            LocalCliOperation::AgentList { input } => LocalCliReply::AgentList {
                page: context.agent_list_local(input).await?,
            },
            LocalCliOperation::AgentSearch { input } => LocalCliReply::AgentSearch {
                page: context.agent_search_local(input).await?,
            },
            LocalCliOperation::AgentMeeting { session_id } => LocalCliReply::AgentMeeting {
                meeting: context.agent_meeting(&session_id).await?,
            },
            LocalCliOperation::AgentTranscript { session_id } => LocalCliReply::AgentTranscript {
                transcript: context.agent_transcript(&session_id).await?,
            },
            LocalCliOperation::AgentExport { session_id } => LocalCliReply::AgentExport {
                export: context.agent_export(&session_id).await?,
            },
            LocalCliOperation::AgentSegments { session_id } => LocalCliReply::AgentSegments {
                segments: context.agent_segments(&session_id).await?,
            },
            LocalCliOperation::AttachmentPage {
                session_id,
                limit,
                after,
            } => LocalCliReply::AttachmentPage {
                page: context
                    .attachment_page(&session_id, limit, after.as_deref())
                    .await?,
            },
            LocalCliOperation::Metadata { session_id } => LocalCliReply::Metadata {
                snapshot: context.read_metadata(&session_id).await?,
            },
            LocalCliOperation::SaveMetadata { session_id, input } => LocalCliReply::MetadataSaved {
                outcome: context.save_metadata_local(&session_id, input).await?,
            },
            LocalCliOperation::Note { session_id } => LocalCliReply::Note {
                snapshot: context.read_note(&session_id).await?,
            },
            LocalCliOperation::SaveNote { session_id, input } => LocalCliReply::NoteSaved {
                outcome: context.save_note(&session_id, input).await?,
            },
            LocalCliOperation::AppendNote {
                session_id,
                markdown,
                expected_revision,
            } => LocalCliReply::NoteSaved {
                outcome: mutations::append(context, &session_id, markdown, expected_revision)
                    .await?,
            },
            LocalCliOperation::TagDelta {
                session_id,
                tags,
                remove,
                expected_revision,
            } => mutations::tags(context, &session_id, tags, remove, expected_revision).await?,
            LocalCliOperation::CreateSession {
                title,
                markdown,
                metadata,
            } => {
                let metadata = crate::InitialSessionMetadata {
                    created_at: metadata.created_at,
                    started_at: metadata.started_at,
                    ended_at: metadata.ended_at,
                    tags: metadata.tags,
                    author: metadata.author,
                    skill: metadata.skill,
                };
                let meta = context
                    .create_session_with_metadata(
                        LOCAL_CLI_NAMESPACE.into(),
                        request_id,
                        CreateSessionRequest {
                            title,
                            markdown: markdown.unwrap_or_default(),
                        },
                        metadata,
                    )
                    .await?;
                LocalCliReply::SessionCreated { meta }
            }
            LocalCliOperation::SessionLifecycle { session_id } => {
                let snapshot = context.read_session_lifecycle(&session_id).await?;
                LocalCliReply::SessionLifecycle {
                    session_id: snapshot.session_id,
                    revision: snapshot.revision,
                }
            }
            LocalCliOperation::DeleteSession {
                session_id,
                expected_revision,
            } => {
                let input = DeleteSessionRequest {
                    session_id: session_id.clone(),
                    expected_revision,
                };
                match context
                    .delete_session(
                        LOCAL_CLI_NAMESPACE.into(),
                        request_id.clone(),
                        input.clone(),
                    )
                    .await?
                {
                    DeleteSessionOutcome::Trashed { .. } => {
                        let details = context
                            .local_session_deletion_details(
                                LOCAL_CLI_NAMESPACE.into(),
                                request_id,
                                input,
                            )
                            .await?
                            .ok_or_else(|| {
                                error(ErrorCode::Internal, "Deletion receipt requires review")
                            })?;
                        LocalCliReply::SessionDeleted {
                            session_id,
                            path: details.path,
                            trash_path: details.trash_path,
                            deleted_at: details.deleted_at,
                        }
                    }
                    DeleteSessionOutcome::NotFound => {
                        return Err(error(ErrorCode::NotFound, "Session not found"));
                    }
                    DeleteSessionOutcome::Conflict { snapshot } => {
                        let mut cause =
                            error(ErrorCode::Conflict, "Session changed before deletion");
                        cause.current_revision = Some(snapshot.revision);
                        cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
                        return Err(cause);
                    }
                }
            }
            LocalCliOperation::SessionPath { session_id } => {
                context.read_metadata(&session_id).await?;
                LocalCliReply::SessionPath {
                    path: context.store.vault_base().join(
                        context
                            .store
                            .session_dir(&session_id)
                            .await
                            .map_err(|cause| storage_error(cause, false))?,
                    ),
                }
            }
            LocalCliOperation::Tags {} => {
                let cache = context
                    .search
                    .cache()
                    .await
                    .map_err(|_| error(ErrorCode::Unavailable, "Search is not ready"))?;
                let context = context.clone();
                let tags = tokio::task::spawn_blocking(move || {
                    let _context = context;
                    cache.tags_fresh().map_err(|_| {
                        error(ErrorCode::Internal, "Vault tag registry could not be read")
                    })
                })
                .await
                .map_err(|_| error(ErrorCode::Internal, "Tag reader stopped"))??;
                LocalCliReply::Tags { tags }
            }
            LocalCliOperation::Attach {
                session_id,
                filename,
            } => {
                let _admission = context.admit_session_work(&session_id).await?;
                let mut artifact = artifact.ok_or_else(|| {
                    error(ErrorCode::Validation, "Attachment content is required")
                })?;
                artifact
                    .rewind()
                    .map_err(|_| error(ErrorCode::Internal, "Cannot read attachment input"))?;
                let filename = hypr_fs_sync_core::attachment::sanitize_filename(&filename)
                    .map_err(|_| error(ErrorCode::Validation, "Invalid attachment filename"))?;
                let input = UploadAttachmentRequest {
                    filename,
                    size_bytes: artifact.size_bytes,
                    sha256: artifact.sha256.clone(),
                };
                let saved = context
                    .upload_attachment(
                        LOCAL_CLI_NAMESPACE.into(),
                        request_id,
                        &session_id,
                        input,
                        artifact,
                    )
                    .await?;
                let directory = context
                    .store
                    .session_dir(&session_id)
                    .await
                    .map_err(|cause| storage_error(cause, false))?;
                LocalCliReply::Attached {
                    attachment_id: saved.filename.clone(),
                    relative_path: directory.join("attachments").join(saved.filename),
                }
            }
            LocalCliOperation::Attachment {
                session_id,
                attachment_id,
            } => {
                let admission = context.admit_session_work(&session_id).await?;
                let capacity = context
                    .media_readers
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| error(ErrorCode::Unavailable, "Media readers are busy"))?;
                let opened = context
                    .store
                    .open_attachment(&session_id, &attachment_id)
                    .await
                    .map_err(|cause| storage_error(cause, false))?
                    .ok_or_else(|| error(ErrorCode::NotFound, "Attachment not found"))?;
                let size = opened
                    .file
                    .metadata()
                    .map_err(|_| error(ErrorCode::Internal, "Cannot inspect attachment"))?
                    .len();
                let artifact = LocalCliArtifact::new(
                    opened.file,
                    size,
                    String::new(),
                    None,
                    Box::new((context.clone(), admission, capacity)),
                );
                return Ok(LocalCliOutput {
                    reply: LocalCliReply::Attachment {
                        filename: opened.attachment_id,
                    },
                    artifact: Some(artifact),
                });
            }
            LocalCliOperation::Initialize {} => return self.initialize(progress).await,
            LocalCliOperation::Doctor {} => return self.doctor().await,
            operation @ (LocalCliOperation::Import { .. }
            | LocalCliOperation::Transcribe { .. }) => {
                return self
                    .execute_media(request_id, operation, artifact, progress)
                    .await;
            }
        };
        Ok(LocalCliOutput {
            reply,
            artifact: None,
        })
    }
}
