use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use libsodium_rs::crypto_sign::KeyPair;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tauri_specta::Event;
use tokio::sync::{mpsc, oneshot};
use uuid::Uuid;
use vault_sync::{
    crypto::VaultKey,
    engine::{Binding, Engine, atomic_json, now_ms},
    enrollment::{Challenge, ExpectedIdentity, Kind, hex},
    keychain::Keychain,
    remote::{BrowserLogin, Environment, RemoteClient, TransferProgress},
};
use zeroize::Zeroizing;

#[derive(Clone, Default, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    enabled: bool,
    phase: String,
    vault_path: String,
    browser_url: Option<String>,
    user_code: Option<String>,
    pairing_code: Option<String>,
    error: Option<String>,
    last_success: Option<u64>,
    pending_work: usize,
    active_transfers: usize,
    transferred_bytes: u64,
    transfer_bytes: u64,
    used_bytes: u64,
    quota_bytes: u64,
    conflicts: Vec<SyncConflict>,
    devices: Vec<Device>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct Device {
    id: String,
    public_key: String,
    enrolled_at: u64,
    revoked_at: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize, specta::Type, tauri_specta::Event)]
pub struct SyncStatusChanged {
    pub status: SyncStatus,
}

#[derive(Clone, PartialEq, Serialize, Deserialize, specta::Type)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncEntity {
    Session { id: String },
    People,
    Tags,
    Tasks,
}
impl From<vault_sync::scope::Entity> for SyncEntity {
    fn from(value: vault_sync::scope::Entity) -> Self {
        use vault_sync::scope::Entity;
        match value {
            Entity::Session(id) => Self::Session { id },
            Entity::People => Self::People,
            Entity::Tags => Self::Tags,
            Entity::Tasks => Self::Tasks,
        }
    }
}
impl From<SyncEntity> for vault_sync::scope::Entity {
    fn from(value: SyncEntity) -> Self {
        match value {
            SyncEntity::Session { id } => Self::Session(id),
            SyncEntity::People => Self::People,
            SyncEntity::Tags => Self::Tags,
            SyncEntity::Tasks => Self::Tasks,
        }
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize, specta::Type)]
pub struct SyncConflict {
    pub entity: SyncEntity,
    pub local: String,
    pub cloud: String,
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum SyncAction {
    Connect,
    Cancel,
    Pause,
    Resume,
    ReconcileRecovery,
    Disconnect,
    SaveRecoveryKit,
    ImportRecoveryKit,
    RefreshDevices,
    PairNewMac,
    ApproveMac {
        code: String,
    },
    RevokeDevice {
        id: String,
    },
    PurgeVersion {
        revision: String,
    },
    History {
        entity: SyncEntity,
        before: Option<(u64, String)>,
    },
    Preview {
        entity: SyncEntity,
        revision: String,
    },
    Restore {
        entity: SyncEntity,
        revision: String,
    },
    Export {
        entity: SyncEntity,
        revision: String,
    },
}

#[derive(Clone, Serialize, Deserialize, specta::Type)]
pub struct Version {
    pub id: String,
    pub operation: String,
    pub device_id: String,
    pub created_at: u64,
    pub current: Option<u8>,
    pub pinned: u8,
}
#[derive(Clone, Serialize, Deserialize, specta::Type)]
pub struct VersionPreview {
    pub text: String,
    pub files: Vec<String>,
    pub changed: Vec<String>,
    pub captured_at: Option<u64>,
    pub missing_references: Vec<String>,
    pub deleted: bool,
}
#[derive(Clone, Serialize, Deserialize, specta::Type)]
pub struct SyncContent {
    pub versions: Vec<Version>,
    pub preview: Option<VersionPreview>,
}

struct Request {
    action: SyncAction,
    reply: oneshot::Sender<Result<Option<SyncContent>, String>>,
}

pub struct SyncState {
    status: Arc<RwLock<SyncStatus>>,
    commands: mpsc::Sender<Request>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    version: u32,
    binding: Binding,
    paused: bool,
}

#[derive(Clone, Deserialize)]
struct AccountResponse {
    account: Account,
    identity: Identity,
}
#[derive(Clone, Deserialize)]
struct Account {
    vault_id: Uuid,
    recovery_generation: Uuid,
    enrollment_authority: Option<String>,
    used_bytes: u64,
    quota_bytes: u64,
}
#[derive(Clone, Deserialize)]
struct Identity {
    user: String,
    session: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingEnrollment {
    challenge: Challenge,
    authority_signature: String,
}

struct Runtime {
    app: AppHandle,
    store: Arc<crate::session_store::SessionStore>,
    root: PathBuf,
    status: Arc<RwLock<SyncStatus>>,
    connection: Option<Connection>,
    login: Option<BrowserLogin>,
    account: Option<AccountResponse>,
    remote: Option<RemoteClient>,
    engine: Option<Engine>,
    kit: Option<VaultKey>,
    pairing: Option<tokio::task::JoinHandle<Result<Option<(VaultKey, Uuid, KeyPair)>, String>>>,
    progress: TransferProgress,
}

fn enabled<R: tauri::Runtime>(app: &AppHandle<R>) -> bool {
    cfg!(feature = "staging") && app.config().identifier == "io.loofah.staging"
}
fn root() -> Result<PathBuf, String> {
    dirs::data_dir()
        .map(|path| path.join("io.loofah.sync.staging"))
        .ok_or("Application storage is unavailable.".into())
}
fn directory(root: &Path, vault: &Path) -> Result<PathBuf, String> {
    use sha2::{Digest, Sha256};
    let path = vault.canonicalize().map_err(|error| error.to_string())?;
    Ok(root.join(hex(&Sha256::digest(path.as_os_str().as_encoded_bytes()))))
}

pub async fn recover(app: &AppHandle, vault: &Path) -> Result<(), String> {
    if !enabled(app) {
        return Ok(());
    }
    let directory = directory(&root()?, vault)?.join("replica");
    let vault = vault.to_owned();
    tokio::task::spawn_blocking(move || {
        vault_sync::replica::LocalReplica::open(&vault, &directory)?.recover()
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn spawn(app: AppHandle, store: Arc<crate::session_store::SessionStore>) {
    let (commands, mut requests) = mpsc::channel::<Request>(16);
    let status = Arc::new(RwLock::new(SyncStatus {
        enabled: enabled(&app),
        phase: "disconnected".into(),
        vault_path: store.vault_base().display().to_string(),
        ..Default::default()
    }));
    app.manage(SyncState {
        status: status.clone(),
        commands,
    });
    if !enabled(&app) {
        return;
    }
    let progress = TransferProgress::default();
    let mut runtime = match root().and_then(|root| directory(&root, store.vault_base())) {
        Ok(root) => Runtime {
            app: app.clone(),
            store: store.clone(),
            root,
            status,
            connection: None,
            login: None,
            account: None,
            remote: None,
            engine: None,
            kit: None,
            pairing: None,
            progress: progress.clone(),
        },
        Err(error) => {
            app.state::<SyncState>().status.write().unwrap().error = Some(error);
            return;
        }
    };
    let event_app = app.clone();
    let event_status = runtime.status.clone();
    tauri::async_runtime::spawn(async move {
        let mut previous = None;
        let mut timer = tokio::time::interval(Duration::from_millis(250));
        timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            timer.tick().await;
            let progress = progress.snapshot();
            let mut status = event_status.write().unwrap();
            status.active_transfers = progress.active;
            status.transferred_bytes = progress.completed;
            status.transfer_bytes = progress.total;
            if previous.as_ref() != Some(&*status) {
                previous = Some(status.clone());
                let _ = (SyncStatusChanged {
                    status: status.clone(),
                })
                .emit(&event_app);
            }
        }
    });
    let mut changes = store.subscribe_index_changes();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = app
            .state::<crate::startup::StartupState>()
            .wait_until_ready()
            .await
        {
            runtime.report(Err(error));
            return;
        }
        let result = runtime.load().await;
        runtime.report(result);
        let mut next = tokio::time::Instant::now();
        let mut failures = 0u32;
        let mut last_tick = now_ms();
        loop {
            enum Outcome {
                Changed(Option<(crate::session_store::IndexEntity, Vec<String>)>),
                Request(Option<Request>),
                Login(vault_sync::Result<Option<Zeroizing<String>>>),
                Tick(vault_sync::Result<()>),
                Pairing(Result<Option<(VaultKey, Uuid, KeyPair)>, String>),
                Idle,
            }
            let accept_changes = tokio::time::Instant::now() < next || runtime.engine.is_none();
            let outcome = {
                let work = async {
                    if let Some(login) = runtime.login.as_mut() {
                        return Outcome::Login(login.poll().await);
                    }
                    if let Some(pairing) = runtime.pairing.as_mut() {
                        return Outcome::Pairing(
                            pairing
                                .await
                                .unwrap_or_else(|_| Err("Pairing cancelled.".into())),
                        );
                    }
                    tokio::time::sleep_until(next).await;
                    if runtime
                        .connection
                        .as_ref()
                        .is_some_and(|value| !value.paused)
                    {
                        if let Some(engine) = runtime.engine.as_mut() {
                            if now_ms().saturating_sub(last_tick) > 30_000 {
                                engine.reconcile();
                            }
                            return Outcome::Tick(engine.tick(&runtime.store).await);
                        }
                    }
                    Outcome::Idle
                };
                tokio::select! { change = changes.recv(), if accept_changes => Outcome::Changed(change), request = requests.recv() => Outcome::Request(request), result = work => result }
            };
            let result = match outcome {
                Outcome::Changed(Some((entity, ids))) => {
                    if let Some(engine) = runtime.engine.as_mut() {
                        use crate::session_store::IndexEntity;
                        use vault_sync::scope::Entity;
                        match entity {
                            IndexEntity::People => engine.changed(Entity::People),
                            IndexEntity::Tags => engine.changed(Entity::Tags),
                            _ if ids.is_empty() => engine.reconcile(),
                            _ => {
                                for id in ids {
                                    engine.changed(if id.is_empty() {
                                        Entity::Tasks
                                    } else {
                                        Entity::Session(id)
                                    });
                                }
                            }
                        }
                    }
                    Ok(())
                }
                Outcome::Changed(None) => break,
                Outcome::Request(Some(mut request)) => loop {
                    let next_request = tokio::select! {
                        result = runtime.action(request.action.clone()) => {
                            let report = result.as_ref().map(|_| ()).map_err(Clone::clone);
                            let _ = request.reply.send(result);
                            break report;
                        },
                        next_request = requests.recv() => next_request,
                    };
                    let _ = request.reply.send(Err(
                        "Sync action interrupted. Pending work is preserved.".into(),
                    ));
                    match next_request {
                        Some(next_request) => request = next_request,
                        None => break Err("Sync stopped.".into()),
                    }
                },
                Outcome::Request(None) => break,
                Outcome::Login(Ok(Some(credential))) => {
                    runtime.login = None;
                    runtime.accept_login(&credential).await
                }
                Outcome::Login(Ok(None)) => Ok(()),
                Outcome::Login(Err(error)) => {
                    runtime.login = None;
                    Err(error.to_string())
                }
                Outcome::Tick(result) => {
                    if matches!(&result, Err(vault_sync::Error::Cloud { code, .. }) if code == "recovery_required")
                    {
                        if let Some(connection) = runtime.connection.as_mut() {
                            connection.paused = true;
                        }
                        runtime.phase("recovery_required");
                        let _ = runtime.save();
                    }
                    if matches!(&result, Err(vault_sync::Error::Cloud { status: 401, .. })) {
                        if let Some(connection) = runtime.connection.as_mut() {
                            connection.paused = true;
                        }
                        runtime.phase("authorization_required");
                        let _ = runtime.save();
                    }
                    last_tick = now_ms();
                    failures = if result.is_ok() {
                        0
                    } else {
                        (failures + 1).min(5)
                    };
                    next = tokio::time::Instant::now()
                        + Duration::from_secs((10u64 << failures).min(300));
                    if result.is_ok() {
                        runtime.status.write().unwrap().error = None;
                    }
                    result.map_err(|error| match error {
                        vault_sync::Error::Cloud { code, .. } if code == "quota" => "Storage is full. Uploads remain queued on this Mac. You can preview, export or restore cloud versions, or purge old history to free space.".into(),
                        vault_sync::Error::Cloud { status: 401, .. } => "Your account session expired or this device was revoked. Connect again to continue.".into(),
                        error => error.to_string(),
                    })
                }
                Outcome::Pairing(result) => {
                    runtime.pairing = None;
                    runtime.status.write().unwrap().pairing_code = None;
                    match result {
                        Ok(Some((key, device, pair))) => runtime.enroll(key, device, pair).await,
                        Ok(None) => {
                            runtime.phase(
                                if runtime.connection.as_ref().is_some_and(|c| !c.paused) {
                                    "connected"
                                } else {
                                    "paused"
                                },
                            );
                            Ok(())
                        }
                        Err(error) => {
                            runtime.phase(if runtime.engine.is_some() {
                                "paused"
                            } else {
                                "import_recovery_kit"
                            });
                            Err(error)
                        }
                    }
                }
                Outcome::Idle => {
                    next = tokio::time::Instant::now() + Duration::from_secs(10);
                    Ok(())
                }
            };
            runtime.report(result);
        }
    });
}

impl Runtime {
    fn report(&mut self, result: Result<(), String>) {
        let mut status = self.status.write().unwrap();
        if let Err(error) = result {
            status.error = Some(error);
        }
        if let Some(engine) = &self.engine {
            status.last_success = engine.last_success();
            status.pending_work = engine.pending_work();
            status.conflicts = engine
                .conflicts()
                .into_iter()
                .map(|value| SyncConflict {
                    entity: value.entity.into(),
                    local: value.local.to_string(),
                    cloud: value.cloud.to_string(),
                })
                .collect();
        }
    }
    fn phase(&self, phase: &str) {
        let mut status = self.status.write().unwrap();
        status.phase = phase.into();
        status.error = None;
    }
    fn save(&self) -> Result<(), String> {
        atomic_json(&self.root.join("connection.json"), &self.connection)
            .map_err(|error| error.to_string())
    }
    async fn load(&mut self) -> Result<(), String> {
        let bytes = match tokio::fs::read(self.root.join("connection.json")).await {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.to_string()),
        };
        self.connection = serde_json::from_slice(&bytes)
            .map_err(|_| "Sync settings need recovery. Local content is safe.")?;
        if let Some(connection) = &self.connection {
            if connection.version != 1
                || connection.binding.local
                    != self
                        .store
                        .vault_base()
                        .canonicalize()
                        .map_err(|e| e.to_string())?
            {
                return Err("The sync connection belongs to another vault.".into());
            }
            let credential = tokio::task::block_in_place(|| {
                Keychain::new(Environment::Staging).session(connection.binding.vault)
            })
            .map_err(|e| e.to_string())?;
            self.attach(
                std::str::from_utf8(&credential).map_err(|_| "Invalid saved account session.")?,
            )
            .await?;
        }
        Ok(())
    }
    async fn accept_login(&mut self, credential: &str) -> Result<(), String> {
        let remote =
            RemoteClient::new(Environment::Staging, credential, None).map_err(|e| e.to_string())?;
        let account: AccountResponse = remote.get("/account").await.map_err(|e| e.to_string())?;
        if self.connection.as_ref().is_some_and(|value| {
            value.binding.account != account.identity.user
                || value.binding.vault != account.account.vault_id
        }) {
            return Err(
                "Disconnect the previous account before connecting another account.".into(),
            );
        }
        tokio::task::block_in_place(|| {
            Keychain::new(Environment::Staging)
                .save_session(account.account.vault_id, credential.as_bytes())
        })
        .map_err(|e| e.to_string())?;
        self.connection = Some(Connection {
            version: 1,
            paused: true,
            binding: Binding {
                account: account.identity.user.clone(),
                vault: account.account.vault_id,
                generation: account.account.recovery_generation,
                local: self
                    .store
                    .vault_base()
                    .canonicalize()
                    .map_err(|e| e.to_string())?,
            },
        });
        self.save()?;
        self.attach(credential).await
    }
    async fn attach(&mut self, credential: &str) -> Result<(), String> {
        let remote =
            RemoteClient::new(Environment::Staging, credential, None).map_err(|e| e.to_string())?;
        let account: AccountResponse = remote.get("/account").await.map_err(|e| e.to_string())?;
        let connection = self
            .connection
            .as_ref()
            .ok_or("Connect your account first.")?;
        if connection.binding.account != account.identity.user
            || connection.binding.vault != account.account.vault_id
            || connection.binding.generation != account.account.recovery_generation
        {
            self.phase("recovery_required");
            return Err(
                "Account or recovery generation changed. Explicit reconciliation is required."
                    .into(),
            );
        }
        {
            let mut status = self.status.write().unwrap();
            status.used_bytes = account.account.used_bytes;
            status.quota_bytes = account.account.quota_bytes;
            status.browser_url = None;
            status.user_code = None;
        }
        self.remote = Some(
            RemoteClient::new(
                Environment::Staging,
                credential,
                Some(account.account.recovery_generation),
            )
            .map_err(|e| e.to_string())?
            .with_progress(self.progress.clone()),
        );
        self.account = Some(account);
        let identity = tokio::task::block_in_place(|| {
            Keychain::new(Environment::Staging).identity(connection.binding.vault)
        });
        match identity {
            Ok((key, device, pair)) => self.enroll(key, device, pair).await,
            Err(_) => {
                self.phase(
                    if self
                        .account
                        .as_ref()
                        .unwrap()
                        .account
                        .enrollment_authority
                        .is_none()
                    {
                        "save_recovery_kit"
                    } else {
                        "import_recovery_kit"
                    },
                );
                Ok(())
            }
        }
    }
    async fn enroll(&mut self, key: VaultKey, device: Uuid, pair: KeyPair) -> Result<(), String> {
        let account = self.account.as_ref().ok_or("Connect your account first.")?;
        let remote = self.remote.as_ref().ok_or("Connect your account first.")?;
        let public_key = hex(pair.public_key.as_bytes());
        let authority = hex(key
            .enrollment_authority()
            .map_err(|e| e.to_string())?
            .public_key
            .as_bytes());
        #[derive(Deserialize)]
        struct Devices {
            devices: Vec<Device>,
        }
        let devices: Devices = remote.get("/devices").await.map_err(|e| e.to_string())?;
        let existing = devices
            .devices
            .iter()
            .find(|item| item.id == device.to_string());
        if existing.is_some_and(|item| item.revoked_at.is_some() || item.public_key != public_key) {
            self.phase("import_recovery_kit");
            return Err(
                "This device was revoked. Import your recovery kit to enroll a new device.".into(),
            );
        }
        let kind = if existing.is_some() {
            Kind::Bind
        } else {
            Kind::Enroll
        };
        let pending: Option<PendingEnrollment> = if kind == Kind::Enroll {
            match tokio::fs::read(self.root.join("pending-enrollment.json")).await {
                Ok(bytes) => Some(serde_json::from_slice(&bytes).map_err(
                    |_| "Interrupted enrollment needs recovery import or a new pairing.",
                )?),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => return Err(error.to_string()),
            }
        } else {
            None
        };
        let challenge: Challenge = if let Some(pending) = &pending {
            pending.challenge.clone()
        } else {
            remote.post("/enrollment/challenge", &serde_json::json!({ "kind": kind, "device": device, "publicKey": public_key, "authority": authority })).await.map_err(|e| e.to_string())?
        };
        let challenge = challenge
            .verify(
                ExpectedIdentity {
                    vault: key.vault(),
                    user: &account.identity.user,
                    session: &account.identity.session,
                    device,
                    public_key: &public_key,
                    authority: &authority,
                    generation: account.account.recovery_generation,
                    kind,
                },
                now_ms(),
            )
            .map_err(|e| e.to_string())?;
        let approval = if let Some(pending) = pending {
            Some(pending.authority_signature)
        } else if kind == Kind::Enroll {
            Some(
                challenge
                    .approve_enrollment(&key)
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        remote.post::<serde_json::Value>("/enrollment/finish", &serde_json::json!({ "challenge": challenge.id(), "deviceSignature": challenge.sign_device(&pair).map_err(|e| e.to_string())?, "authoritySignature": approval })).await.map_err(|e| e.to_string())?;
        let _ = tokio::fs::remove_file(self.root.join("pending-enrollment.json")).await;
        let connection = self.connection.as_ref().unwrap();
        let paused = connection.paused;
        let engine = tokio::task::block_in_place(|| {
            Engine::open(
                &self.root.join("replica"),
                connection.binding.clone(),
                key,
                remote.clone(),
            )
        });
        self.engine = Some(match engine {
            Ok(engine) => engine,
            Err(vault_sync::Error::RecoveryRequired) => {
                self.phase("recovery_required");
                return Err("Sync state belongs to an earlier account or recovery generation. Reconcile explicitly to continue; previous state will be retained.".into());
            }
            Err(error) => return Err(error.to_string()),
        });
        self.phase(if paused { "paused" } else { "connected" });
        Ok(())
    }
    async fn reconcile_recovery(&mut self) -> Result<(), String> {
        let binding = self
            .connection
            .as_ref()
            .ok_or("Connect your account first.")?
            .binding
            .clone();
        let credential = tokio::task::block_in_place(|| {
            Keychain::new(Environment::Staging).session(binding.vault)
        })
        .map_err(|e| e.to_string())?;
        let credential =
            std::str::from_utf8(&credential).map_err(|_| "Invalid account session.")?;
        let remote =
            RemoteClient::new(Environment::Staging, credential, None).map_err(|e| e.to_string())?;
        let current: AccountResponse = remote.get("/account").await.map_err(|e| e.to_string())?;
        if current.identity.user != binding.account || current.account.vault_id != binding.vault {
            return Err("Disconnect before changing accounts.".into());
        }
        let (key, _, _) = tokio::task::block_in_place(|| {
            Keychain::new(Environment::Staging).identity(binding.vault)
        })
        .map_err(|e| e.to_string())?;
        if current.account.enrollment_authority.as_deref()
            != Some(&hex(key
                .enrollment_authority()
                .map_err(|e| e.to_string())?
                .public_key
                .as_bytes()))
        {
            return Err("Recover the correct vault keys before reconciliation.".into());
        }
        if let Some(connection) = self.connection.as_mut() {
            connection.paused = true;
        }
        self.save()?;
        if let Some(engine) = &self.engine {
            engine.wait_idle().await.map_err(|e| e.to_string())?;
        }
        self.engine = None;
        let directory = self.root.join("replica");
        tokio::task::block_in_place(|| -> Result<(), String> {
            vault_sync::replica::LocalReplica::open(self.store.vault_base(), &directory)
                .and_then(|replica| replica.recover())
                .map_err(|e| e.to_string())?;
            let _transaction =
                hypr_vault_read::transaction::VaultTransaction::exclusive(self.store.vault_base())
                    .map_err(|e| e.to_string())?;
            hypr_vault_read::transaction::VaultTransaction::ensure_ready(self.store.vault_base())
                .map_err(|e| e.to_string())?;
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(directory.join("engine.lock"))
                .map_err(|e| e.to_string())?;
            lock.try_lock()
                .map_err(|_| "Another sync engine is active.")?;
            let archive = self.root.join("archives");
            std::fs::create_dir_all(&archive).map_err(|e| e.to_string())?;
            std::fs::rename(&directory, archive.join(Uuid::new_v4().to_string()))
                .map_err(|e| e.to_string())?;
            std::fs::File::open(&archive)
                .and_then(|file| file.sync_all())
                .map_err(|e| e.to_string())?;
            std::fs::File::open(&self.root)
                .and_then(|file| file.sync_all())
                .map_err(|e| e.to_string())?;
            Ok(())
        })?;
        self.store
            .rebuild_index()
            .await
            .map_err(|e| e.to_string())?;
        self.connection.as_mut().unwrap().binding.generation = current.account.recovery_generation;
        self.save()?;
        self.attach(credential).await
    }

    async fn action(&mut self, action: SyncAction) -> Result<Option<SyncContent>, String> {
        match action {
            SyncAction::PurgeVersion { revision } => {
                let revision = Uuid::parse_str(&revision).map_err(|_| "Invalid revision.")?;
                self.engine
                    .as_ref()
                    .ok_or("Finish enrollment first.")?
                    .purge(&[revision])
                    .await
                    .map_err(|e| e.to_string())?;
            }
            SyncAction::History { entity, before } => {
                let entity: vault_sync::scope::Entity = entity.into();
                entity.root().map_err(|e| e.to_string())?;
                let value = self
                    .engine
                    .as_ref()
                    .ok_or("Finish enrollment first.")?
                    .history_page(
                        &entity,
                        before
                            .map(|(at, id)| Uuid::parse_str(&id).map(|id| (at, id)))
                            .transpose()
                            .map_err(|_| "Invalid history cursor.")?,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                let versions: Vec<Version> = serde_json::from_value(value["results"].clone())
                    .map_err(|_| "Invalid version history.")?;
                return Ok(Some(SyncContent {
                    versions,
                    preview: None,
                }));
            }
            SyncAction::Preview { entity, revision } => {
                let entity: vault_sync::scope::Entity = entity.into();
                let revision = Uuid::parse_str(&revision).map_err(|_| "Invalid revision.")?;
                let preview = self
                    .engine
                    .as_ref()
                    .ok_or("Finish enrollment first.")?
                    .preview(&entity, revision)
                    .await
                    .map_err(|e| e.to_string())?;
                return Ok(Some(SyncContent {
                    versions: vec![],
                    preview: Some(VersionPreview {
                        text: preview.text,
                        files: preview.manifest.files.keys().cloned().collect(),
                        deleted: preview.manifest.deleted,
                        changed: preview.changed,
                        captured_at: preview.captured_at,
                        missing_references: preview.missing_references,
                    }),
                }));
            }
            SyncAction::Restore { entity, revision } => {
                let entity: vault_sync::scope::Entity = entity.into();
                let revision = Uuid::parse_str(&revision).map_err(|_| "Invalid revision.")?;
                self.engine
                    .as_mut()
                    .ok_or("Finish enrollment first.")?
                    .restore(&self.store, entity, revision)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            SyncAction::Export { entity, revision } => {
                let entity: vault_sync::scope::Entity = entity.into();
                let revision = Uuid::parse_str(&revision).map_err(|_| "Invalid revision.")?;
                let path =
                    tokio::task::block_in_place(|| self.app.dialog().file().blocking_pick_folder())
                        .ok_or("Export cancelled.")?
                        .into_path()
                        .map_err(|e| e.to_string())?;
                self.engine
                    .as_ref()
                    .ok_or("Finish enrollment first.")?
                    .export(
                        &entity,
                        revision,
                        &path.join(format!("loofah-version-{revision}")),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
            }
            SyncAction::Connect => {
                if let Some(engine) = &self.engine {
                    engine.wait_idle().await.map_err(|e| e.to_string())?;
                }
                self.engine = None;
                if let Some(connection) = self.connection.as_mut() {
                    connection.paused = true;
                }
                self.save()?;
                self.phase("authorizing");
                let login = BrowserLogin::begin(Environment::Staging)
                    .await
                    .map_err(|e| e.to_string())?;
                let url = login.browser_url();
                {
                    let mut status = self.status.write().unwrap();
                    status.browser_url = Some(url.clone());
                    status.user_code = Some(login.user_code().into());
                }
                self.login = Some(login);
                self.phase("authorizing");
                self.app
                    .opener()
                    .open_url(url, None::<&str>)
                    .map_err(|e| e.to_string())?;
            }
            SyncAction::Cancel => {
                let was_pairing = self.status.read().unwrap().phase == "pairing";
                self.login = None;
                if let Some(pairing) = self.pairing.take() {
                    pairing.abort();
                }
                self.status.write().unwrap().pairing_code = None;
                self.phase(if self.engine.is_some() {
                    "paused"
                } else if was_pairing && self.account.is_some() {
                    "import_recovery_kit"
                } else {
                    "disconnected"
                });
            }
            SyncAction::Pause => {
                if let Some(engine) = &self.engine {
                    engine.wait_idle().await.map_err(|e| e.to_string())?;
                }
                let connection = self.connection.as_mut().ok_or("Connect first.")?;
                connection.paused = true;
                self.save()?;
                self.phase("paused");
            }
            SyncAction::Resume => {
                if self.engine.is_none() {
                    return Err("Finish device enrollment first.".into());
                }
                self.connection.as_mut().ok_or("Connect first.")?.paused = false;
                self.save()?;
                self.engine.as_mut().unwrap().reconcile();
                self.phase("connected");
            }
            SyncAction::ReconcileRecovery => self.reconcile_recovery().await?,
            SyncAction::Disconnect => {
                if let Some(engine) = &self.engine {
                    engine.wait_idle().await.map_err(|e| e.to_string())?;
                }
                if let Some(pairing) = self.pairing.take() {
                    pairing.abort();
                }
                self.status.write().unwrap().pairing_code = None;
                self.login = None;
                self.engine = None;
                self.kit = None;
                if let Some(connection) = &mut self.connection {
                    connection.paused = true;
                }
                self.save()?;
                if let Some(connection) = &self.connection {
                    tokio::task::block_in_place(|| {
                        Keychain::new(Environment::Staging).forget_session(connection.binding.vault)
                    })
                    .map_err(|e| e.to_string())?;
                }
                self.connection = None;
                let remote = self.remote.take();
                self.account = None;
                self.save()?;
                self.phase("disconnected");
                if let Some(remote) = remote {
                    remote.post::<serde_json::Value>("/auth/sign-out", &serde_json::json!({})).await.map_err(|_| "This Mac is disconnected. The server could not confirm session revocation; revoke this device from another signed-in Mac.".to_owned())?;
                }
            }
            SyncAction::SaveRecoveryKit => {
                let account = self.account.as_ref().ok_or("Connect first.")?;
                if account.account.enrollment_authority.is_some() {
                    return Err("This vault already has keys. Import your recovery kit or pair with a trusted Mac.".into());
                }
                if self.kit.is_none() {
                    self.kit = Some(
                        VaultKey::generate(account.account.vault_id).map_err(|e| e.to_string())?,
                    );
                }
                let path = tokio::task::block_in_place(|| {
                    self.app
                        .dialog()
                        .file()
                        .set_file_name("Loofah Staging Recovery.loofah-key")
                        .blocking_save_file()
                })
                .ok_or("Recovery kit save cancelled.")?
                .into_path()
                .map_err(|e| e.to_string())?;
                if path
                    .parent()
                    .ok_or("Invalid recovery kit location.")?
                    .canonicalize()
                    .map_err(|e| e.to_string())?
                    .starts_with(
                        self.store
                            .vault_base()
                            .canonicalize()
                            .map_err(|e| e.to_string())?,
                    )
                {
                    return Err("Save the recovery kit outside your vault.".into());
                }
                let kit = self.kit.as_ref().unwrap().recovery_kit();
                tokio::task::block_in_place(|| -> std::io::Result<()> {
                    use std::io::Write;
                    use std::os::unix::fs::OpenOptionsExt;
                    let mut file = std::fs::OpenOptions::new()
                        .create_new(true)
                        .write(true)
                        .mode(0o600)
                        .open(path)?;
                    file.write_all(&kit)?;
                    file.sync_all()
                })
                .map_err(|e| e.to_string())?;
                self.phase("confirm_recovery_kit");
            }
            SyncAction::ImportRecoveryKit => {
                let account = self.account.as_ref().ok_or("Connect first.")?;
                let path =
                    tokio::task::block_in_place(|| self.app.dialog().file().blocking_pick_file())
                        .ok_or("Recovery kit import cancelled.")?
                        .into_path()
                        .map_err(|e| e.to_string())?;
                if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 4096 {
                    return Err("Invalid recovery kit.".into());
                }
                let bytes = Zeroizing::new(tokio::fs::read(path).await.map_err(|e| e.to_string())?);
                let key = VaultKey::from_recovery_kit(&bytes, account.account.vault_id)
                    .map_err(|e| e.to_string())?;
                if let Some(generated) = &self.kit {
                    if *generated.recovery_kit() != *key.recovery_kit() {
                        return Err("Import the recovery kit you just saved.".into());
                    }
                }
                if account.account.enrollment_authority.is_none() && self.kit.is_none() {
                    return Err(
                        "Save a new recovery kit before confirming first enrollment.".into(),
                    );
                }
                if account
                    .account
                    .enrollment_authority
                    .as_ref()
                    .is_some_and(|expected| {
                        key.enrollment_authority()
                            .map(|authority| hex(authority.public_key.as_bytes()) != *expected)
                            .unwrap_or(true)
                    })
                {
                    return Err("This recovery kit belongs to another vault root.".into());
                }
                let device = Uuid::new_v4();
                let pair = KeyPair::generate().map_err(|_| "Device identity generation failed.")?;
                tokio::task::block_in_place(|| {
                    Keychain::new(Environment::Staging)
                        .save_confirmed_identity(&key, device, &pair, &bytes)
                })
                .map_err(|e| e.to_string())?;
                match tokio::fs::remove_file(self.root.join("pending-enrollment.json")).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
                self.kit = None;
                self.enroll(key, device, pair).await?;
            }
            SyncAction::PairNewMac => self.start_pairing(None).await?,
            SyncAction::ApproveMac { code } => self.start_pairing(Some(code)).await?,
            SyncAction::RefreshDevices => {
                #[derive(Deserialize)]
                struct Devices {
                    devices: Vec<Device>,
                }
                let devices: Devices = self
                    .remote
                    .as_ref()
                    .ok_or("Connect first.")?
                    .get("/devices")
                    .await
                    .map_err(|e| e.to_string())?;
                let account: AccountResponse = self
                    .remote
                    .as_ref()
                    .unwrap()
                    .get("/account")
                    .await
                    .map_err(|e| e.to_string())?;
                let mut status = self.status.write().unwrap();
                status.used_bytes = account.account.used_bytes;
                status.quota_bytes = account.account.quota_bytes;
                status.devices = devices.devices;
            }
            SyncAction::RevokeDevice { id } => {
                let id = Uuid::parse_str(&id).map_err(|_| "Invalid device.")?;
                self.remote
                    .as_ref()
                    .ok_or("Connect first.")?
                    .post::<serde_json::Value>(
                        &format!("/devices/{id}/revoke"),
                        &serde_json::json!({}),
                    )
                    .await
                    .map_err(|e| e.to_string())?;
            }
        }
        Ok(None)
    }
}

#[tauri::command]
#[specta::specta]
pub fn sync_status<R: tauri::Runtime>(app: AppHandle<R>) -> SyncStatus {
    app.try_state::<SyncState>()
        .map(|state| state.status.read().unwrap().clone())
        .unwrap_or_default()
}

#[tauri::command]
#[specta::specta]
pub async fn sync_action<R: tauri::Runtime>(
    app: AppHandle<R>,
    action: SyncAction,
) -> Result<Option<SyncContent>, String> {
    if !enabled(&app) {
        return Err("Sync is available only in Loofah Staging.".into());
    }
    let state = app.state::<SyncState>();
    let (reply, response) = oneshot::channel();
    state
        .commands
        .send(Request { action, reply })
        .await
        .map_err(|_| "Sync is unavailable.")?;
    response.await.map_err(|_| "Sync is unavailable.")?
}

mod pairing;
