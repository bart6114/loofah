use super::*;
use std::io::Read;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitialSessionMetadata {
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub tags: Vec<String>,
    pub author: Option<String>,
    pub skill: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RichCreation {
    schema_version: u32,
    operation_key: String,
    namespace: String,
    request_id: String,
    title: String,
    markdown: String,
    metadata: InitialSessionMetadata,
    frozen_meta: SessionMeta,
    files_created: bool,
    completed: bool,
}

impl InitialSessionMetadata {
    fn normalize(mut self) -> Result<Self, ApplicationError> {
        fn timestamp(value: Option<String>) -> Result<Option<String>, ApplicationError> {
            value
                .map(|value| {
                    chrono::DateTime::parse_from_rfc3339(&value)
                        .map(|time| {
                            time.with_timezone(&chrono::Utc)
                                .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
                        })
                        .map_err(|_| error(ErrorCode::Validation, "Invalid creation timestamp"))
                })
                .transpose()
        }
        self.created_at = timestamp(self.created_at)?;
        self.started_at = timestamp(self.started_at)?;
        self.ended_at = timestamp(self.ended_at)?;
        if self.skill.is_some() && self.author.is_none() {
            return Err(error(
                ErrorCode::Validation,
                "A creation skill requires an author",
            ));
        }
        if self
            .tags
            .iter()
            .any(|tag| hypr_vault_read::normalize_tag_name(tag).is_none())
        {
            return Err(error(ErrorCode::Validation, "Invalid creation tag"));
        }
        Ok(self)
    }
    fn resolve(&self, id: String, title: String, created_at: String) -> SessionMeta {
        SessionMeta {
            id,
            title,
            created_at,
            started_at: self.started_at.clone(),
            ended_at: self.ended_at.clone(),
            tags: self.tags.clone(),
            author: self.author.clone(),
            skill: self.skill.clone(),
            tag_suggestions: None,
            tracking_id: None,
            folder: None,
            extra: Default::default(),
        }
    }
    fn matches_legacy(&self, creation: &Creation) -> bool {
        self.started_at.is_none()
            && self.ended_at.is_none()
            && self.tags.is_empty()
            && self.author.is_none()
            && self.skill.is_none()
            && self
                .created_at
                .as_ref()
                .is_none_or(|date| date == &creation.created_at)
    }
}

impl RichCreation {
    fn validate(&self, session: &str) -> Result<(), ApplicationError> {
        let request_id = uuid::Uuid::parse_str(&self.request_id)
            .ok()
            .filter(|id| !id.is_nil() && id.to_string() == self.request_id)
            .ok_or_else(|| {
                error(
                    ErrorCode::Conflict,
                    "Creation receipt has an invalid request identifier",
                )
            })?;
        let (key, id) = creation_identity(&self.namespace, request_id);
        let date = self
            .metadata
            .created_at
            .clone()
            .unwrap_or_else(|| self.frozen_meta.created_at.clone());
        if self.schema_version != 2
            || self.operation_key != key
            || id != session
            || self.metadata.clone().normalize()? != self.metadata
            || self.metadata.resolve(id, self.title.clone(), date) != self.frozen_meta
            || (self.completed && !self.files_created)
            || chrono::DateTime::parse_from_rfc3339(&self.frozen_meta.created_at).is_err()
        {
            return Err(error(
                ErrorCode::Conflict,
                "Creation receipt requires review",
            ));
        }
        Ok(())
    }
}

fn receipt_path(root: &Path, session: &str) -> PathBuf {
    root.join("session-creations")
        .join(format!("{session}.json"))
}

fn check_file(path: &Path) -> Result<bool, ApplicationError> {
    if let Some(parent) = path.parent() {
        match std::fs::symlink_metadata(parent) {
            Ok(meta) if meta.is_dir() && !meta.file_type().is_symlink() => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            _ => {
                return Err(error(
                    ErrorCode::Internal,
                    "Unsafe creation receipt directory",
                ));
            }
        }
    }
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => Ok(true),
        Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(false),
        _ => Err(error(ErrorCode::Internal, "Unsafe creation receipt")),
    }
}

