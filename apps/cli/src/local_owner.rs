use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use hypr_loofah_core::{
    VaultRuntime,
    local_cli::{LocalCli, LocalCliArtifact},
};
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
                    } else {
                        let runtime = VaultRuntime::open_leased(
                            format!("local:{key}"),
                            lease.clone(),
                            &self.cache_base()?,
                            &operations,
                            hypr_vault_write::ContentLimits::unlimited(),
                        )
                        .await
                        .map_err(|cause| {
                            Error::operation("open local vault owner", cause.to_string())
                        })?;
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
                        (
                            Dispatcher::Application(Arc::new(LocalCli::new(
                                runtime.context.clone(),
                            )?)),
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
        let request_id = uuid::Uuid::new_v4().to_string();
        let mutation = operation.is_mutation();
        let output = client
            .execute_with_progress(request_id.clone(), operation, blob, move |event| {
                progress(event)
            })
            .await
            .map_err(owner_error);
        let stopped = match standalone {
            Some(owner) => owner.finish().await,
            None => Ok(()),
        };
        match (output, stopped) {
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
                Err(failure.into())
            }
            (Ok(response), Ok(())) => Ok(response),
        }
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
            return Ok(self.cache_base()?.join("app-local"));
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
            return EndpointRoot::new(base.join("loofah-cli-owner-tests")).map_err(owner_error);
        }
        #[cfg(not(test))]
        {
            EndpointRoot::system().map_err(owner_error)
        }
    }
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
                let input = match blob {
                    Some(blob) => {
                        let size = blob.len();
                        let sha = blob.sha256().to_owned();
                        let path = blob.trusted_path().map_err(application_owner_error)?;
                        let file = blob
                            .with_file(|file| Ok(file.try_clone()?))
                            .await
                            .map_err(application_owner_error)?;
                        Some(LocalCliArtifact::new(
                            file,
                            size,
                            sha,
                            Some(path),
                            Box::new(blob),
                        ))
                    }
                    None => None,
                };
                let progress = Arc::new(move |event| progress.report(event));
                let result = match dispatcher {
                    Dispatcher::Application(dispatcher) => {
                        dispatcher
                            .execute(request_id, operation, input, progress)
                            .await?
                    }
                    Dispatcher::Maintenance { lease, cache } => {
                        if input.is_some() {
                            return Err(hypr_loofah_client::failure(
                                ErrorCode::Validation,
                                "Maintenance does not accept an upload",
                            ));
                        }
                        hypr_loofah_core::local_cli::execute_local_maintenance(
                            lease, cache, operation, progress,
                        )
                        .await?
                    }
                };
                let blob = match result.artifact {
                    Some(artifact) => {
                        let (file, owner) = artifact.into_parts();
                        Some(
                            Blob::copy_from_owned(file, &temporary, owner)
                                .await
                                .map_err(application_owner_error)?,
                        )
                    }
                    None => None,
                };
                Ok::<_, ApplicationError>((result.reply, blob))
            }
            .await;
            match result {
                Ok((reply, blob)) => OwnerResponse {
                    response: LocalCliResponse { result: Ok(reply) },
                    blob,
                },
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
    Error::Remote(application_owner_error(cause))
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
