use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use crate::{
    credentials::{CredentialStore, DeviceIdentity},
    crypto::VaultKey,
    engine::{Binding, Engine, atomic_json, now_ms},
    enrollment::{Challenge, ExpectedIdentity, Kind, hex},
    remote::{BrowserLogin, Environment, RemoteClient, TransferProgress},
};
use libsodium_rs::crypto_sign::KeyPair;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use zeroize::Zeroizing;

#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    pub enabled: bool,
    pub phase: String,
    pub vault_path: String,
    pub browser_url: Option<String>,
    pub user_code: Option<String>,
    pub pairing_code: Option<String>,
    pub pairing_role: Option<String>,
    pub account_email: Option<String>,
    pub account_id: Option<String>,
    pub vault_id: Option<String>,
    pub current_device_id: Option<String>,
    pub has_started: bool,
    pub up_to_date: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
    pub last_success: Option<u64>,
    pub pending_work: usize,
    pub active_transfers: usize,
    pub transferred_bytes: u64,
    pub transfer_bytes: u64,
    pub used_bytes: u64,
    pub quota_bytes: u64,
    pub conflicts: Vec<SyncConflict>,
    pub devices: Vec<Device>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct Device {
    pub id: String,
    pub name: Option<String>,
    pub public_key: String,
    pub enrolled_at: u64,
    pub revoked_at: Option<u64>,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SyncEntity {
    Session { id: String },
    People,
    Tags,
    Tasks,
}
impl From<crate::scope::Entity> for SyncEntity {
    fn from(value: crate::scope::Entity) -> Self {
        use crate::scope::Entity;
        match value {
            Entity::Session(id) => Self::Session { id },
            Entity::People => Self::People,
            Entity::Tags => Self::Tags,
            Entity::Tasks => Self::Tasks,
        }
    }
}
impl From<SyncEntity> for crate::scope::Entity {
    fn from(value: SyncEntity) -> Self {
        match value {
            SyncEntity::Session { id } => Self::Session(id),
            SyncEntity::People => Self::People,
            SyncEntity::Tags => Self::Tags,
            SyncEntity::Tasks => Self::Tasks,
        }
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncConflict {
    pub entity: SyncEntity,
    pub local: String,
    pub cloud: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncAction {
    Connect,
    ConnectCli,
    RenameDevice {
        id: String,
        name: String,
    },
    Cancel,
    Pause,
    Resume,
    ReconcileRecovery,
    Disconnect,
    SaveRecoveryKit {
        path: PathBuf,
    },
    ImportRecoveryKit {
        path: PathBuf,
    },
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
        destination: PathBuf,
    },
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Version {
    pub id: String,
    pub operation: String,
    pub device_id: String,
    pub created_at: u64,
    pub current: Option<u8>,
    pub pinned: u8,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct VersionPreview {
    pub title: Option<String>,
    pub device_id: String,
    pub created_at: u64,
    pub text: String,
    pub files: Vec<String>,
    pub changed: Vec<String>,
    pub captured_at: Option<u64>,
    pub missing_references: Vec<String>,
    pub deleted: bool,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct SyncContent {
    pub cancelled: bool,
    pub versions: Vec<Version>,
    pub preview: Option<VersionPreview>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Connection {
    version: u32,
    binding: Binding,
    paused: bool,
    #[serde(default)]
    has_started: bool,
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
    email: String,
    user: String,
    session: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingEnrollment {
    challenge: Challenge,
    authority_signature: String,
}

pub struct Runtime {
    credentials: Arc<dyn CredentialStore>,
    client: crate::remote::AuthorizationClient,
    next_tick: tokio::time::Instant,
    reconciled_at: std::time::Instant,
    failures: u32,
    store: Arc<hypr_vault_write::SessionStore>,
    root: PathBuf,
    pub(crate) status: Arc<RwLock<SyncStatus>>,
    connection: Option<Connection>,
    login: Option<BrowserLogin>,
    account: Option<AccountResponse>,
    remote: Option<RemoteClient>,
    engine: Option<Engine>,
    kit: Option<VaultKey>,
    pairing: Option<tokio::task::JoinHandle<Result<Option<DeviceIdentity>, String>>>,
    pub(crate) progress: TransferProgress,
    refreshed_at: std::time::Instant,
    refresh_error: Option<String>,
}

fn sync_error(error: crate::Error) -> String {
    error.to_string()
}
impl Runtime {
    fn report(&mut self, result: Result<(), String>) {
        let mut status = self.status.write().unwrap();
        if let Err(error) = result {
            status.error = Some(error);
        }
        status.has_started = self.connection.as_ref().is_some_and(|c| c.has_started);
        status.up_to_date = status.phase == "connected"
            && status.error.is_none()
            && self.engine.as_ref().is_some_and(Engine::up_to_date);
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
    fn connected_phase(&self) -> &'static str {
        if self.engine.is_none() {
            "import_recovery_kit"
        } else if self.connection.as_ref().is_some_and(|c| !c.paused) {
            "connected"
        } else {
            "paused"
        }
    }
    async fn refresh_account(&mut self) -> Result<(), String> {
        self.refreshed_at = std::time::Instant::now();
        let remote = self.remote.as_ref().ok_or("Sign in to continue.")?;
        #[derive(Deserialize)]
        struct Devices {
            devices: Vec<Device>,
        }
        let result: crate::Result<(Devices, AccountResponse)> = async {
            let devices = remote.get("/devices").await?;
            let account = remote.get("/account").await?;
            Ok((devices, account))
        }
        .await;
        let (devices, account) = match result {
            Ok(value) => value,
            Err(error) => {
                if matches!(&error, crate::Error::Cloud { status: 401, .. }) {
                    if let Some(engine) = &self.engine {
                        let _ = engine.wait_idle().await;
                    }
                    if let Some(connection) = self.connection.as_mut() {
                        connection.paused = true;
                    }
                    self.save()?;
                    self.phase("authorization_required");
                }
                let message = sync_error(error);
                self.refresh_error = Some(message.clone());
                return Err(message);
            }
        };
        if self.connection.as_ref().is_some_and(|connection| {
            connection.binding.generation != account.account.recovery_generation
        }) {
            if let Some(engine) = &self.engine {
                let _ = engine.wait_idle().await;
            }
            if let Some(connection) = self.connection.as_mut() {
                connection.paused = true;
            }
            self.save()?;
            self.phase("recovery_required");
            return Err("Cloud sync has changed. Compare this device with the cloud before continuing. Your local files are safe.".into());
        }
        let mut status = self.status.write().unwrap();
        if self.refresh_error.take().as_ref() == status.error.as_ref() {
            status.error = None;
        }
        status.used_bytes = account.account.used_bytes;
        status.quota_bytes = account.account.quota_bytes;
        status.account_email = Some(account.identity.email);
        status.devices = devices.devices;
        Ok(())
    }
    async fn refresh_after_change(&mut self) {
        if let Err(error) = self.refresh_account().await {
            self.status.write().unwrap().error = Some(error);
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
    pub async fn load(&mut self) -> Result<(), String> {
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
            let credential = match tokio::task::block_in_place(|| {
                self.credentials.session(connection.binding.vault)
            }) {
                Ok(credential) => credential,
                Err(error) => {
                    self.phase("needs_unlock");
                    return Err(error.to_string());
                }
            };
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
            self.credentials
                .save_session(account.account.vault_id, credential.as_bytes())
        })
        .map_err(|e| e.to_string())?;
        self.connection = Some(Connection {
            version: 1,
            paused: true,
            has_started: self.connection.as_ref().is_some_and(|c| c.has_started),
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
                "Cloud sync has changed. Compare this device with the cloud before continuing. Your local files are safe."
                    .into(),
            );
        }
        {
            let mut status = self.status.write().unwrap();
            status.account_email = Some(account.identity.email.clone());
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
        let identity =
            tokio::task::block_in_place(|| self.credentials.identity(connection.binding.vault));
        match identity {
            Ok((key, device, pair)) => self.enroll(key, device, pair).await,
            Err(crate::Error::CredentialMissing) => {
                let account = &self.account.as_ref().unwrap().account;
                let phase = if account.enrollment_authority.is_some() {
                    "import_recovery_kit"
                } else if self
                    .credentials
                    .read(&format!("pending-kit:{}", account.vault_id))
                    .map_err(sync_error)?
                    .is_some()
                {
                    "confirm_recovery_kit"
                } else {
                    "save_recovery_kit"
                };
                self.phase(phase);
                Ok(())
            }
            Err(error) => {
                self.phase("needs_unlock");
                Err(error.to_string())
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
            let name = format!("pending-enrollment:{}", key.vault());
            let saved = self.credentials.read(&name).map_err(|e| e.to_string())?;
            let bytes = match saved {
                Some(bytes) => Some(bytes),
                None => match tokio::fs::read(self.root.join("pending-enrollment.json")).await {
                    Ok(bytes) => {
                        let bytes = Zeroizing::new(bytes);
                        self.credentials
                            .write(&name, &bytes)
                            .map_err(|e| e.to_string())?;
                        tokio::fs::remove_file(self.root.join("pending-enrollment.json"))
                            .await
                            .map_err(|e| e.to_string())?;
                        Some(bytes)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => return Err(error.to_string()),
                },
            };
            bytes
                .map(|bytes| {
                    serde_json::from_slice(&bytes).map_err(
                        |_| "Interrupted enrollment needs recovery import or a new pairing.",
                    )
                })
                .transpose()?
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
        self.account.as_mut().unwrap().account.enrollment_authority = Some(authority);
        self.credentials
            .remove(&format!("pending-enrollment:{}", key.vault()))
            .map_err(|e| e.to_string())?;
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
            Err(crate::Error::RecoveryRequired) => {
                self.phase("recovery_required");
                return Err("Cloud sync has changed. Compare this device with the cloud to continue. Your previous sync state will be kept.".into());
            }
            Err(error) => return Err(error.to_string()),
        });
        let previously_started = !paused
            || self
                .engine
                .as_ref()
                .is_some_and(|engine| engine.last_success().is_some());
        self.connection.as_mut().unwrap().has_started |= previously_started;
        self.status.write().unwrap().current_device_id = Some(device.to_string());
        self.save()?;
        self.phase(if paused { "paused" } else { "connected" });
        self.refresh_after_change().await;
        Ok(())
    }
    async fn reconcile_recovery(&mut self) -> Result<(), String> {
        let binding = self
            .connection
            .as_ref()
            .ok_or("Connect your account first.")?
            .binding
            .clone();
        let credential = tokio::task::block_in_place(|| self.credentials.session(binding.vault))
            .map_err(|e| e.to_string())?;
        let credential =
            std::str::from_utf8(&credential).map_err(|_| "Invalid account session.")?;
        let remote =
            RemoteClient::new(Environment::Staging, credential, None).map_err(|e| e.to_string())?;
        let current: AccountResponse = remote.get("/account").await.map_err(|e| e.to_string())?;
        if current.identity.user != binding.account || current.account.vault_id != binding.vault {
            return Err("Disconnect before changing accounts.".into());
        }
        let (key, _, _) = tokio::task::block_in_place(|| self.credentials.identity(binding.vault))
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
            crate::replica::LocalReplica::open(self.store.vault_base(), &directory)
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

    pub async fn action(&mut self, action: SyncAction) -> Result<Option<SyncContent>, String> {
        {
            let mut status = self.status.write().unwrap();
            status.notice = None;
            status.error = None;
        }
        let cli_login = matches!(&action, SyncAction::ConnectCli);
        match action {
            SyncAction::RenameDevice { id, name } => {
                let id = Uuid::parse_str(&id).map_err(|_| "Invalid device.")?;
                self.remote
                    .as_ref()
                    .ok_or("Sign in first.")?
                    .post::<serde_json::Value>(
                        &format!("/devices/{id}/name"),
                        &serde_json::json!({ "name": name }),
                    )
                    .await
                    .map_err(sync_error)?;
                for device in &mut self.status.write().unwrap().devices {
                    if device.id == id.to_string() {
                        device.name = Some(name.trim().into());
                    }
                }
                self.refresh_after_change().await;
            }
            SyncAction::PurgeVersion { revision } => {
                let revision = Uuid::parse_str(&revision).map_err(|_| "Invalid revision.")?;
                self.engine
                    .as_ref()
                    .ok_or("Finish Sync enrollment with a recovery kit or trusted device first.")?
                    .purge(&[revision])
                    .await
                    .map_err(sync_error)?;
                self.refresh_after_change().await;
            }
            SyncAction::History { entity, before } => {
                let entity: crate::scope::Entity = entity.into();
                entity.root().map_err(|e| e.to_string())?;
                let value = self
                    .engine
                    .as_ref()
                    .ok_or("Finish Sync enrollment with a recovery kit or trusted device first.")?
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
                    cancelled: false,
                    versions,
                    preview: None,
                }));
            }
            SyncAction::Preview { entity, revision } => {
                let entity: crate::scope::Entity = entity.into();
                let revision = Uuid::parse_str(&revision).map_err(|_| "Invalid revision.")?;
                let preview = self
                    .engine
                    .as_ref()
                    .ok_or("Finish Sync enrollment with a recovery kit or trusted device first.")?
                    .preview(&entity, revision)
                    .await
                    .map_err(sync_error)?;
                return Ok(Some(SyncContent {
                    cancelled: false,
                    versions: vec![],
                    preview: Some(VersionPreview {
                        title: preview.title,
                        device_id: preview.device_id.to_string(),
                        created_at: preview.created_at,
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
                let entity: crate::scope::Entity = entity.into();
                let revision = Uuid::parse_str(&revision).map_err(|_| "Invalid revision.")?;
                self.engine
                    .as_mut()
                    .ok_or("Finish Sync enrollment with a recovery kit or trusted device first.")?
                    .restore(&self.store, entity, revision)
                    .await
                    .map_err(sync_error)?;
            }
            SyncAction::Export {
                entity,
                revision,
                destination,
            } => {
                let entity: crate::scope::Entity = entity.into();
                let revision = Uuid::parse_str(&revision).map_err(|_| "Invalid revision.")?;
                self.engine
                    .as_ref()
                    .ok_or("Finish Sync enrollment with a recovery kit or trusted device first.")?
                    .export(&entity, revision, &destination)
                    .await
                    .map_err(|e| e.to_string())?;
            }
            SyncAction::Connect | SyncAction::ConnectCli => {
                self.client = if cli_login {
                    crate::remote::AuthorizationClient::Cli
                } else {
                    crate::remote::AuthorizationClient::Desktop
                };
                if let Some(engine) = &self.engine {
                    engine.wait_idle().await.map_err(|e| e.to_string())?;
                }
                self.engine = None;
                if let Some(connection) = self.connection.as_mut() {
                    connection.paused = true;
                }
                self.save()?;
                {
                    let mut status = self.status.write().unwrap();
                    status.browser_url = None;
                    status.user_code = None;
                }
                self.phase("authorizing");
                let login = BrowserLogin::begin_with_client(Environment::Staging, self.client)
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
            }
            SyncAction::Cancel => {
                let was_pairing = self.status.read().unwrap().phase == "pairing";
                self.login = None;
                {
                    let mut status = self.status.write().unwrap();
                    status.browser_url = None;
                    status.user_code = None;
                }
                if let Some(pairing) = self.pairing.take() {
                    pairing.abort();
                }
                {
                    let mut status = self.status.write().unwrap();
                    status.pairing_code = None;
                    status.pairing_role = None;
                }
                self.phase(if self.engine.is_some() {
                    self.connected_phase()
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
                let Some(connection) = self.connection.as_mut() else {
                    return Ok(None);
                };
                connection.paused = true;
                self.save()?;
                self.phase("paused");
            }
            SyncAction::Resume => {
                if self.engine.is_none() && self.connection.is_some() {
                    self.load().await?;
                }
                if self.engine.is_none() {
                    return Err(
                        "Finish Sync enrollment with a recovery kit or trusted device first."
                            .into(),
                    );
                }
                let connection = self.connection.as_mut().ok_or("Connect first.")?;
                connection.paused = false;
                connection.has_started = true;
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
                        self.credentials.forget_session(connection.binding.vault)
                    })
                    .map_err(|e| e.to_string())?;
                }
                self.connection = None;
                let remote = self.remote.take();
                self.account = None;
                self.save()?;
                self.phase("disconnected");
                {
                    let mut status = self.status.write().unwrap();
                    status.account_email = None;
                    status.current_device_id = None;
                    status.pairing_role = None;
                    status.last_success = None;
                    status.pending_work = 0;
                    status.conflicts.clear();
                    status.devices.clear();
                    status.used_bytes = 0;
                    status.quota_bytes = 0;
                }
                if let Some(remote) = remote {
                    remote.post::<serde_json::Value>("/auth/sign-out", &serde_json::json!({})).await.map_err(|_| "This device is disconnected. The server could not confirm session revocation; revoke this device from another signed-in device.".to_owned())?;
                }
            }
            SyncAction::SaveRecoveryKit { path } => {
                let account = self.account.as_ref().ok_or("Connect first.")?;
                if account.account.enrollment_authority.is_some() {
                    return Err("This vault already has keys. Import your recovery kit or pair with a trusted device.".into());
                }
                if self.kit.is_none() {
                    let name = format!("pending-kit:{}", account.account.vault_id);
                    self.kit = Some(
                        match self.credentials.read(&name).map_err(|e| e.to_string())? {
                            Some(bytes) => {
                                VaultKey::from_recovery_kit(&bytes, account.account.vault_id)
                            }
                            None => VaultKey::generate(account.account.vault_id),
                        }
                        .map_err(|e| e.to_string())?,
                    );
                    self.credentials
                        .write(&name, &self.kit.as_ref().unwrap().recovery_kit())
                        .map_err(|e| e.to_string())?;
                }
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
            SyncAction::ImportRecoveryKit { path } => {
                let account = self.account.as_ref().ok_or("Connect first.")?;
                if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 4096 {
                    return Err("Invalid recovery kit.".into());
                }
                let bytes = Zeroizing::new(tokio::fs::read(path).await.map_err(|e| e.to_string())?);
                if self.kit.is_none()
                    && let Some(pending) = self
                        .credentials
                        .read(&format!("pending-kit:{}", account.account.vault_id))
                        .map_err(|e| e.to_string())?
                {
                    self.kit = Some(
                        VaultKey::from_recovery_kit(&pending, account.account.vault_id)
                            .map_err(|e| e.to_string())?,
                    );
                }
                let key = VaultKey::from_recovery_kit(&bytes, account.account.vault_id)
                    .map_err(|e| e.to_string())?;
                if let Some(generated) = &self.kit
                    && *generated.recovery_kit() != *key.recovery_kit()
                {
                    return Err("Import the recovery kit you just saved.".into());
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
                    self.credentials
                        .save_confirmed_identity(&key, device, &pair, &bytes)
                })
                .map_err(|e| e.to_string())?;
                match tokio::fs::remove_file(self.root.join("pending-enrollment.json")).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.to_string()),
                }
                self.credentials
                    .remove(&format!("pending-kit:{}", account.account.vault_id))
                    .map_err(|e| e.to_string())?;
                self.credentials
                    .remove(&format!("pending-enrollment:{}", account.account.vault_id))
                    .map_err(|e| e.to_string())?;
                self.kit = None;
                self.enroll(key, device, pair).await?;
            }
            SyncAction::PairNewMac => self.start_pairing(None).await?,
            SyncAction::ApproveMac { code } => self.start_pairing(Some(code)).await?,
            SyncAction::RefreshDevices => self.refresh_account().await?,
            SyncAction::RevokeDevice { id } => {
                if let Some(engine) = &self.engine {
                    engine.wait_idle().await.map_err(sync_error)?;
                }
                let id = Uuid::parse_str(&id).map_err(|_| "Invalid device.")?;
                self.remote
                    .as_ref()
                    .ok_or("Connect first.")?
                    .post::<serde_json::Value>(
                        &format!("/devices/{id}/revoke"),
                        &serde_json::json!({}),
                    )
                    .await
                    .map_err(sync_error)?;
                for device in &mut self.status.write().unwrap().devices {
                    if device.id == id.to_string() {
                        device.revoked_at = Some(now_ms());
                    }
                }
                if self.status.read().unwrap().current_device_id.as_deref() == Some(&id.to_string())
                {
                    self.engine = None;
                    self.connection.as_mut().unwrap().paused = true;
                    self.save()?;
                    self.phase("authorization_required");
                    let mut status = self.status.write().unwrap();
                    status.notice = Some(
                        "This device's access was removed. Your local files are still here.".into(),
                    );
                    for device in &mut status.devices {
                        if device.id == id.to_string() {
                            device.revoked_at = Some(now_ms());
                        }
                    }
                } else {
                    self.refresh_after_change().await;
                }
            }
        }
        Ok(None)
    }
}

mod pairing;

impl Runtime {
    pub fn new(
        root: PathBuf,
        store: Arc<hypr_vault_write::SessionStore>,
        credentials: Arc<dyn CredentialStore>,
        client: crate::remote::AuthorizationClient,
    ) -> Self {
        let status = Arc::new(RwLock::new(SyncStatus {
            enabled: true,
            phase: "disconnected".into(),
            vault_path: store.vault_base().display().to_string(),
            ..Default::default()
        }));
        Self {
            root,
            store,
            credentials,
            client,
            next_tick: tokio::time::Instant::now(),
            reconciled_at: std::time::Instant::now(),
            failures: 0,
            status,
            connection: None,
            login: None,
            account: None,
            remote: None,
            engine: None,
            kit: None,
            pairing: None,
            progress: TransferProgress::default(),
            refreshed_at: std::time::Instant::now(),
            refresh_error: None,
        }
    }

    pub fn status(&mut self) -> SyncStatus {
        self.report(Ok(()));
        let progress = self.progress.snapshot();
        let mut status = self.status.write().unwrap();
        status.account_id = self
            .connection
            .as_ref()
            .map(|connection| connection.binding.account.clone());
        status.vault_id = self
            .connection
            .as_ref()
            .map(|connection| connection.binding.vault.to_string());
        status.active_transfers = progress.active;
        status.transferred_bytes = progress.completed;
        status.transfer_bytes = progress.total;
        status.clone()
    }

    pub fn report_error(&mut self, error: String) {
        self.report(Err(error));
    }

    pub async fn shutdown(&mut self) -> Result<(), String> {
        if let Some(pairing) = self.pairing.take() {
            pairing.abort();
        }
        if let Some(engine) = &self.engine {
            engine.wait_idle().await.map_err(sync_error)?;
        }
        Ok(())
    }

    pub fn changed(&mut self) {
        if let Some(engine) = self.engine.as_mut() {
            engine.reconcile();
        }
    }

    pub async fn advance(&mut self) -> Result<(), String> {
        if let Some(login) = self.login.as_mut() {
            match login.poll().await {
                Ok(Some(credential)) => {
                    self.login = None;
                    self.phase("authorization_required");
                    self.accept_login(&credential).await?;
                }
                Ok(None) => {}
                Err(error) => {
                    self.login = None;
                    self.phase("authorization_required");
                    let mut status = self.status.write().unwrap();
                    status.browser_url = None;
                    status.user_code = None;
                    return Err(sync_error(error));
                }
            }
            return Ok(());
        }
        if let Some(pairing) = self.pairing.as_mut() {
            if !pairing.is_finished() {
                return Ok(());
            }
            let result = pairing.await.map_err(|_| "Pairing cancelled.")?;
            self.pairing = None;
            {
                let mut status = self.status.write().unwrap();
                status.pairing_code = None;
                status.pairing_role = None;
            }
            self.phase(self.connected_phase());
            match result? {
                Some((key, device, pair)) => self.enroll(key, device, pair).await?,
                None => {
                    self.phase(self.connected_phase());
                    self.refresh_after_change().await;
                }
            }
            return Ok(());
        }
        tokio::time::sleep_until(self.next_tick).await;
        self.next_tick = tokio::time::Instant::now() + Duration::from_secs(10);
        if self.reconciled_at.elapsed() >= Duration::from_secs(30) {
            self.changed();
            self.reconciled_at = std::time::Instant::now();
        }
        if self.connection.is_some()
            && self.remote.is_none()
            && self.status.read().unwrap().phase != "needs_unlock"
        {
            self.load().await?;
        }
        if self.remote.is_some() && self.refreshed_at.elapsed() >= Duration::from_secs(60) {
            self.refresh_account().await?;
        }
        if self.connection.as_ref().is_some_and(|value| !value.paused)
            && let Some(engine) = self.engine.as_mut()
        {
            let result = engine.tick(&self.store).await;
            if matches!(&result, Err(crate::Error::Cloud { status: 401, .. }))
                || matches!(&result, Err(crate::Error::Cloud { code, .. }) if code == "recovery_required")
            {
                self.connection.as_mut().unwrap().paused = true;
                self.save()?;
                self.phase(
                    if matches!(&result, Err(crate::Error::Cloud { status: 401, .. })) {
                        "authorization_required"
                    } else {
                        "recovery_required"
                    },
                );
            }
            if result.is_ok() {
                self.status.write().unwrap().error = None;
            }
            self.failures = if result.is_ok() {
                0
            } else {
                (self.failures + 1).min(5)
            };
            self.next_tick = tokio::time::Instant::now()
                + Duration::from_secs((10u64 << self.failures).min(300));
            result.map_err(sync_error)?;
        }
        Ok(())
    }

    pub async fn once(&mut self, timeout: Duration) -> Result<(), String> {
        self.status.write().unwrap().error = None;
        if self.engine.is_none() {
            return Err("Finish Sync enrollment first.".into());
        }
        self.connection.as_mut().unwrap().has_started = true;
        self.save()?;
        self.engine.as_mut().unwrap().reconcile();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let engine = self.engine.as_mut().unwrap();
            let result = tokio::time::timeout_at(deadline, engine.tick(&self.store)).await;
            engine.wait_idle().await.map_err(sync_error)?;
            result
                .map_err(|_| format!("Sync timed out with {} pending items; local files are preserved. Active recordings: {}.", engine.pending_work(), hypr_vault_read::transaction::VaultTransaction::any_recording(self.store.vault_base()).unwrap_or(true)))?
                .map_err(sync_error)?;
            if !engine.conflicts().is_empty() {
                return Err(format!(
                    "Sync has {} unresolved conflicts and {} pending items; inspect both revisions before resolving.",
                    engine.conflicts().len(),
                    engine.pending_work()
                ));
            }
            if engine.up_to_date() {
                return Ok(());
            }
            if tokio::time::Instant::now() >= deadline {
                return Err("Sync timed out; recording deferrals or queued work remain.".into());
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    pub async fn resolve(
        &mut self,
        entity: SyncEntity,
        local: Uuid,
        cloud: Uuid,
        selected: Uuid,
    ) -> Result<(), String> {
        let entity: crate::scope::Entity = entity.into();
        let engine = self
            .engine
            .as_mut()
            .ok_or("Finish Sync enrollment first.")?;
        if !engine.conflicts().iter().any(|conflict| {
            conflict.entity == entity && conflict.local == local && conflict.cloud == cloud
        }) || (selected != local && selected != cloud)
        {
            return Err("Conflict revisions changed; review both revisions again.".into());
        }
        engine
            .restore(&self.store, entity, selected)
            .await
            .map_err(sync_error)
    }
}

impl Runtime {
    pub async fn unlock_from_file(&mut self, path: &std::path::Path) -> Result<(), String> {
        self.configure_credentials(None, Some(path)).await
    }

    pub async fn configure_credentials(
        &mut self,
        backend: Option<crate::credentials::Backend>,
        unlock_file: Option<&std::path::Path>,
    ) -> Result<(), String> {
        if self.login.is_some() || self.pairing.is_some() {
            return Err("Finish or cancel the active connection or pairing first.".into());
        }
        let secret = unlock_file
            .map(crate::credentials::file::read_secret)
            .transpose()
            .map_err(sync_error)?;
        let credentials = crate::credentials::open(
            &self.root,
            Environment::Staging,
            backend,
            secret.as_deref().map(|bytes| bytes.as_slice()),
        )
        .map_err(sync_error)?;
        self.shutdown().await?;
        self.engine = None;
        self.credentials = credentials;
        self.load().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn attached_owner_configures_credentials_without_replacing_an_existing_store() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let vault = root.join("vault");
        std::fs::create_dir(&vault).unwrap();
        let unlock = root.join("unlock");
        std::fs::write(&unlock, b"local unlock").unwrap();
        std::fs::set_permissions(&unlock, std::fs::Permissions::from_mode(0o600)).unwrap();
        let mut runtime = Runtime::new(
            root.join("state"),
            Arc::new(hypr_vault_write::SessionStore::new(vault)),
            Arc::new(crate::credentials::Locked),
            crate::remote::AuthorizationClient::Cli,
        );
        runtime
            .configure_credentials(
                Some(crate::credentials::Backend::EncryptedFile),
                Some(&unlock),
            )
            .await
            .unwrap();
        runtime.credentials.write("test", b"retained").unwrap();
        assert!(
            runtime
                .configure_credentials(Some(crate::credentials::Backend::Keychain), None)
                .await
                .is_err()
        );
        std::fs::write(&unlock, b"wrong unlock").unwrap();
        assert!(runtime.unlock_from_file(&unlock).await.is_err());
        assert_eq!(
            runtime
                .credentials
                .read("test")
                .unwrap()
                .unwrap()
                .as_slice(),
            b"retained"
        );
        assert!(!runtime.status().has_started);
    }

    #[test]
    fn legacy_connection_keeps_pause_preference_and_start_consent() {
        let mut connection: Connection = serde_json::from_value(serde_json::json!({
            "version": 1, "paused": true,
            "binding": { "account": "account", "vault": Uuid::new_v4(), "generation": Uuid::new_v4(), "local": "/tmp/test-vault" }
        })).unwrap();
        assert!(connection.paused);
        assert!(!connection.has_started);
        connection.has_started = true;
        connection.paused = false;
        let restored: Connection =
            serde_json::from_slice(&serde_json::to_vec(&connection).unwrap()).unwrap();
        assert!(restored.has_started);
        assert!(!restored.paused);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn first_device_kit_is_durable_before_reimport_and_stays_paused() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().canonicalize().unwrap();
        let vault = directory.join("vault");
        std::fs::create_dir(&vault).unwrap();
        let state = directory.join("state");
        let credentials = crate::credentials::open(
            &state,
            Environment::Staging,
            Some(crate::credentials::Backend::EncryptedFile),
            Some(b"local unlock"),
        )
        .unwrap();
        let account_id = Uuid::new_v4();
        let make_runtime = || {
            let mut runtime = Runtime::new(
                state.clone(),
                Arc::new(hypr_vault_write::SessionStore::new(vault.clone())),
                credentials.clone(),
                crate::remote::AuthorizationClient::Cli,
            );
            runtime.account = Some(AccountResponse {
                account: Account {
                    vault_id: account_id,
                    recovery_generation: Uuid::new_v4(),
                    enrollment_authority: None,
                    used_bytes: 0,
                    quota_bytes: 1000,
                },
                identity: Identity {
                    email: "test@example.invalid".into(),
                    user: "test".into(),
                    session: "test".into(),
                },
            });
            runtime
        };
        let mut first = make_runtime();
        assert!(
            first
                .action(SyncAction::SaveRecoveryKit {
                    path: vault.join("unsafe-kit")
                })
                .await
                .is_err()
        );
        let saved = directory.join("kit");
        first
            .action(SyncAction::SaveRecoveryKit {
                path: saved.clone(),
            })
            .await
            .unwrap();
        assert!(!first.status().has_started);
        drop(first);
        let mut restarted = make_runtime();
        let copy = directory.join("kit-again");
        restarted
            .action(SyncAction::SaveRecoveryKit { path: copy.clone() })
            .await
            .unwrap();
        assert_eq!(std::fs::read(&saved).unwrap(), std::fs::read(copy).unwrap());
        let other = VaultKey::generate(account_id).unwrap();
        let wrong = directory.join("wrong-kit");
        std::fs::write(&wrong, &*other.recovery_kit()).unwrap();
        assert!(
            restarted
                .action(SyncAction::ImportRecoveryKit { path: wrong })
                .await
                .err()
                .unwrap()
                .contains("just saved")
        );
        assert!(restarted.engine.is_none());
    }
}
