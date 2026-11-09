use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use hypr_loofah_core::{VaultRuntime, local_cli::LocalCli};
use hypr_loofah_local_owner::{
    Blob, EndpointRoot, OwnerClient, OwnerFuture, OwnerHandler, OwnerRequest, OwnerResolution,
    OwnerResponse, OwnerServer,
};
use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, LocalCliOperation, LocalCliOperationKind, LocalCliProgress,
    LocalCliResponse, MutationOutcome,
};

use crate::{Error, Result};

#[derive(Clone)]
pub(crate) struct LocalGateway {
    vault: Arc<PathBuf>,
    cache: hypr_search_cache::Cache,
}
impl LocalGateway {
    pub fn new(vault: PathBuf, cache: hypr_search_cache::Cache) -> Self {
        Self {
            vault: Arc::new(vault.canonicalize().unwrap_or(vault)),
            cache,
        }
    }
    pub fn vault(&self) -> &Path {
        &self.vault
    }

    pub async fn execute(&self, operation: LocalCliOperation) -> Result<OwnerResponse> {
        self.execute_with_upload(operation, None, Arc::new(|_| {}))
            .await
    }

    pub async fn execute_with_upload(
        &self,
        operation: LocalCliOperation,
        upload: Option<std::fs::File>,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> Result<OwnerResponse> {
        let request_id = uuid::Uuid::new_v4().to_string();
        let mut upload = upload;
        for attempt in 0..3 {
            let mut transport_lost = false;
            // Owner startup/recovery is a large state machine. Keep it out of
            // every enclosing command future on Windows' fixed main-thread stack.
            let response = Box::pin(self.execute_once(
                operation.clone(),
                upload.take(),
                progress.clone(),
                request_id.clone(),
                &mut transport_lost,
            ))
            .await;
            if !transport_lost || operation.is_mutation() || attempt == 2 {
                return response;
            }
            // A short-lived CLI owner may finish while another reader is awaiting
            // delivery. Re-resolve ownership; an unknown write is never repeated.
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        unreachable!("bounded Local owner read attempts")
    }

    async fn execute_once(
        &self,
        operation: LocalCliOperation,
        upload: Option<std::fs::File>,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
        request_id: String,
        transport_lost: &mut bool,
    ) -> Result<OwnerResponse> {
        let endpoints = self.endpoints()?;
        let temporary = endpoints.path().to_owned();
        let resolution = OwnerClient::resolve(&self.vault, &endpoints, &temporary)
            .await
            .map_err(owner_error)?;
        let (client, standalone) = match resolution {
            OwnerResolution::Connected(client) => (client, None),
            OwnerResolution::Acquired(lease) => {
                let (dispatcher, runtime) = if matches!(
                    operation,
                    LocalCliOperation::Doctor {} | LocalCliOperation::Initialize {}
                ) {
                    (
                        Dispatcher::Maintenance {
                            lease: lease.clone(),
                            cache: self.cache.clone(),
                        },
                        None,
                    )
                } else {
                    let key = hypr_storage::vault::canonical_vault_key(lease.canonical_vault());
                    let operations = hypr_storage::vault::local_operation_root(
                        &lease,
                        &self.app_local_data()?,
                        None,
                    )
                    .map_err(|cause| {
                        Error::operation("select local recovery state", cause.to_string())
                    })?;
                    if per_id_read(&operation.kind()) {
                        (
                            Dispatcher::Application(
                                LocalCli::open_read_owner(lease.clone(), operations).await?,
                            ),
                            None,
                        )
                    } else if per_id_mutation(&operation.kind()) {
                        let cache = self.ready_cache(lease.clone()).await?;
                        (
                            Dispatcher::Application(
                                LocalCli::open_session_owner(lease.clone(), operations, cache)
                                    .await?,
                            ),
                            None,
                        )
                    } else {
                        let cache = self.cache.clone();
                        let cache_owner = lease.clone();
                        let initializing = tokio::task::spawn_blocking(move || {
                            let _owner = cache_owner;
                            match cache.check() {
                                Ok(()) => Ok(false),
                                Err(hypr_search_cache::Error::NotReady) => Ok(true),
                                Err(cause) => Err(cause),
                            }
                        })
                        .await
                        .map_err(|cause| {
                            Error::operation("inspect local search", cause.to_string())
                        })??;
                        let started = std::time::Instant::now();
                        if initializing {
                            crate::output::startup_progress("initializing", started);
                        }
                        let runtime = if initializing {
                            VaultRuntime::open_leased(
                                format!("local:{key}"),
                                lease.clone(),
                                &self.cache_base()?,
                                &operations,
                                hypr_vault_write::ContentLimits::unlimited(),
                            )
                            .await
                        } else {
                            VaultRuntime::open_leased_with_ready_cache(
                                format!("local:{key}"),
                                lease.clone(),
                                &self.cache_base()?,
                                &operations,
                                hypr_vault_write::ContentLimits::unlimited(),
                                self.cache.clone(),
                            )
                            .await
                        }
                        .map_err(|cause| {
                            Error::operation("open local vault owner", cause.to_string())
                        })?;
                        runtime
                            .context
                            .enable_standalone_local_session_lifecycle(&lease)
                            .map_err(Error::local)?;
                        runtime
                            .context
                            .startup
                            .wait_until_ready()
                            .await
                            .map_err(|cause| Error::operation("prepare local vault", cause))?;
                        runtime
                            .context
                            .search
                            .wait_until_initialized()
                            .await
                            .map_err(|cause| Error::operation("prepare local search", cause))?;
                        if initializing {
                            crate::output::startup_progress("ready", started);
                        }
                        (
                            Dispatcher::Application(Arc::new(
                                LocalCli::new(runtime.context.clone())?.with_media_host(Arc::new(
                                    hypr_loofah_local_media::LocalMediaEngine::new(),
                                )),
                            )),
                            Some(runtime),
                        )
                    }
                };
                let host = Arc::new(CoreOwner {
                    dispatcher,
                    temporary: temporary.clone(),
                });
                let server = OwnerServer::bind(lease, &endpoints, &temporary, host)
                    .await
                    .map_err(owner_error)?;
                let standalone = Standalone {
                    server: Some(server),
                    runtime,
                };
                let client = match OwnerClient::resolve(&self.vault, &endpoints, &temporary)
                    .await
                    .map_err(owner_error)?
                {
                    OwnerResolution::Connected(client) => client,
                    OwnerResolution::Acquired(_) => {
                        return Err(Error::operation(
                            "connect local owner",
                            "New local owner unexpectedly released its vault",
                        ));
                    }
                };
                (client, Some(standalone))
            }
        };
        if !client.supports(&operation.kind()) {
            if let Some(owner) = standalone {
                owner.finish().await?;
            }
            return Err(Error::operation(
                "run local command",
                "This vault owner does not support this operation yet; no independent Store was opened",
            ));
        }
        let blob = match upload {
            Some(file) => Some(
                Blob::copy_from(file, &temporary)
                    .await
                    .map_err(owner_error)?,
            ),
            None => None,
        };
        let mutation = operation.is_mutation();
        let media_recovery = match &operation {
            LocalCliOperation::RecoverMedia { request_id } => Some(request_id.clone()),
            LocalCliOperation::Import { .. } | LocalCliOperation::Transcribe { .. } => {
                Some(request_id.clone())
            }
            _ => None,
        };
        let output = client
            .execute_with_progress(request_id.clone(), operation, blob, move |event| {
                progress(event)
            })
            .await
            .map_err(|cause| {
                *transport_lost=matches!(&cause,hypr_loofah_local_owner::Error::Io(cause) if matches!(cause.kind(), std::io::ErrorKind::UnexpectedEof|std::io::ErrorKind::BrokenPipe|std::io::ErrorKind::ConnectionReset|std::io::ErrorKind::ConnectionAborted|std::io::ErrorKind::NotConnected));
                let mut cause = application_owner_error(cause);
                if cause.mutation_outcome == Some(MutationOutcome::Unknown)
                    && let Some(original) = media_recovery
                {
                    cause.request_id = Some(original.clone());
                    cause.message.push_str(&format!(
                        "; run loof sessions recover {original}; inference will not be repeated"
                    ));
                }
                Error::local(cause)
            });
        let stopped = match standalone {
            Some(owner) => owner.finish().await,
            None => Ok(()),
        };
        let response = match (output, stopped) {
            (Err(cause), stopped) => {
                if let Err(stopping) = stopped {
                    eprintln!("error: {stopping}");
                }
                Err(cause)
            }
            (Ok(response), Err(stopping)) if response.response.result.is_err() => {
                eprintln!("error: {stopping}");
                Ok(response)
            }
            (Ok(_), Err(stopping)) => {
                let mut failure = hypr_loofah_client::failure(
                    ErrorCode::Internal,
                    &format!(
                        "Command completed, but stopping its temporary owner failed: {stopping}"
                    ),
                );
                failure.request_id = Some(request_id);
                failure.retryable = false;
                if mutation {
                    failure.mutation_outcome = Some(MutationOutcome::Persisted);
                }
                Err(Error::local(failure))
            }
            (Ok(response), Ok(())) => Ok(response),
        }?;
        if let Ok(hypr_loofah_protocol::LocalCliReply::WritePersisted {
            request_id,
            session_ids,
            reason,
        }) = &response.response.result
        {
            return Err(Error::WritePersisted {
                request_id: request_id.clone(),
                session_ids: session_ids.clone(),
                reason: reason.clone(),
            });
        }
        if let Err(cause) = &response.response.result {
            return Err(Error::local(cause.clone()));
        }
        Ok(response)
    }

    async fn ready_cache(
        &self,
        owner: hypr_storage::vault::VaultLease,
    ) -> Result<Option<hypr_search_cache::Cache>> {
        let cache = self.cache.clone();
        tokio::task::spawn_blocking(move || {
            let _owner = owner;
            match cache.check() {
                Ok(()) => Ok(Some(cache)),
                Err(hypr_search_cache::Error::NotReady) => Ok(None),
                Err(cause) => Err(Error::Cache(cause)),
            }
        })
        .await
        .map_err(|cause| Error::operation("inspect local search", cause.to_string()))?
    }

    fn cache_base(&self) -> Result<PathBuf> {
        self.cache
            .path()
            .parent()
            .and_then(Path::parent)
            .map(Path::to_owned)
            .ok_or_else(|| Error::operation("prepare local cache", "Invalid cache location"))
    }
    fn app_local_data(&self) -> Result<PathBuf> {
        #[cfg(test)]
        {
            Ok(self.cache_base()?.join("app-local"))
        }
        #[cfg(not(test))]
        {
            dirs::data_local_dir().ok_or_else(|| {
                Error::operation(
                    "resolve local application storage",
                    "Application-data directory is unavailable",
                )
            })
        }
    }
    fn endpoints(&self) -> Result<EndpointRoot> {
        #[cfg(test)]
        {
            let base = std::env::var_os("LOOFAH_OWNER_TEST_ROOT")
                .or_else(|| std::env::var_os("RUNNER_TEMP"))
                .map(PathBuf::from)
                .unwrap_or_else(std::env::temp_dir);
            EndpointRoot::new(base.join("loofah-cli-owner-tests")).map_err(owner_error)
        }
        #[cfg(not(test))]
        {
            EndpointRoot::system().map_err(owner_error)
        }
    }
}

fn per_id_mutation(kind: &LocalCliOperationKind) -> bool {
    matches!(
        kind,
        LocalCliOperationKind::SaveMetadata
            | LocalCliOperationKind::SessionLifecycle
            | LocalCliOperationKind::DeleteSession
    )
}

fn per_id_read(kind: &LocalCliOperationKind) -> bool {
    use LocalCliOperationKind::*;
    matches!(
        kind,
        AgentMeeting
            | AgentTranscript
            | AgentExport
            | AgentSegments
            | Metadata
            | Note
            | SessionPath
            | Attachment
            | AttachmentPage
    )
}

#[derive(Clone)]
enum Dispatcher {
    Application(Arc<LocalCli>),
    Maintenance {
        lease: hypr_storage::vault::VaultLease,
        cache: hypr_search_cache::Cache,
    },
}
struct CoreOwner {
    dispatcher: Dispatcher,
    temporary: PathBuf,
}
impl OwnerHandler for CoreOwner {
    fn operations(&self) -> Vec<LocalCliOperationKind> {
        match &self.dispatcher {
            Dispatcher::Application(dispatcher) => dispatcher.operations(),
            Dispatcher::Maintenance { .. } => vec![
                LocalCliOperationKind::Doctor,
                LocalCliOperationKind::Initialize,
            ],
        }
    }
    fn execute(&self, request: OwnerRequest) -> OwnerFuture {
        let dispatcher = self.dispatcher.clone();
        let temporary = self.temporary.clone();
        Box::pin(async move {
            let OwnerRequest {
                request_id,
                operation,
                blob,
                progress,
            } = request;
            let result = async {
                if let Dispatcher::Application(dispatcher) = dispatcher {
                    return Ok(
                        hypr_loofah_local_owner::CoreOwner::new(dispatcher, temporary)
                            .execute(OwnerRequest {
                                request_id,
                                operation,
                                blob,
                                progress,
                            })
                            .await,
                    );
                }
                let Dispatcher::Maintenance { lease, cache } = dispatcher else {
                    unreachable!()
                };
                if blob.is_some() {
                    return Err(hypr_loofah_client::failure(
                        ErrorCode::Validation,
                        "Maintenance does not accept an upload",
                    ));
                }
                let progress = Arc::new(move |event| progress.report(event));
                let result = hypr_loofah_core::local_cli::execute_local_maintenance(
                    lease, cache, operation, progress,
                )
                .await?;
                Ok::<_, ApplicationError>(OwnerResponse {
                    response: LocalCliResponse {
                        result: Ok(result.reply),
                    },
                    blob: None,
                })
            }
            .await;
            match result {
                Ok(response) => response,
                Err(cause) => OwnerResponse {
                    response: LocalCliResponse { result: Err(cause) },
                    blob: None,
                },
            }
        })
    }
}

struct Standalone {
    server: Option<OwnerServer>,
    runtime: Option<VaultRuntime>,
}
impl Standalone {
    async fn finish(mut self) -> Result<()> {
        let server = self.server.take().unwrap();
        let runtime = self.runtime.take();
        tokio::spawn(async move { drain(server, runtime).await })
            .await
            .map_err(|_| Error::operation("stop local owner", "Local owner drain stopped"))?
    }
}
impl Drop for Standalone {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            let runtime = self.runtime.take();
            tokio::spawn(async move {
                if let Err(cause) = drain(server, runtime).await {
                    eprintln!("error: {cause}");
                }
            });
        }
    }
}
async fn drain(server: OwnerServer, runtime: Option<VaultRuntime>) -> Result<()> {
    let server_result = server.shutdown().await;
    if let Some(runtime) = runtime {
        let startup = runtime.context.startup.clone();
        let runtime_result = runtime.shutdown().await;
        if let Err(cause) = runtime_result {
            eprintln!("error: local vault drain remains active: {cause}");
            startup
                .wait_until_stopped()
                .await
                .map_err(|cause| Error::operation("finish local vault shutdown", cause))?;
        }
    }
    server_result.map_err(owner_error)
}

