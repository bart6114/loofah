pub mod auth;
pub mod content;
pub mod dispatch;
pub mod http;
pub mod registry;
pub mod transfers;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};

use hypr_vault_access::{
    ChangeEvent, Error, Result, User, Vault,
    filesystem::{FilesystemStorage, RootLock, atomic_json},
};
use tokio::sync::{Mutex, RwLock, broadcast};

use registry::Registry;

#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
pub struct Config {
    pub data_dir: PathBuf,
    pub public_url: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    #[serde(default = "default_upload_limit")]
    pub max_upload_bytes: u64,
}

fn default_upload_limit() -> u64 {
    20 * 1024 * 1024 * 1024
}

pub struct VaultRuntime {
    pub gate: RwLock<()>,
    pub view_gate: RwLock<()>,
    pub revisions: std::sync::RwLock<hypr_vault_access::Versions>,
    pub storage: Arc<FilesystemStorage>,
    pub store: Arc<hypr_vault_write::SessionStore>,
    pub failed: std::sync::atomic::AtomicBool,
    pub headers:
        std::sync::RwLock<std::collections::BTreeMap<String, hypr_vault_write::SessionListHeader>>,
    pub search: hypr_search_cache::Cache,
    pub search_ready: std::sync::atomic::AtomicBool,
    pub index_changes: tokio::sync::mpsc::Sender<Vec<String>>,
    pub index_dirty: std::sync::atomic::AtomicBool,
    pub transfer_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub artifact_versions:
        std::sync::Mutex<HashMap<String, (u64, u64, std::time::SystemTime, String)>>,
}

impl VaultRuntime {
    pub fn index_changed(&self, ids: Vec<String>) {
        if self.index_changes.try_send(ids).is_err() {
            self.index_dirty.store(true, Ordering::Release);
        }
    }
}

pub struct Server {
    pub config: Config,
    pub registry: RwLock<Registry>,
    pub events: broadcast::Sender<ChangeEvent>,
    pub logins: Mutex<HashMap<String, auth::Login>>,
    pub http: reqwest::Client,
    pub stopping: tokio::sync::watch::Sender<bool>,
    runtimes: Mutex<HashMap<String, Arc<tokio::sync::OnceCell<Arc<VaultRuntime>>>>>,
    epoch: String,
    sequence: AtomicU64,
    _owner: RootLock,
}

