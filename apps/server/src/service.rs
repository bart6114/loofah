use hypr_loofah_core::{VaultContext, VaultRuntime};
use hypr_loofah_protocol::ErrorCode;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::{Mutex, OnceCell, Semaphore};
use tokio_util::sync::CancellationToken;

use crate::{
    auth::Authenticator,
    catalog::Catalog,
    config::Config,
    error::{ApiError, Result},
};

type ShutdownResult = Option<std::result::Result<(), hypr_loofah_protocol::ApplicationError>>;

pub struct Service {
    #[cfg(test)]
    pub(crate) transcription_test_engine:
        std::sync::OnceLock<Arc<dyn hypr_loofah_core::transcription_jobs::TranscriptionEngine>>,
    #[cfg(test)]
    pub(crate) capture_live_test_engine:
        std::sync::OnceLock<Arc<dyn hypr_loofah_core::capture_live::CaptureLiveEngine>>,
    #[cfg(test)]
    pub(crate) injected_changes: std::sync::atomic::AtomicBool,
    #[cfg(test)]
    pub(crate) startup_pause:
        std::sync::Mutex<Option<(std::sync::mpsc::Sender<()>, Arc<std::sync::Barrier>)>>,
    pub auth: Authenticator,
    pub catalog: Catalog,
    pub config: Config,
    pub stop: CancellationToken,
    pub writes: Arc<Semaphore>,
    pub requests: Arc<Semaphore>,
    pub content_reads: Arc<Semaphore>,
    pub media_reads: Arc<Semaphore>,
    pub media_leases: hypr_loofah_core::media_leases::MediaLeases,
    pub media_streams: Arc<Semaphore>,
    pub attachment_streams: Arc<Semaphore>,
    pub attachment_uploads: Arc<Semaphore>,
    pub streams: Arc<Semaphore>,
    processing: Option<Arc<crate::processing::ProcessingHost>>,
    #[cfg(feature = "cpu-transcription")]
    transcription: Option<Arc<crate::transcription_host::TranscriptionHost>>,
    runtimes: Mutex<BTreeMap<String, Arc<OnceCell<VaultRuntime>>>>,
    catalog_worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
    shutdown: OnceCell<tokio::sync::watch::Receiver<ShutdownResult>>,
}