fn application_owner_error(cause: hypr_loofah_local_owner::Error) -> ApplicationError {
    let mut error = hypr_loofah_client::failure(ErrorCode::Unavailable, &cause.to_string());
    if let hypr_loofah_local_owner::Error::Unknown { request_id } = cause {
        error.request_id = Some(request_id);
        error.mutation_outcome = Some(MutationOutcome::Unknown);
        error.retryable = false;
    }
    error
}
fn owner_error(cause: hypr_loofah_local_owner::Error) -> Error {
    Error::local(application_owner_error(cause))
}

#[cfg(test)]
mod tests {
    use super::*;
    use hypr_storage::vault::{LeaseAttempt, VaultLease};

    fn gateway(root: &tempfile::TempDir) -> LocalGateway {
        let vault = root.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let cache = hypr_search_cache::Cache::new(&root.path().join("cache"), &vault).unwrap();
        LocalGateway::new(vault, cache)
    }

    #[tokio::test]
    async fn doctor_does_not_initialize_search_config_or_normalize_legacy_sessions() {
        let root = tempfile::tempdir().unwrap();
        let gateway = gateway(&root);
        let source = gateway.vault().join("sessions/legacy folder");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("_meta.json"), r#"{"id":"canonical-id","title":"Legacy","created_at":"2026-11-05T00:00:00Z","tags":[]}"#).unwrap();
        let response = gateway.execute(LocalCliOperation::Doctor {}).await.unwrap();
        assert!(matches!(
            response.response.result.unwrap(),
            hypr_loofah_protocol::LocalCliReply::Doctor {
                cache_ready: false,
                ..
            }
        ));
        assert!(source.join("_meta.json").exists());
        assert!(!gateway.vault().join("sessions/canonical-id").exists());
        assert!(!gateway.vault().join("config.json").exists());
        assert!(!gateway.cache.path().exists());
        assert!(matches!(
            VaultLease::try_acquire_existing(gateway.vault()).unwrap(),
            LeaseAttempt::Acquired(_)
        ));
    }