impl Server {
    pub fn open(mut config: Config) -> Result<Arc<Self>> {
        config.public_url = hypr_vault_access::connection::validate_server_url(&config.public_url)?;
        let issuer = url::Url::parse(&config.issuer)
            .map_err(|_| Error::new("invalid_request", "invalid OIDC issuer"))?;
        if issuer.scheme() != "https"
            && !(issuer.scheme() == "http"
                && matches!(issuer.host_str(), Some("127.0.0.1" | "localhost")))
        {
            return Err(Error::new("invalid_request", "OIDC issuer must use HTTPS"));
        }
        let owner = RootLock::acquire(&config.data_dir)?;
        std::fs::create_dir_all(config.data_dir.join("service"))?;
        std::fs::create_dir_all(config.data_dir.join("vaults"))?;
        let registry_path = config.data_dir.join("service/registry.json");
        let registry = match std::fs::read(&registry_path) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let registry = Registry::default();
                atomic_json(&registry_path, &registry)?;
                registry
            }
            Err(error) => return Err(error.into()),
        };
        if registry.version != 1 {
            return Err(Error::new(
                "invalid_data",
                "unsupported service registry version",
            ));
        }
        Ok(Arc::new(Self {
            config,
            registry: RwLock::new(registry),
            events: broadcast::channel(1024).0,
            logins: Mutex::new(HashMap::new()),
            http: reqwest::Client::new(),
            stopping: tokio::sync::watch::channel(false).0,
            runtimes: Mutex::new(HashMap::new()),
            epoch: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
            _owner: owner,
        }))
    }

    pub async fn save_registry(&self, registry: &Registry) -> Result<()> {
        let path = self.config.data_dir.join("service/registry.json");
        let bytes = serde_json::to_vec_pretty(registry)?;
        tokio::task::spawn_blocking(move || {
            hypr_vault_access::filesystem::atomic_write(&path, &bytes)
        })
        .await
        .map_err(|error| Error::new("storage", error.to_string()))?
    }

    pub async fn runtime(&self, vault_id: &str) -> Result<Arc<VaultRuntime>> {
        hypr_vault_access::validate_id(vault_id)?;
        if !self.registry.read().await.vaults.contains_key(vault_id) {
            return Err(Error::new("not_found", "vault not found"));
        }
        let slot = self
            .runtimes
            .lock()
            .await
            .entry(vault_id.into())
            .or_default()
            .clone();
        slot.get_or_try_init(|| self.open_runtime(vault_id))
            .await
            .cloned()
    }

    async fn open_runtime(&self, vault_id: &str) -> Result<Arc<VaultRuntime>> {
        let content = self.config.data_dir.join("vaults").join(vault_id);
        let state = self.config.data_dir.join("service/vaults").join(vault_id);
        let storage = Arc::new(
            tokio::task::spawn_blocking(move || FilesystemStorage::new(content, state))
                .await
                .map_err(|error| Error::new("storage", error.to_string()))??,
        );
        let store = Arc::new(hypr_vault_write::SessionStore::new(storage.content.clone()));
        let report = store.rebuild_index().await.map_err(content::store_error)?;
        if !report.errors.is_empty() {
            return Err(Error::new(
                "invalid_data",
                format!("vault could not be indexed: {:?}", report.errors),
            ));
        }
        let search =
            hypr_search_cache::Cache::new(&self.config.data_dir.join("cache"), &storage.content)
                .map_err(|error| Error::new("storage", error.to_string()))?;
        let headers = store
            .session_list_headers()
            .into_iter()
            .map(|header| (header.id.clone(), header))
            .collect();
        let revision_storage = storage.clone();
        let ids = store.session_ids();
        let revisions = tokio::task::spawn_blocking(move || {
            use hypr_vault_access::filesystem::VaultStorage;
            let keys = content::document_keys(&revision_storage.content, &ids)?
                .into_iter()
                .filter(|key| !key.ends_with("transcript.json"))
                .collect::<Vec<_>>();
            revision_storage.versions(&keys)
        })
        .await
        .map_err(|error| Error::new("storage", error.to_string()))??;
        let (index_changes, mut changes) = tokio::sync::mpsc::channel::<Vec<String>>(64);
        let runtime = Arc::new(VaultRuntime {
            gate: RwLock::new(()),
            view_gate: RwLock::new(()),
            revisions: std::sync::RwLock::new(revisions),
            storage,
            store,
            failed: false.into(),
            headers: std::sync::RwLock::new(headers),
            search,
            search_ready: false.into(),
            index_changes,
            index_dirty: false.into(),
            transfer_locks: Mutex::new(HashMap::new()),
            artifact_versions: std::sync::Mutex::new(HashMap::new()),
        });
        let indexing = Arc::downgrade(&runtime);
        let cache = runtime.search.clone();
        tokio::spawn(async move {
            loop {
                if indexing.strong_count() == 0 {
                    return;
                }
                let worker = cache.clone();
                match tokio::task::spawn_blocking(move || worker.initialize(&mut |_| {})).await {
                    Ok(Ok(_)) => {
                        if let Some(runtime) = indexing.upgrade() {
                            runtime.search_ready.store(true, Ordering::Release);
                        }
                        break;
                    }
                    result => {
                        tracing::warn!(?result, "vault search initialization will retry");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
            }
            while let Some(ids) = changes.recv().await {
                let mut pending = ids.into_iter().collect::<std::collections::BTreeSet<_>>();
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                while let Ok(ids) = changes.try_recv() {
                    pending.extend(ids);
                }
                let worker = cache.clone();
                let ids = pending.into_iter().collect::<Vec<_>>();
                let mut full = indexing
                    .upgrade()
                    .is_some_and(|runtime| runtime.index_dirty.swap(false, Ordering::AcqRel));
                loop {
                    let worker = worker.clone();
                    let ids = ids.clone();
                    let result = tokio::task::spawn_blocking(move || {
                        if full {
                            worker.initialize(&mut |_| {}).map(|_| ())
                        } else {
                            worker.reconcile_sessions(&ids).map(|_| ())
                        }
                    })
                    .await;
                    match result {
                        Ok(Ok(())) => break,
                        result => {
                            tracing::warn!(?result, "vault search reconciliation will retry");
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                            full = true;
                        }
                    }
                    if indexing.strong_count() == 0 {
                        return;
                    }
                }
            }
        });
        Ok(runtime)
    }

    pub async fn authorize(
        &self,
        user: &User,
        vault_id: &str,
        write: bool,
        owner: bool,
    ) -> Result<Vault> {
        let registry = self.registry.read().await;
        let current = registry
            .users
            .get(&user.id)
            .ok_or_else(|| Error::new("unauthenticated", "user is unavailable"))?;
        if current.disabled {
            return Err(Error::new("unauthenticated", "user is disabled"));
        }
        let vault = registry
            .vaults
            .get(vault_id)
            .ok_or_else(|| Error::new("not_found", "vault not found"))?;
        let role = vault
            .members
            .get(&user.id)
            .ok_or_else(|| Error::new("forbidden", "vault access was removed"))?;
        if (write && !role.can_write()) || (owner && *role != hypr_vault_access::Role::Owner) {
            return Err(Error::new("forbidden", "insufficient vault permissions"));
        }
        Ok(vault.clone())
    }

    pub fn notify(&self, vault_id: Option<String>, entity: &str, ids: Vec<String>) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let _ = self.events.send(ChangeEvent {
            cursor: format!("{}:{sequence}", self.epoch),
            vault_id,
            entity: entity.into(),
            ids,
            reset: false,
        });
    }
}