impl Service {
    pub async fn open(mut config: Config) -> Result<Arc<Self>> {
        config
            .validate()
            .map_err(|message| ApiError::new(ErrorCode::Validation, &message))?;
        let storage = config.storage_root.clone();
        let cache = config.cache_root.clone();
        let (storage, cache) = tokio::task::spawn_blocking(move || prepare_roots(&storage, &cache))
            .await
            .map_err(ApiError::internal)??;
        config.storage_root = storage;
        config.cache_root = cache;
        let catalog = Catalog::open(&config.storage_root).await?;
        let auth = Authenticator::discover(&config).await?;
        let processing = config
            .processing
            .as_ref()
            .map(crate::processing::ProcessingHost::load)
            .transpose()?;
        #[cfg(feature = "cpu-transcription")]
        let transcription = config
            .transcription
            .as_ref()
            .map(crate::transcription_host::TranscriptionHost::load)
            .transpose()?;
        let service = Arc::new(Self {
            #[cfg(test)]
            transcription_test_engine: Default::default(),
            #[cfg(test)]
            capture_live_test_engine: Default::default(),
            #[cfg(test)]
            injected_changes: std::sync::atomic::AtomicBool::new(false),
            #[cfg(test)]
            startup_pause: Default::default(),
            auth,
            catalog,
            config,
            stop: CancellationToken::new(),
            writes: Arc::new(Semaphore::new(32)),
            requests: Arc::new(Semaphore::new(64)),
            content_reads: Arc::new(Semaphore::new(4)),
            media_reads: Arc::new(Semaphore::new(2)),
            media_leases: Default::default(),
            media_streams: Arc::new(Semaphore::new(8)),
            attachment_streams: Arc::new(Semaphore::new(2)),
            attachment_uploads: Arc::new(Semaphore::new(2)),
            streams: Arc::new(Semaphore::new(128)),
            processing,
            #[cfg(feature = "cpu-transcription")]
            transcription,
            runtimes: Mutex::new(BTreeMap::new()),
            catalog_worker: Mutex::new(None),
            shutdown: OnceCell::new(),
        });
        let mut changes = service.catalog.subscribe();
        let weak = Arc::downgrade(&service);
        let stop = service.stop.clone();
        #[cfg(feature = "cpu-transcription")]
        let transcription = service.transcription.clone();
        let worker = tokio::spawn(async move {
            let mut media_expiry = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = stop.cancelled() => {
                        #[cfg(feature = "cpu-transcription")]
                        if let Some(host) = &transcription { host.stop_admission(); }
                        break;
                    },
                    _ = media_expiry.tick() => {
                        let Some(service) = weak.upgrade() else { break; };
                        service.media_leases.expire_idle();
                        continue;
                    },
                    change = changes.changed() => if change.is_err() { break; },
                }
                let Some(service) = weak.upgrade() else {
                    break;
                };
                let contexts: Vec<_> = service
                    .runtimes
                    .lock()
                    .await
                    .values()
                    .filter_map(|cell| cell.get().map(|runtime| runtime.context.clone()))
                    .collect();
                for context in contexts {
                    context
                        .events
                        .publish(hypr_loofah_protocol::Change::CatalogChanged);
                }
            }
        });
        *service.catalog_worker.lock().await = Some(worker);
        Ok(service)
    }

    pub async fn vault(&self, id: &str) -> Result<Arc<VaultContext>> {
        // Only IDs from an authorized catalog record reach this method.
        uuid::Uuid::parse_str(id)
            .map_err(|_| ApiError::new(ErrorCode::NotFound, "Vault not found"))?;
        if self.stop.is_cancelled() {
            return Err(ApiError::new(
                ErrorCode::Unavailable,
                "Server is shutting down",
            ));
        }
        let cell = {
            let mut runtimes = self.runtimes.lock().await;
            if self.stop.is_cancelled() {
                return Err(ApiError::new(
                    ErrorCode::Unavailable,
                    "Server is shutting down",
                ));
            }
            runtimes.entry(id.into()).or_default().clone()
        };
        let root = self.config.storage_root.join("vaults").join(id);
        let runtime = self
            .config
            .storage_root
            .join("runtime")
            .join(id)
            .join("creates");
        let id = id.to_owned();
        let cache = self.config.cache_root.clone();
        let catalog = self.catalog.clone();
        let processing = self.processing.clone();
        #[cfg(any(test, feature = "cpu-transcription"))]
        let transcription = self.transcription_engine();
        #[cfg(any(test, feature = "cpu-transcription"))]
        let capture_live = self.capture_live_engine();
        let transcription_stop = self.stop.clone();
        let capture_processing_model = self.config.transcription.as_ref().map(|config| {
            hypr_loofah_core::capture_processing::CaptureProcessingModel {
                alias: config.model.clone(),
                sha256: config.model_sha256.clone(),
            }
        });
        #[cfg(feature = "cpu-transcription")]
        let capture_processing_availability: Arc<
            dyn Fn() -> hypr_loofah_protocol::TranscriptionAvailability + Send + Sync,
        > = {
            let host = self.transcription.clone();
            let model = capture_processing_model
                .as_ref()
                .map(|model| model.alias.clone());
            Arc::new(move || match (&host, &model) {
                (Some(host), Some(model)) => host.policy(model).availability,
                _ => hypr_loofah_protocol::TranscriptionAvailability::Disabled,
            })
        };
        #[cfg(not(feature = "cpu-transcription"))]
        let capture_processing_availability: Arc<
            dyn Fn() -> hypr_loofah_protocol::TranscriptionAvailability + Send + Sync,
        > = Arc::new(|| hypr_loofah_protocol::TranscriptionAvailability::Disabled);
        let capture_origin = format!("{}/", self.config.public_url.origin().ascii_serialization());
        #[cfg(test)]
        let pause = self.startup_pause.lock().unwrap().take();
        #[cfg(test)]
        let injected_changes = self
            .injected_changes
            .load(std::sync::atomic::Ordering::Acquire);
        // Initialization owns its lifetime; a timed-out read cannot cancel migration or release its lease.
        tokio::spawn(async move {
            let _catalog = catalog.clone();
            let context = cell
                .get_or_try_init(|| async {
                    #[cfg(test)]
                    if let Some((entered, resume)) = pause {
                        tokio::task::spawn_blocking(move || {
                            let _ = entered.send(());
                            resume.wait();
                        })
                        .await
                        .map_err(ApiError::internal)?;
                    }
                    reject_symlinks(&root).await?;
                    #[cfg(test)]
                    let opened = if injected_changes {
                        VaultRuntime::open_with_changes(
                            id,
                            &root,
                            &cache,
                            &runtime,
                            tokio::sync::mpsc::unbounded_channel().1,
                        )
                        .await
                    } else {
                        VaultRuntime::open(id, &root, &cache, &runtime).await
                    };
                    #[cfg(not(test))]
                    let opened = VaultRuntime::open(id, &root, &cache, &runtime).await;
                    let mut opened = opened.map_err(|cause| {
                        tracing::error!(%cause, "vault startup failed");
                        ApiError::new(ErrorCode::Unavailable, "Vault could not be opened")
                    })?;
                    let prepared = async {
                        opened
                            .context
                            .enable_capture_ingestion(capture_origin, Default::default())?;
                        opened.prepare_capture_processing(
                            hypr_loofah_core::capture_processing::CaptureProcessingPolicy {
                                model:capture_processing_model,summary_enabled:processing.is_some(),availability:capture_processing_availability,
                            },Arc::new(crate::transcription_authorization::CatalogTranscriptionAuthorizer::new(catalog.clone(),opened.context.events.vault_id(),transcription_stop.clone())),
                        ).await?;
                        #[cfg(any(test, feature = "cpu-transcription"))]
                        if let Some((engine, limits)) = transcription {
                            opened
                                .enable_transcription(
                                    engine,
                                    Arc::new(crate::transcription_authorization::CatalogTranscriptionAuthorizer::new(
                                        catalog.clone(),
                                        opened.context.events.vault_id(),
                                        transcription_stop.clone(),
                                    )),
                                    limits,
                                )
                                .await?;
                        }
                        #[cfg(any(test, feature = "cpu-transcription"))]
                        if let Some(engine) = capture_live {
                            opened.enable_capture_live(
                                engine,
                                Arc::new(crate::transcription_authorization::CatalogTranscriptionAuthorizer::new(
                                    catalog.clone(), opened.context.events.vault_id(), transcription_stop,
                                )),
                            ).await?;
                        }
                        if let Some(processing) = processing {
                            processing.attach(&mut opened, catalog.clone()).await?;
                        }
                        opened.start_capture_processing();
                        Ok::<_, ApiError>(())
                    }
                    .await;
                    if let Err(cause) = prepared {
                        // An empty OnceCell cannot account for a detached Runtime::Drop
                        // drain. Keep this initializer registered until actual cleanup ends.
                        let startup = opened.context.startup.clone();
                        if let Err(error) = opened.shutdown().await {
                            tracing::error!(%error, "failed vault initialization is still draining");
                        }
                        startup.wait_until_stopped().await.map_err(ApiError::internal)?;
                        return Err(cause);
                    }
                    Ok::<_, ApiError>(opened)
                })
                .await?;
            Ok(context.context.clone())
        })
        .await
        .map_err(ApiError::internal)?
    }

    pub(crate) fn transcription_policy(&self) -> hypr_loofah_protocol::TranscriptionPolicy {
        #[cfg(feature = "cpu-transcription")]
        if let (Some(host), Some(config)) = (&self.transcription, &self.config.transcription) {
            let mut policy = host.policy(&config.model);
            if self.stop.is_cancelled() {
                policy.availability = hypr_loofah_protocol::TranscriptionAvailability::Stopping;
            }
            return policy;
        }
        hypr_loofah_protocol::TranscriptionPolicy::default()
    }

    pub async fn shutdown(self: &Arc<Self>) -> Result<()> {
        self.shutdown_with_deadline(std::time::Duration::from_secs(30))
            .await
    }

    #[cfg(any(test, feature = "cpu-transcription"))]
    fn transcription_engine(
        &self,
    ) -> Option<(
        Arc<dyn hypr_loofah_core::transcription_jobs::TranscriptionEngine>,
        hypr_loofah_core::transcription_jobs::TranscriptionLimits,
    )> {
        #[cfg(test)]
        if let Some(engine) = self.transcription_test_engine.get() {
            return Some((engine.clone(), Default::default()));
        }
        #[cfg(feature = "cpu-transcription")]
        {
            self.transcription.as_ref().map(|host| host.engine())
        }
        #[cfg(not(feature = "cpu-transcription"))]
        {
            None
        }
    }

    #[cfg(any(test, feature = "cpu-transcription"))]
    fn capture_live_engine(
        &self,
    ) -> Option<Arc<dyn hypr_loofah_core::capture_live::CaptureLiveEngine>> {
        #[cfg(test)]
        if let Some(engine) = self.capture_live_test_engine.get() {
            return Some(engine.clone());
        }
        #[cfg(feature = "cpu-transcription")]
        {
            self.transcription.as_ref().map(|host| host.live_engine())
        }
        #[cfg(not(feature = "cpu-transcription"))]
        {
            None
        }
    }

    #[cfg(all(test, feature = "cpu-transcription"))]
    pub(crate) fn transcription_resource_observer(
        &self,
    ) -> Option<hypr_transcribe_whisper_local::resources::ResourceObserver> {
        self.transcription.as_ref().map(|host| host.observer())
    }

    pub(crate) async fn shutdown_with_deadline(
        self: &Arc<Self>,
        deadline: std::time::Duration,
    ) -> Result<()> {
        let completion = self.shutdown_completion().await;
        tokio::time::timeout(deadline, wait_shutdown(completion))
            .await
            .map_err(|_| {
                ApiError::new(
                    ErrorCode::Unavailable,
                    "Server shutdown deadline exceeded; owned operations are still draining",
                )
            })?
    }

    /// The process host awaits this before tearing down Tokio, including after a response deadline.
    pub async fn wait_for_shutdown(self: &Arc<Self>) -> Result<()> {
        wait_shutdown(self.shutdown_completion().await).await
    }

    async fn shutdown_completion(self: &Arc<Self>) -> tokio::sync::watch::Receiver<ShutdownResult> {
        self.stop.cancel();
        #[cfg(feature = "cpu-transcription")]
        if let Some(host) = &self.transcription {
            host.stop_admission();
        }
        self.shutdown
            .get_or_init(|| async {
                let (result, completion) = tokio::sync::watch::channel(None);
                let service = self.clone();
                tokio::spawn(async move {
                    let outcome = service.shutdown_owned().await.map_err(|cause| cause.0);
                    result.send_replace(Some(outcome));
                });
                completion
            })
            .await
            .clone()
    }

    async fn shutdown_owned(self: Arc<Self>) -> Result<()> {
        let mut failure = None;
        let catalog_worker = self.catalog_worker.lock().await.take();
        if let Some(worker) = catalog_worker
            && let Err(cause) = worker.await
        {
            failure = Some(ApiError::internal(cause));
        }
        let _writes = drain_slots(self.writes.clone(), 32).await;
        let _reads = drain_slots(self.content_reads.clone(), 4).await;
        let _media_reads = drain_slots(self.media_reads.clone(), 2).await;
        self.media_leases.shutdown().await;
        let runtimes = std::mem::take(&mut *self.runtimes.lock().await);
        for (_, mut cell) in runtimes {
            let cell = loop {
                match Arc::try_unwrap(cell) {
                    Ok(cell) => break cell,
                    Err(current) => {
                        cell = current;
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                }
            };
            if let Some(runtime) = cell.into_inner() {
                let startup = runtime.context.startup.clone();
                if let Err(cause) = runtime.shutdown().await {
                    failure.get_or_insert_with(|| ApiError::internal(cause));
                }
                // A runtime's response deadline does not stop its owner or retry flusher.
                if let Err(cause) = startup.wait_until_stopped().await {
                    failure.get_or_insert_with(|| ApiError::internal(cause));
                }
            }
        }
        #[cfg(feature = "cpu-transcription")]
        if let Some(host) = &self.transcription {
            host.shutdown().await;
        }
        failure.map_or(Ok(()), Err)
    }
}