    #[tokio::test]
    async fn explicit_initialize_reports_the_first_projection_and_releases_owner() {
        let root = tempfile::tempdir().unwrap();
        let gateway = gateway(&root);
        let session = gateway.vault().join("sessions/first");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(
            session.join("_meta.json"),
            r#"{"id":"first","title":"First","created_at":"2026-11-05T00:00:00Z","tags":[]}"#,
        )
        .unwrap();
        let response = gateway
            .execute(LocalCliOperation::Initialize {})
            .await
            .unwrap();
        assert!(matches!(
            response.response.result.unwrap(),
            hypr_loofah_protocol::LocalCliReply::Initialized {
                added: 1,
                sessions: 1,
                ..
            }
        ));
        assert!(matches!(
            VaultLease::try_acquire_existing(gateway.vault()).unwrap(),
            LeaseAttempt::Acquired(_)
        ));
    }

    #[tokio::test]
    async fn held_owner_without_endpoint_never_initializes_independent_cache() {
        let root = tempfile::tempdir().unwrap();
        let gateway = gateway(&root);
        let LeaseAttempt::Acquired(_owner) =
            VaultLease::try_acquire_existing(gateway.vault()).unwrap()
        else {
            panic!("owner");
        };
        assert!(
            gateway
                .execute(LocalCliOperation::Initialize {})
                .await
                .is_err()
        );
        assert!(!gateway.cache.path().exists());
        assert!(!gateway.vault().join("config.json").exists());
    }

