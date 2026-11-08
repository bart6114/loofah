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
    pub(crate) root: PathBuf,
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

    pub fn session_page(
        &self,
        limit: usize,
        after: Option<&str>,
    ) -> Result<hypr_loofah_protocol::SessionPage, ApplicationError> {
        self.store
            .ensure_ready()
            .map_err(|_| error(ErrorCode::Unavailable, "Vault is not ready"))?;
        if !(1..=200).contains(&limit) || after.is_some_and(|cursor| cursor.len() > 512) {
            return Err(error(
                ErrorCode::Validation,
                "Invalid session page size or cursor",
            ));
        }
        let mut sessions: Vec<_> = self
            .store
            .session_list()
            .into_iter()
            .map(|entry| entry.meta)
            .collect();
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        sessions.retain(|session| after.is_none_or(|after| session.id.as_str() > after));
        let next_cursor = (sessions.len() > limit).then(|| sessions[limit - 1].id.clone());
        sessions.truncate(limit);
        Ok(hypr_loofah_protocol::SessionPage {
            sessions,
            next_cursor,
        })
    }

    pub async fn session_document(&self, id: &str) -> Result<SessionDocument, ApplicationError> {
        self.validate(id)?;
        self.store
            .read_recovered_session_document(self.document_operation_root().as_deref(), id)
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
        let limits = self.store.content_limits();
        if request.markdown.len() > limits.note_write_bytes
            || request.title.len() > limits.title_bytes
        {
            return Err(error(
                ErrorCode::ResourceLimit,
                "Note exceeds the size limit",
            ));
        }
        let context = self.clone();
        // A lost response must not cancel a file commit between its durable intent and receipt.
        tokio::spawn(async move {
            let _admission = context
                .admit_session_work(&creation_identity(&namespace, request_id).1)
                .await?;
            let operations = context.operations.as_ref().ok_or_else(|| {
                error(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                )
            })?;
            operations
                .create(&context.store, &namespace, request_id, request, None)
                .await
        })
        .await
        .map_err(|cause| {
            tracing::error!(%cause, "session creation worker failed");
            error(ErrorCode::Internal, "Session creation worker failed")
        })?
    }

    pub(crate) async fn create_capture_session(
        self: &Arc<Self>,
        namespace: String,
        request_id: uuid::Uuid,
        observed_at_ms: u64,
    ) -> Result<SessionDocument, ApplicationError> {
        self.store
            .ensure_ready()
            .map_err(|cause| storage_error(cause, false))?;
        let observed_at_ms = i64::try_from(observed_at_ms)
            .ok()
            .filter(|value| *value <= 9_007_199_254_740_991)
            .and_then(chrono::DateTime::<chrono::Utc>::from_timestamp_millis)
            .ok_or_else(|| {
                error(
                    ErrorCode::Validation,
                    "Capture producer observation cannot create a session",
                )
            })?;
        let created_at = observed_at_ms.to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let context = self.clone();
        tokio::spawn(async move {
            let _admission = context
                .admit_session_work(&creation_identity(&namespace, request_id).1)
                .await?;
            let operations = context.operations.as_ref().ok_or_else(|| {
                error(
                    ErrorCode::Unavailable,
                    "Operation storage is not configured",
                )
            })?;
            operations
                .create(
                    &context.store,
                    &namespace,
                    request_id,
                    CreateSessionRequest {
                        title: String::new(),
                        markdown: String::new(),
                    },
                    Some(created_at),
                )
                .await
        })
        .await
        .map_err(|_| {
            let mut cause = error(
                ErrorCode::Internal,
                "Capture session creation outcome is unknown",
            );
            cause.mutation_outcome = Some(MutationOutcome::Unknown);
            cause
        })?
    }
}

pub(crate) fn creation_identity(namespace: &str, request_id: uuid::Uuid) -> (String, String) {
    let hash = Sha256::digest(serde_json::to_vec(&(namespace, request_id.to_string())).unwrap());
    let key: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();
    let mut id: [u8; 16] = hash[..16].try_into().unwrap();
    id[6] = (id[6] & 15) | 64;
    id[8] = (id[8] & 63) | 128;
    (key, uuid::Uuid::from_bytes(id).to_string())
}