async fn wait_shutdown(mut completion: tokio::sync::watch::Receiver<ShutdownResult>) -> Result<()> {
    loop {
        let outcome = completion.borrow_and_update().clone();
        if let Some(outcome) = outcome {
            return outcome.map_err(ApiError);
        }
        completion.changed().await.map_err(ApiError::internal)?;
    }
}

async fn drain_slots(
    slots: Arc<Semaphore>,
    count: u32,
) -> Option<tokio::sync::OwnedSemaphorePermit> {
    match slots.clone().acquire_many_owned(count).await {
        Ok(permit) => Some(permit),
        Err(_) => {
            // Closing admission does not release permits held by actual operations.
            while slots.available_permits() < count as usize {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            None
        }
    }
}

fn prepare_roots(storage: &Path, cache: &Path) -> Result<(PathBuf, PathBuf)> {
    std::fs::create_dir_all(storage)?;
    std::fs::create_dir_all(cache)?;
    let storage = std::fs::canonicalize(storage)?;
    let cache = std::fs::canonicalize(cache)?;
    for name in ["catalog.json", ".loofah-server.lock", "vaults", "runtime"] {
        if std::fs::symlink_metadata(storage.join(name))
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(ApiError::new(
                ErrorCode::Validation,
                "Owned service paths must not be symbolic links",
            ));
        }
    }
    for parent in storage.ancestors().skip(1) {
        if parent.join(".loofah-server.lock").exists() {
            return Err(ApiError::new(
                ErrorCode::Validation,
                "Service storage roots must not overlap",
            ));
        }
    }
    let vaults = storage.join("vaults");
    if cache == storage || cache.starts_with(&vaults) || vaults.starts_with(&cache) {
        return Err(ApiError::new(
            ErrorCode::Validation,
            "Cache storage must be separate from canonical vaults",
        ));
    }
    std::fs::create_dir_all(&vaults)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&storage, std::fs::Permissions::from_mode(0o700))?;
        std::fs::set_permissions(&cache, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok((storage, cache))
}

async fn reject_symlinks(root: &Path) -> Result<()> {
    let root = root.to_owned();
    tokio::task::spawn_blocking(move || -> Result<()> {
        let mut pending = vec![root];
        while let Some(path) = pending.pop() {
            let metadata = match std::fs::symlink_metadata(&path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            if metadata.file_type().is_symlink() {
                return Err(ApiError::new(
                    ErrorCode::Validation,
                    "Server vaults must not contain symbolic links",
                ));
            }
            if metadata.is_dir() {
                for entry in std::fs::read_dir(path)? {
                    pending.push(entry?.path());
                }
            }
        }
        Ok(())
    })
    .await
    .map_err(ApiError::internal)?
}