    #[test]
    fn idle_agent_reader_retains_no_vault_owner() {
        let root = tempfile::tempdir().unwrap();
        let gateway = gateway(&root);
        let _reader = crate::agent_reader::AgentReader::local(
            gateway.vault().to_owned(),
            gateway.cache.clone(),
        );
        assert!(matches!(
            VaultLease::try_acquire_existing(gateway.vault()).unwrap(),
            LeaseAttempt::Acquired(_)
        ));
        assert!(!gateway.cache.path().exists());
    }

    struct ClosingOwner {
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl OwnerHandler for ClosingOwner {
        fn operations(&self) -> Vec<LocalCliOperationKind> {
            vec![LocalCliOperationKind::Note, LocalCliOperationKind::SaveNote]
        }
        fn execute(&self, request: OwnerRequest) -> OwnerFuture {
            let entered = self.entered.clone();
            let release = self.release.clone();
            let calls = self.calls.clone();
            Box::pin(async move {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                entered.notify_one();
                release.notified().await;
                let snapshot = hypr_loofah_protocol::NoteSnapshot {
                    markdown: Some("Old owner reply".into()),
                    title: "Owner".into(),
                    revision: "r".into(),
                };
                let reply = if request.operation.is_mutation() {
                    hypr_loofah_protocol::LocalCliReply::NoteSaved {
                        outcome: hypr_loofah_protocol::NoteSaveOutcome::Saved(snapshot),
                    }
                } else {
                    hypr_loofah_protocol::LocalCliReply::Note { snapshot }
                };
                OwnerResponse {
                    response: LocalCliResponse { result: Ok(reply) },
                    blob: None,
                }
            })
        }
    }
    #[tokio::test]
    async fn temporary_owner_delivery_loss_reresolves_reads_but_never_replays_unknown_writes() {
        for mutation in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let gateway = gateway(&root);
            let session = gateway.vault().join("sessions/session");
            std::fs::create_dir_all(&session).unwrap();
            std::fs::write(session.join("_meta.json"),r#"{"id":"session","title":"Original","created_at":"2026-11-08T00:00:00Z","tags":[]}"#).unwrap();
            std::fs::write(session.join("notes.md"), "Canonical note").unwrap();
            let LeaseAttempt::Acquired(lease) =
                VaultLease::try_acquire_existing(gateway.vault()).unwrap()
            else {
                panic!("owner");
            };
            let entered = Arc::new(tokio::sync::Notify::new());
            let release = Arc::new(tokio::sync::Notify::new());
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let endpoints = gateway.endpoints().unwrap();
            let server = OwnerServer::bind(
                lease,
                &endpoints,
                endpoints.path(),
                Arc::new(ClosingOwner {
                    entered: entered.clone(),
                    release: release.clone(),
                    calls: calls.clone(),
                }),
            )
            .await
            .unwrap();
            let caller = gateway.clone();
            let request = tokio::spawn(async move {
                caller
                    .execute(if mutation {
                        LocalCliOperation::SaveNote {
                            session_id: "session".into(),
                            input: hypr_loofah_protocol::SaveNoteRequest {
                                markdown: "Should not replay".into(),
                                title: None,
                                expected_revision: "r".into(),
                            },
                        }
                    } else {
                        LocalCliOperation::Note {
                            session_id: "session".into(),
                        }
                    })
                    .await
            });
            tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
                .await
                .unwrap();
            server.request_shutdown();
            release.notify_one();
            server.wait_for_shutdown().await.unwrap();
            let result = tokio::time::timeout(std::time::Duration::from_secs(10), request)
                .await
                .unwrap()
                .unwrap();
            if mutation {
                let Error::Local(error) = result.err().expect("unknown write") else {
                    panic!("typed owner error");
                };
                assert_eq!(error.mutation_outcome, Some(MutationOutcome::Unknown));
                assert!(error.request_id.is_some());
            } else {
                let hypr_loofah_protocol::LocalCliReply::Note { snapshot } =
                    result.unwrap().response.result.unwrap()
                else {
                    panic!("note");
                };
                assert_eq!(snapshot.markdown.as_deref(), Some("Canonical note"));
            }
            assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
            assert_eq!(
                std::fs::read_to_string(session.join("notes.md")).unwrap(),
                "Canonical note"
            );
            assert!(!gateway.cache.path().exists());
        }
    }