async fn read_private<T: serde::de::DeserializeOwned + Send + 'static>(
    path: PathBuf,
    limit: usize,
) -> Result<Option<T>, ApplicationError> {
    tokio::task::spawn_blocking(move || {
        if !check_file(&path)? {
            return Ok(None);
        }
        let file = std::fs::File::open(path).map_err(io_error)?;
        let limit = u64::try_from(limit).unwrap_or(u64::MAX);
        if file.metadata().map_err(io_error)?.len() > limit {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Creation receipt exceeds its limit",
            ));
        }
        let mut bytes = Vec::new();
        file.take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() as u64 > limit {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Creation receipt exceeds its limit",
            ));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| error(ErrorCode::Internal, "Creation receipt is invalid"))
    })
    .await
    .map_err(|_| error(ErrorCode::Internal, "Creation receipt read failed"))?
}

pub(super) async fn reject_rich_reuse(root: &Path, session: &str) -> Result<(), ApplicationError> {
    let path = receipt_path(root, session);
    if tokio::task::spawn_blocking(move || check_file(&path))
        .await
        .map_err(|_| error(ErrorCode::Internal, "Creation receipt check failed"))??
    {
        return Err(error(
            ErrorCode::Conflict,
            "Request identifier belongs to an internal metadata creation",
        ));
    }
    Ok(())
}

impl VaultContext {
    pub async fn create_session_with_metadata(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        request: CreateSessionRequest,
        metadata: InitialSessionMetadata,
    ) -> Result<SessionMeta, ApplicationError> {
        self.store
            .ensure_ready()
            .map_err(|cause| storage_error(cause, false))?;
        let request_id = uuid::Uuid::parse_str(&request_id)
            .ok()
            .filter(|id| !id.is_nil())
            .ok_or_else(|| error(ErrorCode::Validation, "A UUID request ID is required"))?;
        let metadata = metadata.normalize()?;
        let limits = self.store.content_limits();
        if request.title.len() > limits.title_bytes
            || request.markdown.len() > limits.note_write_bytes
        {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Note exceeds the size limit",
            ));
        }
        let context = self.clone();
        tokio::spawn(async move {
            let (_, session) = creation_identity(&namespace, request_id);
            let _admission = context.admit_session_work(&session).await?;
            let operations = context.operations.as_ref().ok_or_else(|| {
                error(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                )
            })?;
            operations
                .create_rich(&context.store, namespace, request_id, request, metadata)
                .await
        })
        .await
        .map_err(|_| {
            let mut cause = error(
                ErrorCode::Internal,
                "Session metadata creation outcome is unknown",
            );
            cause.mutation_outcome = Some(MutationOutcome::Unknown);
            cause
        })?
    }
}

impl SessionOperations {
    pub(crate) async fn ensure_creation_idle(
        &self,
        session: &str,
        limit: usize,
    ) -> Result<(), ApplicationError> {
        if let Some(receipt) =
            read_private::<RichCreation>(receipt_path(&self.root, session), limit).await?
        {
            receipt.validate(session)?;
            if !receipt.completed {
                return Err(error(
                    ErrorCode::Unavailable,
                    "Session creation requires completion or review before changing this session",
                ));
            }
        }
        Ok(())
    }

