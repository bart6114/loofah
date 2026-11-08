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

pub struct Service {
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
    runtimes: Mutex<BTreeMap<String, Arc<OnceCell<VaultRuntime>>>>,
    catalog_worker: Mutex<Option<tokio::task::JoinHandle<()>>>,
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
        let service = Arc::new(Self {
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
            runtimes: Mutex::new(BTreeMap::new()),
            catalog_worker: Mutex::new(None),
        });
        let mut changes = service.catalog.subscribe();
        let weak = Arc::downgrade(&service);
        let stop = service.stop.clone();
        let worker = tokio::spawn(async move {
            let mut media_expiry = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = stop.cancelled() => break,
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
                    opened
                        .context
                        .enable_capture_ingestion(capture_origin, Default::default())?;
                    if let Some(processing) = processing {
                        processing.attach(&mut opened, catalog.clone()).await?;
                    }
                    Ok::<_, ApiError>(opened)
                })
                .await?;
            Ok(context.context.clone())
        })
        .await
        .map_err(ApiError::internal)?
    }

    pub async fn shutdown(&self) -> Result<()> {
        self.stop.cancel();
        if let Some(worker) = self.catalog_worker.lock().await.take() {
            worker.await.map_err(ApiError::internal)?;
        }
        let _drained = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            let writes = self
                .writes
                .clone()
                .acquire_many_owned(32)
                .await
                .map_err(ApiError::internal)?;
            let reads = self
                .content_reads
                .clone()
                .acquire_many_owned(4)
                .await
                .map_err(ApiError::internal)?;
            Ok::<_, ApiError>((writes, reads))
        })
        .await
        .map_err(|_| {
            ApiError::new(
                ErrorCode::Unavailable,
                "Active operations did not drain before shutdown",
            )
        })??;
        self.media_leases.close_all();
        let runtimes = std::mem::take(&mut *self.runtimes.lock().await);
        for (_, cell) in runtimes {
            tokio::time::timeout(std::time::Duration::from_secs(30), async {
                while Arc::strong_count(&cell) > 1 {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .map_err(|_| {
                ApiError::new(ErrorCode::Unavailable, "Vault initialization did not drain")
            })?;
            let cell = Arc::try_unwrap(cell).map_err(|_| {
                ApiError::new(ErrorCode::Unavailable, "Vault startup has not drained")
            })?;
            if let Some(runtime) = cell.into_inner() {
                runtime.shutdown().await.map_err(ApiError::internal)?;
            }
        }
        Ok(())
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
