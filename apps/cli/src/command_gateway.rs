use crate::{Error, Result, profiles::RemoteSession};
use hypr_loofah_client::{AttachmentUpload, RemoteOperation as Remote, RemoteReply};
use hypr_loofah_protocol::{LocalCliOperation as Operation, LocalCliReply as Reply, *};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
    sync::Arc,
};
use tokio_util::sync::CancellationToken;

pub(crate) enum CommandGateway {
    Local(crate::local_owner::LocalGateway),
    Remote(RemoteSession),
}
pub(crate) struct DeletedSession {
    pub path: Option<PathBuf>,
    pub trash_path: Option<PathBuf>,
    pub deleted_at: Option<String>,
    pub trash_token: Option<String>,
    pub revision: Option<String>,
}
#[derive(Debug)]
pub(crate) struct SavedAttachment {
    pub attachment_id: String,
    pub relative_path: Option<PathBuf>,
}
impl CommandGateway {
    pub fn conflict(&self, action: &'static str, revision: String) -> Error {
        let message = "The session changed concurrently; nothing was overwritten. Read it again before retrying.";
        match self {
            Self::Local(_) => Error::operation(action, message),
            Self::Remote(session) => {
                let mut cause = hypr_loofah_client::failure(ErrorCode::Conflict, message);
                cause.current_revision = Some(revision);
                cause.request_id = Some(session.request_id.clone());
                cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
                cause.into()
            }
        }
    }
    pub async fn execute(&self, operation: Operation) -> Result<Reply> {
        let Self::Remote(session) = self else {
            let Self::Local(gateway) = self else {
                unreachable!()
            };
            return Ok(gateway.execute(operation).await?.response.result?);
        };
        match operation {
            Operation::Tags {} => match session
                .execute(|vault_id| Remote::Tags { vault_id })
                .await?
            {
                RemoteReply::Tags(tags) => Ok(Reply::Tags { tags }),
                _ => Err(crate::profiles::unexpected()),
            },
            Operation::Metadata { session_id } => match session
                .execute(|vault_id| Remote::Metadata {
                    vault_id,
                    session_id,
                })
                .await?
            {
                RemoteReply::Metadata(snapshot) => Ok(Reply::Metadata { snapshot }),
                _ => Err(crate::profiles::unexpected()),
            },
            Operation::SaveMetadata { session_id, input } => match session
                .execute(|vault_id| Remote::SaveMetadata {
                    vault_id,
                    session_id,
                    input,
                })
                .await?
            {
                RemoteReply::MetadataSave(outcome) => Ok(Reply::MetadataSaved { outcome }),
                _ => Err(crate::profiles::unexpected()),
            },
            Operation::Note { session_id } => match session
                .execute(|vault_id| Remote::Note {
                    vault_id,
                    session_id,
                })
                .await?
            {
                RemoteReply::Note(snapshot) => Ok(Reply::Note { snapshot }),
                _ => Err(crate::profiles::unexpected()),
            },
            Operation::SaveNote { session_id, input } => match session
                .execute(|vault_id| Remote::SaveNote {
                    vault_id,
                    session_id,
                    input,
                })
                .await?
            {
                RemoteReply::NoteSave(outcome) => Ok(Reply::NoteSaved { outcome }),
                _ => Err(crate::profiles::unexpected()),
            },
            Operation::AppendNote {
                session_id,
                markdown,
                expected_revision,
            } => {
                let RemoteReply::Note(snapshot) = session
                    .execute(|vault_id| Remote::Note {
                        vault_id,
                        session_id: session_id.clone(),
                    })
                    .await?
                else {
                    return Err(crate::profiles::unexpected());
                };
                if snapshot.revision != expected_revision {
                    return Ok(Reply::NoteSaved {
                        outcome: NoteSaveOutcome::Conflict(snapshot),
                    });
                }
                let markdown = match snapshot.markdown {
                    Some(existing) if !existing.is_empty() => {
                        if existing.ends_with('\n') {
                            format!("{existing}{markdown}")
                        } else {
                            format!("{existing}\n{markdown}")
                        }
                    }
                    _ => markdown,
                };
                match session
                    .execute(|vault_id| Remote::SaveNote {
                        vault_id,
                        session_id,
                        input: SaveNoteRequest {
                            markdown,
                            title: None,
                            expected_revision,
                        },
                    })
                    .await?
                {
                    RemoteReply::NoteSave(outcome) => Ok(Reply::NoteSaved { outcome }),
                    _ => Err(crate::profiles::unexpected()),
                }
            }
            Operation::CreateSession {
                title,
                markdown,
                metadata,
            } => {
                let reply = if metadata == LocalInitialSessionMetadata::default() {
                    session
                        .execute(|vault_id| Remote::CreateSession {
                            vault_id,
                            input: CreateSessionRequest {
                                title,
                                markdown: markdown.unwrap_or_default(),
                            },
                        })
                        .await?
                } else {
                    session
                        .execute(|vault_id| Remote::CreateSessionWithMetadata {
                            vault_id,
                            input: CreateSessionWithMetadataRequest {
                                title,
                                markdown: markdown.unwrap_or_default(),
                                metadata,
                            },
                        })
                        .await?
                };
                match reply {
                    RemoteReply::Session(value) => Ok(Reply::SessionCreated { meta: value.meta }),
                    RemoteReply::SessionCreated(meta) => Ok(Reply::SessionCreated { meta }),
                    _ => Err(crate::profiles::unexpected()),
                }
            }
            Operation::TagDelta {
                session_id,
                tags,
                remove,
                expected_revision,
            } => edit_tags(session, session_id, tags, remove, expected_revision).await,
            Operation::SessionLifecycle { session_id } => match session
                .execute(|vault_id| Remote::SessionLifecycle {
                    vault_id,
                    session_id: session_id.clone(),
                })
                .await?
            {
                RemoteReply::SessionLifecycle(snapshot) => Ok(Reply::SessionLifecycle {
                    session_id,
                    revision: snapshot.revision,
                }),
                _ => Err(crate::profiles::unexpected()),
            },
            Operation::AgentExport { session_id } => match session
                .execute(|vault_id| Remote::AgentExport {
                    vault_id,
                    session_id,
                })
                .await?
            {
                RemoteReply::AgentExport(export) => Ok(Reply::AgentExport { export }),
                _ => Err(crate::profiles::unexpected()),
            },
            Operation::AgentSegments { session_id } => match session
                .execute(|vault_id| Remote::AgentSegments {
                    vault_id,
                    session_id,
                })
                .await?
            {
                RemoteReply::AgentSegments(segments) => Ok(Reply::AgentSegments { segments }),
                _ => Err(crate::profiles::unexpected()),
            },
            Operation::AttachmentPage {
                session_id,
                limit,
                after,
            } => {
                let limit = limit
                    .try_into()
                    .map_err(|_| Error::operation("list attachments", "Invalid page size"))?;
                match session
                    .execute(|vault_id| Remote::Attachments {
                        vault_id,
                        session_id,
                        limit,
                        after,
                    })
                    .await?
                {
                    RemoteReply::Attachments(page) => Ok(Reply::AttachmentPage { page }),
                    _ => Err(crate::profiles::unexpected()),
                }
            }
            _ => Err(Error::operation(
                "run remote command",
                "This command is not available for server profiles. No local vault was accessed.",
            )),
        }
    }
    pub async fn delete(
        &self,
        session_id: String,
        expected_revision: String,
    ) -> Result<DeletedSession> {
        match self {
            Self::Local(gateway) => match gateway
                .execute(Operation::DeleteSession {
                    session_id,
                    expected_revision,
                })
                .await?
                .response
                .result?
            {
                Reply::SessionDeleted {
                    path,
                    trash_path,
                    deleted_at,
                    ..
                } => Ok(DeletedSession {
                    path: Some(path),
                    trash_path: Some(trash_path),
                    deleted_at: Some(deleted_at),
                    trash_token: None,
                    revision: None,
                }),
                _ => Err(crate::profiles::unexpected()),
            },
            Self::Remote(session) => match session
                .execute(|vault_id| Remote::DeleteSession {
                    vault_id,
                    input: DeleteSessionRequest {
                        session_id: session_id.clone(),
                        expected_revision,
                    },
                })
                .await?
            {
                RemoteReply::SessionDelete(DeleteSessionOutcome::Trashed {
                    trash_token,
                    revision,
                }) => Ok(DeletedSession {
                    path: None,
                    trash_path: None,
                    deleted_at: None,
                    trash_token: Some(trash_token),
                    revision: Some(revision),
                }),
                RemoteReply::SessionDelete(DeleteSessionOutcome::NotFound) => {
                    Err(Error::NotFound(format!("session '{session_id}'")))
                }
                RemoteReply::SessionDelete(DeleteSessionOutcome::Conflict { snapshot }) => {
                    let mut cause = hypr_loofah_client::failure(
                        ErrorCode::Conflict,
                        "The session changed before deletion; nothing was overwritten",
                    );
                    cause.current_revision = Some(snapshot.revision);
                    cause.mutation_outcome = Some(MutationOutcome::NotCommitted);
                    cause.request_id = Some(session.request_id.clone());
                    Err(cause.into())
                }
                _ => Err(crate::profiles::unexpected()),
            },
        }
    }
    pub async fn attachment_file(
        &self,
        session_id: &str,
        filename: &str,
    ) -> Result<Option<tempfile::NamedTempFile>> {
        let suffix = std::path::Path::new(filename)
            .extension()
            .and_then(|s| s.to_str())
            .map(|s| format!(".{s}"))
            .unwrap_or_default();
        match self {
            Self::Local(gateway) => {
                let response = gateway
                    .execute(Operation::Attachment {
                        session_id: session_id.into(),
                        attachment_id: filename.into(),
                    })
                    .await?;
                match response.response.result {
                    Ok(Reply::Attachment { .. }) => {}
                    Ok(_) => return Err(crate::profiles::unexpected()),
                    Err(cause)
                        if matches!(cause.code, ErrorCode::NotFound | ErrorCode::Internal) =>
                    {
                        return Ok(None);
                    }
                    Err(cause) => return Err(cause.into()),
                }
                let blob = response.blob.ok_or_else(crate::profiles::unexpected)?;
                let private = blob.trusted_path().map_err(|cause| {
                    Error::operation("prepare export attachment", cause.to_string())
                })?;
                let directory = private
                    .parent()
                    .ok_or_else(crate::profiles::unexpected)?
                    .to_owned();
                let output = blob
                    .with_file(move |source| {
                        let mut output = tempfile::Builder::new()
                            .suffix(&suffix)
                            .tempfile_in(directory)?;
                        std::io::copy(source, output.as_file_mut())?;
                        Ok(output)
                    })
                    .await
                    .map_err(|cause| {
                        Error::operation("prepare export attachment", cause.to_string())
                    })?;
                Ok(Some(output))
            }
            Self::Remote(session) => {
                let guard = session.guard().await?;
                let id = uuid::Uuid::new_v4().to_string();
                let Some(lease) = session
                    .client
                    .open_attachment(
                        &session.vault,
                        session_id,
                        filename,
                        &id,
                        CancellationToken::new(),
                    )
                    .await?
                else {
                    return Ok(None);
                };
                let output = tempfile::Builder::new()
                    .suffix(&suffix)
                    .tempfile()
                    .map_err(|cause| {
                        Error::operation("prepare export attachment", cause.to_string())
                    })?;
                let file = output.reopen().map_err(|cause| {
                    Error::operation("prepare export attachment", cause.to_string())
                })?;
                let downloaded = session
                    .client
                    .download_attachment_with_guard(
                        lease,
                        tokio::fs::File::from_std(file),
                        (output, guard),
                        CancellationToken::new(),
                    )
                    .await;
                // Closing is best effort: the verified result remains usable if cleanup loses its reply.
                let _ = session
                    .client
                    .close_attachment(&session.vault, &id, CancellationToken::new())
                    .await;
                let (file, (output, _guard)) = downloaded?;
                drop(file);
                Ok(Some(output))
            }
        }
    }
    pub async fn attach(
        &self,
        session_id: String,
        filename: String,
        mut source: File,
    ) -> Result<SavedAttachment> {
        match self {
            Self::Local(gateway) => match gateway
                .execute_with_upload(
                    Operation::Attach {
                        session_id: session_id.clone(),
                        filename,
                    },
                    Some(source),
                    Arc::new(|_| {}),
                )
                .await?
                .response
                .result
                .map_err(|cause| {
                    if cause.code == ErrorCode::NotFound {
                        Error::NotFound(format!("meeting '{session_id}'"))
                    } else {
                        cause.into()
                    }
                })? {
                Reply::Attached {
                    attachment_id,
                    relative_path,
                } => Ok(SavedAttachment {
                    attachment_id,
                    relative_path: Some(relative_path),
                }),
                _ => Err(crate::profiles::unexpected()),
            },
            Self::Remote(session) => {
                validate_attachment_basename(&filename)
                    .map_err(|message| Error::operation("attach file", message))?;
                let guard = Arc::new(session.guard().await?);
                let original = guard.clone();
                let metadata = tokio::task::spawn_blocking(move || {
                    let _guard = original;
                    source.seek(SeekFrom::Start(0))?;
                    let metadata = hypr_fs_sync_core::media::metadata_from_reader(
                        &filename,
                        "application/octet-stream",
                        &mut source,
                    )?;
                    source.seek(SeekFrom::Start(0))?;
                    Ok::<_, std::io::Error>((metadata, source))
                })
                .await
                .map_err(|cause| Error::operation("read attachment", cause.to_string()))?
                .map_err(|cause| Error::operation("read attachment", cause.to_string()))?;
                let (metadata, source) = metadata;
                let input = UploadAttachmentRequest {
                    filename: metadata.filename,
                    size_bytes: metadata.size_bytes,
                    sha256: metadata.sha256,
                };
                let uploaded = session
                    .client
                    .upload_attachment(
                        AttachmentUpload {
                            vault_id: session.vault.clone(),
                            session_id,
                            request_id: session.request_id.clone(),
                            input,
                        },
                        OwnedInput {
                            source,
                            _owner: guard.clone(),
                        },
                        CancellationToken::new(),
                    )
                    .await?;
                Ok(SavedAttachment {
                    attachment_id: uploaded.filename,
                    relative_path: None,
                })
            }
        }
    }
}
struct OwnedInput {
    source: File,
    _owner: Arc<hypr_loofah_client::ProfileCommitGuard>,
}
impl Read for OwnedInput {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.source.read(bytes)
    }
}
async fn edit_tags(
    session: &RemoteSession,
    session_id: String,
    requested: Vec<String>,
    remove: bool,
    expected_revision: String,
) -> Result<Reply> {
    let RemoteReply::Metadata(current) = session
        .execute(|vault_id| Remote::Metadata {
            vault_id,
            session_id: session_id.clone(),
        })
        .await?
    else {
        return Err(crate::profiles::unexpected());
    };
    let mut existing = current
        .meta
        .tags
        .iter()
        .filter_map(|tag| hypr_vault_read::normalize_tag_name(tag))
        .collect::<Vec<_>>();
    existing.sort();
    existing.dedup();
    if current.revision != expected_revision {
        return Ok(Reply::TagDelta {
            outcome: MetadataSaveOutcome::Conflict(current),
            changed: Vec::new(),
            tags: existing,
        });
    }
    let requested = requested
        .into_iter()
        .map(|tag| {
            hypr_vault_read::normalize_tag_name(&tag)
                .ok_or_else(|| Error::operation("edit tags", "Tag name cannot be empty"))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut changed = requested
        .iter()
        .filter(|tag| existing.contains(tag) == remove)
        .cloned()
        .collect::<Vec<_>>();
    changed.sort();
    changed.dedup();
    let finalized = if remove {
        existing
            .into_iter()
            .filter(|tag| !requested.contains(tag))
            .collect::<Vec<_>>()
    } else {
        let mut tags = existing;
        tags.extend(changed.iter().cloned());
        tags.sort();
        tags
    };
    let outcome = if changed.is_empty() {
        MetadataSaveOutcome::Saved(current)
    } else {
        let RemoteReply::MetadataSave(outcome) = session
            .execute(|vault_id| Remote::SaveMetadata {
                vault_id,
                session_id: session_id.clone(),
                input: SaveMetadataRequest {
                    expected_revision,
                    patch: SessionMetaPatch {
                        tags: Some(finalized.clone()),
                        ..Default::default()
                    },
                },
            })
            .await?
        else {
            return Err(crate::profiles::unexpected());
        };
        outcome
    };
    if let MetadataSaveOutcome::Saved(snapshot) = &outcome {
        if !remove {
            for tag in &changed {
                if let Err(cause) = session
                    .execute(|vault_id| Remote::EnsureTag {
                        vault_id,
                        input: EnsureTagRequest { name: tag.clone() },
                    })
                    .await
                {
                    let mut failure = hypr_loofah_client::failure(
                        ErrorCode::Unavailable,
                        &format!(
                            "Tags for session {session_id} were saved, but tag registration failed: {cause}. Do not repeat the edit"
                        ),
                    );
                    failure.mutation_outcome = Some(MutationOutcome::Persisted);
                    failure.current_revision = Some(snapshot.revision.clone());
                    failure.request_id = Some(session.request_id.clone());
                    failure.retryable = false;
                    return Err(failure.into());
                }
            }
        }
    } else {
        changed.clear();
    }
    Ok(Reply::TagDelta {
        outcome,
        changed,
        tags: finalized,
    })
}