    async fn create_rich(
        &self,
        store: &SessionStore,
        namespace: String,
        request_id: uuid::Uuid,
        request: CreateSessionRequest,
        metadata: InitialSessionMetadata,
    ) -> Result<SessionMeta, ApplicationError> {
        let _guard = self.lock.lock().await;
        let (key, session) = creation_identity(&namespace, request_id);
        let path = receipt_path(&self.root, &session);
        let limits = store.content_limits();
        let rich = read_private::<RichCreation>(path.clone(), limits.recovery_bytes).await?;
        let legacy =
            read_private::<Creation>(self.root.join(format!("{key}.json")), limits.recovery_bytes)
                .await?;
        if rich.is_some() && legacy.is_some() {
            return Err(error(
                ErrorCode::Conflict,
                "Multiple creation decisions require review",
            ));
        }
        if let Some(legacy) = legacy {
            if legacy.schema_version != 1
                || legacy.request != request
                || legacy.session_id != session
                || !metadata.matches_legacy(&legacy)
            {
                return Err(error(
                    ErrorCode::Conflict,
                    "Request identifier was used with different creation input",
                ));
            }
            self.create_locked(store, &namespace, request_id, request, metadata.created_at)
                .await?;
            return Ok(legacy_meta(&legacy));
        }
        let fresh = rich.is_none();
        let mut receipt = if let Some(receipt) = rich {
            receipt.validate(&session)?;
            if receipt.namespace != namespace
                || receipt.request_id != request_id.to_string()
                || receipt.title != request.title
                || receipt.markdown != request.markdown
                || receipt.metadata != metadata
            {
                return Err(error(
                    ErrorCode::Conflict,
                    "Request identifier was used with different creation input",
                ));
            }
            receipt
        } else {
            if store
                .read_meta(&session)
                .await
                .map_err(|cause| storage_error(cause, false))?
                .is_some()
            {
                return Err(error(
                    ErrorCode::Conflict,
                    "Creation receipt is missing for an existing session",
                ));
            }
            let created_at = metadata.created_at.clone().unwrap_or_else(|| {
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
            });
            let frozen_meta = metadata.resolve(session.clone(), request.title.clone(), created_at);
            let receipt = RichCreation {
                schema_version: 2,
                operation_key: key,
                namespace,
                request_id: request_id.to_string(),
                title: request.title,
                markdown: request.markdown,
                metadata,
                frozen_meta,
                files_created: false,
                completed: false,
            };
            if serde_json::to_vec_pretty(&receipt.frozen_meta)
                .map_err(|_| error(ErrorCode::Internal, "Metadata could not be serialized"))?
                .len()
                > limits.metadata_bytes
                || serde_json::to_vec(&receipt)
                    .map_err(|_| error(ErrorCode::Internal, "Creation could not be serialized"))?
                    .len()
                    > limits.recovery_bytes
            {
                return Err(error(
                    ErrorCode::ResourceLimit,
                    "Creation metadata or receipt exceeds its limit",
                ));
            }
            persist(&path, &receipt).await?;
            receipt
        };
        if receipt.completed {
            let current = store
                .read_meta(&session)
                .await
                .map_err(|cause| storage_error(cause, false))?
                .ok_or_else(|| {
                    error(ErrorCode::NotFound, "The created session no longer exists")
                })?;
            if current.id != session {
                return Err(error(
                    ErrorCode::Conflict,
                    "The created session binding changed",
                ));
            }
            return Ok(receipt.frozen_meta);
        }
        let committed = async {
            if !receipt.files_created {
                let current = store
                    .read_meta(&session)
                    .await
                    .map_err(|cause| storage_error(cause, false))?;
                if current
                    .as_ref()
                    .is_some_and(|meta| meta != &receipt.frozen_meta)
                {
                    return Err(error(
                        ErrorCode::Conflict,
                        "Unconfirmed creation metadata changed; review is required",
                    ));
                }
                if current.is_none() {
                    if !fresh
                        && store
                            .read_note(&session)
                            .await
                            .map_err(|cause| storage_error(cause, false))?
                            .is_none()
                    {
                        return Err(error(
                            ErrorCode::Conflict,
                            "Creation has no canonical evidence; review is required",
                        ));
                    }
                    store
                        .create_note_session(&receipt.frozen_meta, &receipt.markdown)
                        .await
                        .map_err(|cause| storage_error(cause, true))?;
                }
                receipt.files_created = true;
                persist(&path, &receipt).await?;
            } else {
                let current = store
                    .read_meta(&session)
                    .await
                    .map_err(|cause| storage_error(cause, false))?
                    .ok_or_else(|| {
                        error(ErrorCode::NotFound, "The created session no longer exists")
                    })?;
                if current.id != session {
                    return Err(error(
                        ErrorCode::Conflict,
                        "The created session binding changed",
                    ));
                }
            }
            self.finish_created(store, &receipt.frozen_meta).await?;
            receipt.completed = true;
            persist(&path, &receipt).await?;
            Ok(receipt.frozen_meta.clone())
        }
        .await;
        committed.map_err(|mut cause| {
            cause.message = format!(
                "Session {session} creation requires recovery: {}",
                cause.message
            );
            cause.mutation_outcome = Some(if receipt.files_created {
                MutationOutcome::Persisted
            } else {
                MutationOutcome::Unknown
            });
            cause
        })
    }
}

#[cfg(test)]
#[path = "rich_creation_tests.rs"]
mod tests;