    struct NoteOwner;
    impl OwnerHandler for NoteOwner {
        fn operations(&self) -> Vec<LocalCliOperationKind> {
            vec![LocalCliOperationKind::Note]
        }
        fn execute(&self, _: OwnerRequest) -> OwnerFuture {
            Box::pin(async {
                OwnerResponse {
                    response: LocalCliResponse {
                        result: Ok(hypr_loofah_protocol::LocalCliReply::Note {
                            snapshot: hypr_loofah_protocol::NoteSnapshot {
                                markdown: Some("From live desktop owner".into()),
                                title: "Owner".into(),
                                revision: "owner-revision".into(),
                            },
                        }),
                    },
                    blob: None,
                }
            })
        }
    }
    #[tokio::test]
    async fn current_owner_handles_read_without_cli_startup_or_cache_writes() {
        let root = tempfile::tempdir().unwrap();
        let gateway = gateway(&root);
        let LeaseAttempt::Acquired(lease) =
            VaultLease::try_acquire_existing(gateway.vault()).unwrap()
        else {
            panic!("owner");
        };
        let endpoints = gateway.endpoints().unwrap();
        let server = OwnerServer::bind(lease, &endpoints, endpoints.path(), Arc::new(NoteOwner))
            .await
            .unwrap();
        let response = gateway
            .execute(LocalCliOperation::Note {
                session_id: "session".into(),
            })
            .await
            .unwrap();
        let hypr_loofah_protocol::LocalCliReply::Note { snapshot } =
            response.response.result.unwrap()
        else {
            panic!("note");
        };
        assert_eq!(
            snapshot.markdown.as_deref(),
            Some("From live desktop owner")
        );
        assert!(!gateway.cache.path().exists());
        assert!(!gateway.vault().join("config.json").exists());
        server.shutdown().await.unwrap();
    }
}
