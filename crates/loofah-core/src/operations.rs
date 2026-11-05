use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use hypr_loofah_protocol::{
    ApplicationError, CreateSessionRequest, ErrorCode, MutationOutcome, SessionDocument,
};
use hypr_vault_write::{SessionMeta, SessionStore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    VaultContext,
    vault::{error, storage_error},
};

pub(crate) struct SessionOperations {
    root: PathBuf,
    lock: tokio::sync::Mutex<()>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Creation {
    schema_version: u32,
    request: CreateSessionRequest,
    session_id: String,
    created_at: String,
    response: Option<SessionDocument>,
}

impl VaultContext {
    pub fn with_operation_storage(mut self, root: PathBuf) -> Self {
        self.operations = Some(SessionOperations {
            root,
            lock: tokio::sync::Mutex::new(()),
        });
        self
    }

    pub async fn session_document(&self, id: &str) -> Result<SessionDocument, ApplicationError> {
        self.validate(id)?;
        self.store
            .read_session_document(id)
            .await
            .map_err(|cause| storage_error(cause, false))?
            .ok_or_else(|| error(ErrorCode::NotFound, "Session not found"))
    }

    pub async fn create_session(
        self: &Arc<Self>,
        namespace: String,
        request_id: String,
        request: CreateSessionRequest,
    ) -> Result<SessionDocument, ApplicationError> {
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is not ready"))?;
        let request_id = uuid::Uuid::parse_str(&request_id)
            .map_err(|_| error(ErrorCode::Validation, "A UUID request ID is required"))?;
        if request.markdown.len() > 4 * 1024 * 1024 || request.title.len() > 4096 {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Note exceeds the size limit",
            ));
        }
        let context = self.clone();
        // A lost response must not cancel a file commit between its durable intent and receipt.
        tokio::spawn(async move {
            let operations = context.operations.as_ref().ok_or_else(|| {
                error(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                )
            })?;
            operations
                .create(&context.store, &namespace, request_id, request)
                .await
        })
        .await
        .map_err(|cause| {
            tracing::error!(%cause, "session creation worker failed");
            error(ErrorCode::Internal, "Session creation worker failed")
        })?
    }
}

impl SessionOperations {
    async fn create(
        &self,
        store: &SessionStore,
        namespace: &str,
        request_id: uuid::Uuid,
        request: CreateSessionRequest,
    ) -> Result<SessionDocument, ApplicationError> {
        let _guard = self.lock.lock().await;
        let hash =
            Sha256::digest(serde_json::to_vec(&(namespace, request_id.to_string())).unwrap());
        let key: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
        let path = self.root.join(format!("{key}.json"));
        let read_path = path.clone();
        let stored = tokio::task::spawn_blocking(move || match std::fs::read(read_path) {
            Ok(bytes) => serde_json::from_slice::<Creation>(&bytes)
                .map(Some)
                .map_err(|cause| {
                    tracing::error!(%cause, "invalid operation receipt");
                    error(ErrorCode::Internal, "Operation receipt is invalid")
                }),
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(cause) => Err(io_error(cause)),
        })
        .await
        .map_err(|_| error(ErrorCode::Internal, "Operation receipt read failed"))??;
        let mut creation = if let Some(stored) = stored {
            if stored.schema_version != 1 || stored.request != request {
                return Err(error(
                    ErrorCode::Conflict,
                    "Request ID was used with different input or an unsupported receipt",
                ));
            }
            if let Some(response) = stored.response {
                return Ok(response);
            }
            stored
        } else {
            let mut id: [u8; 16] = hash[..16].try_into().unwrap();
            id[6] = (id[6] & 15) | 64;
            id[8] = (id[8] & 63) | 128;
            let session_id = uuid::Uuid::from_bytes(id).to_string();
            if store
                .read_meta(&session_id)
                .await
                .map_err(|cause| storage_error(cause, false))?
                .is_some()
            {
                return Err(error(
                    ErrorCode::Conflict,
                    "Creation receipt is missing for an existing session; reload the vault",
                ));
            }
            let creation = Creation {
                schema_version: 1,
                request,
                session_id,
                created_at: chrono::Utc::now().to_rfc3339(),
                response: None,
            };
            persist(&path, &creation).await?;
            creation
        };
        let meta = SessionMeta {
            id: creation.session_id.clone(),
            title: creation.request.title.clone(),
            created_at: creation.created_at.clone(),
            started_at: None,
            ended_at: None,
            tags: vec![],
            tag_suggestions: None,
            tracking_id: None,
            folder: None,
            author: None,
            skill: None,
            extra: Default::default(),
        };
        if store
            .read_meta(&meta.id)
            .await
            .map_err(|cause| storage_error(cause, false))?
            .is_none()
        {
            store
                .create_note_session(&meta, &creation.request.markdown)
                .await
                .map_err(|cause| storage_error(cause, true))?;
        }
        let response = store
            .read_session_document(&meta.id)
            .await
            .map_err(|cause| storage_error(cause, false))?
            .ok_or_else(|| error(ErrorCode::Internal, "Session creation did not persist"))?;
        creation.response = Some(response.clone());
        persist(&path, &creation).await?;
        Ok(response)
    }
}

