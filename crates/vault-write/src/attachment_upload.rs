use std::path::{Path, PathBuf};

use hypr_loofah_protocol::UploadAttachmentRequest;
use serde::{Deserialize, Serialize};

use crate::{
    SessionStore, StoreError,
    attachment_stage::SessionAttachmentStage,
    file_snapshot::FileSnapshot,
    media::open_regular_file,
    operation_io::{operation_key, persist, read_receipt},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentPublication {
    Published(String),
    Changed,
    Busy,
    Rejected,
    Uncertain,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    session: String,
    input: UploadAttachmentRequest,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Receipt {
    schema_version: u32,
    request: Request,
    filename: String,
    state: State,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum State {
    Pending,
    Committed,
    Rejected,
    Uncertain,
}

pub struct AttachmentPublicationPlan {
    store: SessionStore,
    stage: Option<SessionAttachmentStage>,
    path: PathBuf,
    request: Request,
    receipt: Option<Receipt>,
    source: Option<FileSnapshot>,
    recovery: Option<FileSnapshot>,
    finished: Option<AttachmentPublication>,
    _serial: tokio::sync::OwnedMutexGuard<()>,
}

impl SessionAttachmentStage {
    pub async fn prepare_publication(
        self,
        operations: &Path,
        namespace: &str,
        request_id: uuid::Uuid,
        input: UploadAttachmentRequest,
    ) -> Result<AttachmentPublicationPlan, StoreError> {
        let request = Request {
            session: self.session_id.clone(),
            input,
        };
        let store = self.store.clone();
        prepare(
            store,
            Some(self),
            operations,
            namespace,
            request_id,
            request,
        )
        .await?
        .ok_or_else(|| StoreError::Io("Missing attachment publication plan".into()))
    }
}

impl SessionStore {
    pub async fn prepare_attachment_upload_status(
        &self,
        operations: &Path,
        namespace: &str,
        request_id: uuid::Uuid,
        session: &str,
        input: UploadAttachmentRequest,
    ) -> Result<Option<AttachmentPublicationPlan>, StoreError> {
        prepare(
            self.clone(),
            None,
            operations,
            namespace,
            request_id,
            Request {
                session: session.into(),
                input,
            },
        )
        .await
    }
}

async fn prepare(
    store: SessionStore,
    stage: Option<SessionAttachmentStage>,
    operations: &Path,
    namespace: &str,
    request_id: uuid::Uuid,
    request: Request,
) -> Result<Option<AttachmentPublicationPlan>, StoreError> {
    crate::validate_session_id(&request.session)?;
    hypr_loofah_protocol::validate_upload_attachment_request(&request.input)
        .map_err(|message| StoreError::Serialize(message.into()))?;
    let path = operations.join(format!("{}.json", operation_key(namespace, request_id)));
    tokio::spawn(async move {
        let serial = store.attachment_publications.clone().lock_owned().await;
        let read_path = path.clone();
        let mut receipt = tokio::task::spawn_blocking(move || read_receipt::<Receipt>(&read_path))
            .await
            .map_err(worker_error)??;
        if let Some(existing) = &receipt {
            if existing.request != request {
                return Err(StoreError::Conflict(
                    "Attachment upload receipt does not match this request".into(),
                ));
            }
            if existing.schema_version != 1 {
                return Ok(Some(AttachmentPublicationPlan {
                    store,
                    stage,
                    path,
                    request,
                    receipt,
                    source: None,
                    recovery: None,
                    finished: Some(AttachmentPublication::Uncertain),
                    _serial: serial,
                }));
            }
            hypr_vault_read::paths::validate_attachment_id(&existing.filename)?;
        } else if stage.is_none() {
            return Ok(None);
        }

        let recovery = if let Some(existing) = &receipt
            && matches!(existing.state, State::Pending)
        {
            store
                .open_attachment(&request.session, &existing.filename)
                .await?
                .map(|opened| FileSnapshot::new(opened.file))
                .transpose()?
        } else {
            None
        };
        tokio::task::spawn_blocking(move || {
            let source = if let Some(prepared) = &stage {
                if prepared.filename
                    != hypr_fs_sync_core::attachment::sanitize_filename(&request.input.filename)
                        .map_err(io_error)?
                {
                    return Err(StoreError::Conflict(
                        "Attachment upload filename changed".into(),
                    ));
                }
                let file = open_regular_file(prepared.path())?
                    .ok_or_else(|| StoreError::Conflict("Attachment stage disappeared".into()))?;
                let source = FileSnapshot::new(file)?;
                if !source.lock_for_recheck()? {
                    return Ok(Some(AttachmentPublicationPlan {
                        store,
                        stage,
                        path,
                        request,
                        receipt,
                        source: None,
                        recovery: None,
                        finished: Some(AttachmentPublication::Busy),
                        _serial: serial,
                    }));
                }
                if !matches_input(&source, &request.input)? {
                    return Err(StoreError::Conflict(
                        "Attachment bytes do not match the declared length and checksum".into(),
                    ));
                }
                Some(source)
            } else {
                None
            };
            let finished = match &receipt {
                Some(existing) => match existing.state {
                    State::Committed => {
                        Some(AttachmentPublication::Published(existing.filename.clone()))
                    }
                    State::Uncertain => Some(AttachmentPublication::Uncertain),
                    State::Rejected if stage.is_none() => Some(AttachmentPublication::Rejected),
                    State::Rejected => {
                        receipt = None;
                        None
                    }
                    State::Pending => {
                        if let Some(file) = &recovery {
                            if !file.lock_for_recheck()? {
                                Some(AttachmentPublication::Busy)
                            } else if matches_input(file, &request.input)? {
                                Some(AttachmentPublication::Published(existing.filename.clone()))
                            } else {
                                Some(AttachmentPublication::Uncertain)
                            }
                        } else {
                            Some(AttachmentPublication::Uncertain)
                        }
                    }
                },
                None => None,
            };
            Ok(Some(AttachmentPublicationPlan {
                store,
                stage,
                path,
                request,
                receipt,
                source,
                recovery,
                finished,
                _serial: serial,
            }))
        })
        .await
        .map_err(worker_error)?
    })
    .await
    .map_err(worker_error)?
}

impl AttachmentPublicationPlan {
    pub fn can_retry(&self) -> bool {
        self.stage.is_none() && self.finished == Some(AttachmentPublication::Rejected)
    }

    pub async fn commit(self) -> Result<AttachmentPublication, StoreError> {
        self.commit_with(|| Ok(())).await
    }

    async fn commit_with(
        self,
        after_move: impl FnOnce() -> Result<(), StoreError> + Send + 'static,
    ) -> Result<AttachmentPublication, StoreError> {
        tokio::spawn(async move {
            let snapshot = self.store.lock_snapshot().await;
            self.store.ensure_ready()?;
            let relative = self.store.session_dir(&self.request.session).await?;
            let directory = self.store.vault_base.join(&relative).join("attachments");
            let needs_session = self.receipt.as_ref().is_none_or(|receipt| receipt.schema_version == 1 && matches!(receipt.state, State::Pending));
            let session_exists = needs_session && self.store.metadata_snapshot_locked(&snapshot._guard, &self.request.session).await?.is_some();
            tokio::task::spawn_blocking(move || {
                let _snapshot = snapshot;
                let _serial = self._serial;
                if let Some(mut existing) = self.receipt {
                    if existing.schema_version != 1 { return Ok(AttachmentPublication::Uncertain); }
                    if !matches!(existing.state, State::Pending) { return Ok(self.finished.unwrap()); }
                    if self.finished == Some(AttachmentPublication::Busy) { return Ok(AttachmentPublication::Busy); }
                    let current = if session_exists { open_target(&directory, &existing.filename)? } else { None };
                    let confirmed = match (&self.recovery, current.as_ref(), &self.finished) {
                        (Some(before), Some(after), Some(AttachmentPublication::Published(_))) => before.unchanged(after)?,
                        _ => false,
                    };
                    if confirmed {
                        existing.state = State::Committed;
                        persist(&self.path, &existing)?;
                        return Ok(AttachmentPublication::Published(existing.filename));
                    }
                    existing.state = State::Uncertain;
                    persist(&self.path, &existing)?;
                    return Ok(AttachmentPublication::Uncertain);
                }
                if let Some(finished) = self.finished { return Ok(finished); }
                if !session_exists || self.store.deleted_sessions.lock().unwrap().contains(&self.request.session) { return Ok(AttachmentPublication::Changed); }
                let mut stage = self.stage.ok_or_else(|| StoreError::Io("Missing attachment stage".into()))?;
                if relative.join("attachments") != stage.relative_directory {
                    return Ok(AttachmentPublication::Changed);
                }
                let current = open_regular_file(stage.path())?.map(FileSnapshot::new).transpose()?;
                if !matches!((&self.source, &current), (Some(before), Some(after)) if before.unchanged(after)?) {
                    return Ok(AttachmentPublication::Changed);
                }
                for counter in 0..u64::MAX {
                    let filename = hypr_fs_sync_core::attachment::candidate_filename(&stage.filename, counter);
                    let target = directory.join(&filename);
                    match std::fs::symlink_metadata(&target) {
                        Ok(_) => continue,
                        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {},
                        Err(cause) => return Err(io_error(cause)),
                    }
                    let mut receipt = Receipt { schema_version: 1, request: self.request.clone(), filename: filename.clone(), state: State::Pending };
                    persist(&self.path, &receipt)?;
                    if let Err(cause) = stage.staged.publish_as(&filename) {
                        if stage.staged.is_published() {
                            self.store.notify_attachment_changed(&self.request.session, Some(&filename));
                            return Err(io_error(cause));
                        }
                        receipt.state = State::Rejected;
                        persist(&self.path, &receipt)?;
                        return Ok(if cause.kind() == std::io::ErrorKind::AlreadyExists { AttachmentPublication::Changed } else { AttachmentPublication::Rejected });
                    }
                    self.store.notify_attachment_changed(&self.request.session, Some(&filename));
                    after_move()?;
                    receipt.state = State::Committed;
                    persist(&self.path, &receipt)?;
                    return Ok(AttachmentPublication::Published(filename));
                }
                Err(StoreError::ResourceLimit("Attachment name space exhausted".into()))
            }).await.map_err(worker_error)?
        }).await.map_err(worker_error)?
    }
}

fn open_target(directory: &Path, filename: &str) -> Result<Option<FileSnapshot>, StoreError> {
    match std::fs::symlink_metadata(directory) {
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(cause) => return Err(io_error(cause)),
        Ok(meta) if !meta.is_dir() || meta.file_type().is_symlink() => {
            return Err(StoreError::Io("Attachment directory is not regular".into()));
        }
        Ok(_) => {}
    }
    open_regular_file(&directory.join(filename))?
        .map(FileSnapshot::new)
        .transpose()
}
fn matches_input(file: &FileSnapshot, input: &UploadAttachmentRequest) -> Result<bool, StoreError> {
    Ok(
        file.file().metadata().map_err(io_error)?.len() == input.size_bytes
            && file.hash()? == input.sha256,
    )
}
fn io_error(cause: std::io::Error) -> StoreError {
    StoreError::Io(cause.to_string())
}
fn worker_error(cause: tokio::task::JoinError) -> StoreError {
    StoreError::Io(format!("Attachment upload worker stopped: {cause}"))
}

#[cfg(test)]
mod tests;