impl SessionOperations {
    async fn create(
        &self,
        store: &SessionStore,
        namespace: &str,
        request_id: uuid::Uuid,
        request: CreateSessionRequest,
        created_at: Option<String>,
    ) -> Result<SessionDocument, ApplicationError> {
        let _guard = self.lock.lock().await;
        let (key, session_id) = creation_identity(namespace, request_id);
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
            if stored.schema_version != 1
                || stored.request != request
                || stored.session_id != session_id
                || created_at
                    .as_ref()
                    .is_some_and(|expected| expected != &stored.created_at)
            {
                return Err(error(
                    ErrorCode::Conflict,
                    "Request ID was used with different input or an unsupported receipt",
                ));
            }
            if stored.response.is_some() {
                return store
                    .read_recovered_session_document(
                        Some(&self.root.join("document-saves")),
                        &stored.session_id,
                    )
                    .await
                    .map_err(|cause| storage_error(cause, false))?
                    .ok_or_else(|| {
                        error(ErrorCode::NotFound, "The created session no longer exists")
                    });
            }
            stored
        } else {
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
                created_at: created_at.unwrap_or_else(|| chrono::Utc::now().to_rfc3339()),
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
            .reconcile_note_tasks(&self.root.join("document-saves"), &meta.id)
            .await
            .map_err(|cause| {
                let mut error = storage_error(cause, false);
                error.mutation_outcome = Some(MutationOutcome::Unknown);
                error
            })?
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
    async fn open_runtime(
        id: String,
        vault: &std::path::Path,
        cache: &std::path::Path,
        operations: &std::path::Path,
    ) -> Result<crate::VaultRuntime, hypr_vault_write::StoreError> {
        crate::VaultRuntime::open_with_changes(
            id,
            vault,
            cache,
            operations,
            tokio::sync::mpsc::unbounded_channel().1,
        )
        .await
    }
    use super::*;
    use hypr_loofah_protocol::NoteSaveOutcome;

    #[tokio::test]
    async fn created_note_tasks_are_initialized_once_and_checked_saves_preserve_their_ids() {
        let vault = tempfile::tempdir().unwrap();
        let runtime_root = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let runtime = open_runtime(
            "private".into(),
            vault.path(),
            cache.path(),
            runtime_root.path(),
        )
        .await
        .unwrap();
        let request_id = uuid::Uuid::new_v4().to_string();
        let input = CreateSessionRequest {
            title: "Task note".into(),
            markdown: "- [ ] First task".into(),
        };
        let created = runtime
            .context
            .create_session("alice".into(), request_id.clone(), input.clone())
            .await
            .unwrap();
        let tasks = runtime
            .context
            .session_tasks(&created.meta.id)
            .await
            .unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].text, "First task");
        assert_eq!(tasks[0].status, "todo");
        runtime
            .context
            .create_session("alice".into(), request_id, input)
            .await
            .unwrap();
        assert_eq!(
            runtime
                .context
                .session_tasks(&created.meta.id)
                .await
                .unwrap(),
            tasks
        );
        let NoteSaveOutcome::Saved(saved) = runtime
            .context
            .save_note(
                &created.meta.id,
                hypr_loofah_protocol::SaveNoteRequest {
                    markdown: "- [x] First task".into(),
                    title: None,
                    expected_revision: created.note.revision,
                },
            )
            .await
            .unwrap()
        else {
            panic!("expected saved note")
        };
        let updated = runtime
            .context
            .session_tasks(&created.meta.id)
            .await
            .unwrap();
        assert_eq!(updated.len(), 1);
        assert_eq!(updated[0].id, tasks[0].id);
        assert_eq!(updated[0].status, "done");
        assert_eq!(
            runtime
                .context
                .session_document(&created.meta.id)
                .await
                .unwrap()
                .note,
            saved
        );
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn session_pages_share_bounded_ordering_and_enforce_startup_readiness() {
        let vault = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let operations = tempfile::tempdir().unwrap();
        let runtime = open_runtime(
            "private".into(),
            vault.path(),
            cache.path(),
            operations.path(),
        )
        .await
        .unwrap();
        for id in ["c", "a", "b"] {
            let meta = serde_json::from_value(serde_json::json!({"id":id,"title":id,"created_at":"2026-11-05T10:00:00Z","tags":[]})).unwrap();
            runtime.context.store.write_meta(&meta).await.unwrap();
        }
        let first = runtime.context.session_page(2, None).unwrap();
        assert_eq!(
            first
                .sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(first.next_cursor.as_deref(), Some("b"));
        let last = runtime
            .context
            .session_page(2, first.next_cursor.as_deref())
            .unwrap();
        assert_eq!(
            last.sessions
                .iter()
                .map(|session| session.id.as_str())
                .collect::<Vec<_>>(),
            ["c"]
        );
        assert!(last.next_cursor.is_none());
        for limit in [0, 201] {
            assert_eq!(
                runtime.context.session_page(limit, None).unwrap_err().code,
                ErrorCode::Validation
            );
        }
        runtime.context.store.set_startup_pending(true);
        assert_eq!(
            runtime.context.session_page(2, None).unwrap_err().code,
            ErrorCode::Unavailable
        );
        runtime.context.store.set_startup_pending(false);
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn completed_receipts_do_not_override_current_content_or_resurrect_deleted_sessions() {
        let vault = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let operations = tempfile::tempdir().unwrap();
        let runtime = open_runtime(
            "private".into(),
            vault.path(),
            cache.path(),
            operations.path(),
        )
        .await
        .unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let input = CreateSessionRequest {
            title: "Original".into(),
            markdown: "original".into(),
        };
        let created = runtime
            .context
            .create_session("alice".into(), id.clone(), input.clone())
            .await
            .unwrap();
        runtime
            .context
            .save_note(
                &created.meta.id,
                hypr_loofah_protocol::SaveNoteRequest {
                    markdown: "edited later".into(),
                    title: Some("Updated".into()),
                    expected_revision: created.note.revision,
                },
            )
            .await
            .unwrap();
        let replayed = runtime
            .context
            .create_session("alice".into(), id.clone(), input.clone())
            .await
            .unwrap();
        assert_eq!(replayed.note.markdown.as_deref(), Some("edited later"));
        assert_eq!(replayed.meta.title, "Updated");
        runtime
            .context
            .store
            .delete_session(&created.meta.id)
            .await
            .unwrap();
        assert_eq!(
            runtime
                .context
                .create_session("alice".into(), id, input)
                .await
                .unwrap_err()
                .code,
            ErrorCode::NotFound
        );
        assert_eq!(runtime.context.store.session_count(), 0);
        runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn creation_retry_and_recovery_never_duplicate_or_overwrite_notes() {
        let vault = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let operations = tempfile::tempdir().unwrap();
        let runtime = open_runtime(
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
        let runtime = open_runtime(
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
            let entry = entry.unwrap();
            if !entry.file_type().unwrap().is_file() {
                continue;
            }
            let path = entry.path();
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
        let runtime = open_runtime(
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