async fn persist(path: &Path, creation: &Creation) -> Result<(), ApplicationError> {
    let content = serde_json::to_string(creation).map_err(|_| {
        error(
            ErrorCode::Internal,
            "Operation receipt could not be serialized",
        )
    })?;
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        hypr_storage::fs::atomic_write(&path, &content).map_err(io_error)?;
        #[cfg(unix)]
        std::fs::File::open(path.parent().unwrap())
            .and_then(|parent| parent.sync_all())
            .map_err(io_error)?;
        Ok(())
    })
    .await
    .map_err(|_| error(ErrorCode::Internal, "Operation receipt write failed"))?
}

fn io_error(cause: std::io::Error) -> ApplicationError {
    tracing::error!(%cause, "operation receipt I/O failed");
    let mut result = error(ErrorCode::Internal, "Operation persistence failed");
    result.mutation_outcome = Some(MutationOutcome::Unknown);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VaultRuntime;

    #[tokio::test]
    async fn creation_retry_and_recovery_never_duplicate_or_overwrite_notes() {
        let vault = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let operations = tempfile::tempdir().unwrap();
        let runtime = VaultRuntime::open(
            "private".into(),
            vault.path(),
            cache.path(),
            operations.path(),
        )
        .await
        .unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        let request = CreateSessionRequest {
            title: "Created once".into(),
            markdown: "durable body".into(),
        };
        let (first, retry) = tokio::join!(
            runtime
                .context
                .create_session("alice".into(), request_id.clone(), request.clone()),
            runtime
                .context
                .create_session("alice".into(), request_id.clone(), request.clone()),
        );
        let first = first.unwrap();
        assert_eq!(retry.unwrap(), first);
        assert_eq!(runtime.context.store.session_count(), 1);
        let mut different = request.clone();
        different.markdown = "different".into();
        assert_eq!(
            runtime
                .context
                .create_session("alice".into(), request_id.clone(), different)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        let other = runtime
            .context
            .create_session("bob".into(), request_id.clone(), request.clone())
            .await
            .unwrap();
        assert_ne!(other.meta.id, first.meta.id);
        runtime.shutdown().await.unwrap();
        let runtime = VaultRuntime::open(
            "private".into(),
            vault.path(),
            cache.path(),
            operations.path(),
        )
        .await
        .unwrap();
        assert_eq!(
            runtime
                .context
                .create_session("alice".into(), request_id.clone(), request.clone())
                .await
                .unwrap(),
            first
        );
        runtime.shutdown().await.unwrap();
        // Reproduce a crash after the note landed but before metadata and the response receipt.
        for entry in std::fs::read_dir(operations.path()).unwrap() {
            let path = entry.unwrap().path();
            let mut receipt: Creation =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            if receipt.session_id == first.meta.id {
                receipt.response = None;
                std::fs::write(path, serde_json::to_vec(&receipt).unwrap()).unwrap();
            }
        }
        let directory = vault.path().join("sessions").join(&first.meta.id);
        std::fs::remove_file(directory.join("_meta.json")).unwrap();
        std::fs::write(directory.join("user-attachment.pdf"), "owned by user").unwrap();
        let runtime = VaultRuntime::open(
            "private".into(),
            vault.path(),
            cache.path(),
            operations.path(),
        )
        .await
        .unwrap();
        assert_eq!(
            runtime
                .context
                .create_session("alice".into(), request_id, request)
                .await
                .unwrap(),
            first
        );
        assert_eq!(
            std::fs::read_to_string(directory.join("user-attachment.pdf")).unwrap(),
            "owned by user"
        );
        assert_eq!(runtime.context.store.session_count(), 2);
        runtime.shutdown().await.unwrap();
    }
}
