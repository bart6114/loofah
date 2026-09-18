use std::fs::{self, File, OpenOptions};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

use crate::{
    Error, Result, credentials,
    enrollment::hex,
    runtime::{Runtime, SyncAction, SyncContent, SyncEntity, SyncStatus},
};

pub const PROTOCOL_VERSION: u32 = 1;
const MAX_FRAME: usize = 1024 * 1024;

pub fn directory(root: &Path, vault: &Path) -> Result<PathBuf> {
    let local = vault.canonicalize()?;
    Ok(root.join(hex(&Sha256::digest(local.as_os_str().as_encoded_bytes()))))
}

pub fn staging_root() -> Result<PathBuf> {
    dirs::data_dir()
        .map(|path| path.join("io.loofah.sync.staging"))
        .ok_or_else(|| Error::Invalid("application storage is unavailable".into()))
}

pub fn recover(vault: &Path) -> Result<()> {
    let directory = directory(&staging_root()?, vault)?.join("replica");
    if directory.exists() {
        crate::replica::LocalReplica::open(vault, &directory)?.recover()?;
    } else {
        hypr_vault_read::transaction::VaultTransaction::ensure_ready(vault)
            .map_err(|_| Error::RecoveryRequired)?;
    }
    Ok(())
}

fn socket_path(directory: &Path) -> Result<PathBuf> {
    let root = PathBuf::from("/tmp").canonicalize()?.join(format!(
        "loofah-sync-{}",
        rustix::process::geteuid().as_raw()
    ));
    credentials::file::private_directory(&root)?;
    Ok(root.join(format!(
        "{}.sock",
        &hex(&Sha256::digest(directory.as_os_str().as_encoded_bytes()))[..32]
    )))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Status,
    Unlock {
        path: PathBuf,
    },
    ConfigureCredentials {
        backend: Option<credentials::Backend>,
        unlock_file: Option<PathBuf>,
    },
    Quiesce,
    Action {
        action: SyncAction,
    },
    Changed,
    Once {
        timeout_seconds: u64,
    },
    Resolve {
        entity: SyncEntity,
        local: Uuid,
        cloud: Uuid,
        selected: Uuid,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    protocol: u32,
    command: Command,
}

#[derive(Serialize, Deserialize)]
pub struct Response {
    pub protocol: u32,
    pub owner: Uuid,
    pub generation: u64,
    pub status: SyncStatus,
    pub content: Option<SyncContent>,
    pub error: Option<String>,
}

pub async fn request(directory: &Path, command: Command) -> Result<Option<Response>> {
    let path = socket_path(directory)?;
    let mut stream = match UnixStream::connect(path).await {
        Ok(stream) => stream,
        Err(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None);
        }
        Err(error) => return Err(error.into()),
    };
    check_peer(&stream)?;
    write_frame(
        &mut stream,
        &Request {
            protocol: PROTOCOL_VERSION,
            command,
        },
    )
    .await?;
    let response: Response = read_frame(&mut stream).await?;
    if response.protocol != PROTOCOL_VERSION {
        return Err(Error::Invalid(
            "incompatible Sync protocol; upgrade both desktop and CLI".into(),
        ));
    }
    Ok(Some(response))
}

pub struct Owner {
    listener_task: tokio::task::JoinHandle<()>,
    requests: tokio::sync::mpsc::Receiver<(Command, tokio::sync::oneshot::Sender<Response>)>,
    socket: PathBuf,
    runtime: Runtime,
    id: Uuid,
    generation: Arc<std::sync::atomic::AtomicU64>,
    quiescing: bool,
    dirty: Arc<std::sync::atomic::AtomicBool>,
    changes: tokio::sync::mpsc::UnboundedReceiver<(hypr_vault_write::IndexEntity, Vec<String>)>,
    _lock: File,
}
impl Drop for Owner {
    fn drop(&mut self) {
        self.listener_task.abort();
        let _ = fs::remove_file(&self.socket);
    }
}

impl Owner {
    pub async fn acquire(
        directory: &Path,
        store: Arc<hypr_vault_write::SessionStore>,
        credentials: Arc<dyn credentials::CredentialStore>,
        client: crate::remote::AuthorizationClient,
    ) -> Result<Option<Self>> {
        credentials::file::private_directory(directory)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(directory.join("owner.lock"))?;
        if let Err(error) = lock.try_lock() {
            if matches!(error, std::fs::TryLockError::WouldBlock) {
                return Ok(None);
            }
            return Err(Error::Invalid("cannot acquire Sync owner lock".into()));
        }
        recover(store.vault_base())?;
        let socket = socket_path(directory)?;
        match fs::remove_file(&socket) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let listener = UnixListener::bind(&socket)?;
        fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
        let changes = store.subscribe_index_changes();
        let mut runtime = Runtime::new(directory.to_owned(), store, credentials, client);
        if let Err(error) = runtime.load().await {
            runtime.report_error(error);
        }
        let id = Uuid::new_v4();
        let generation = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let (sender, requests) = tokio::sync::mpsc::channel(16);
        let status = runtime.status.clone();
        let progress = runtime.progress.clone();
        let published_generation = generation.clone();
        let dirty = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let pending_changes = dirty.clone();
        let listener_task = tokio::spawn(async move {
            let clients = Arc::new(tokio::sync::Semaphore::new(32));
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let Ok(permit) = clients.clone().try_acquire_owned() else {
                    continue;
                };
                let sender = sender.clone();
                let status = status.clone();
                let progress = progress.clone();
                let generation = published_generation.clone();
                let dirty = pending_changes.clone();
                tokio::spawn(async move {
                    let _permit = permit;
                    let result: Result<()> = async {
                        check_peer(&stream)?;
                        let request: Request =
                            tokio::time::timeout(Duration::from_secs(5), read_frame(&mut stream))
                                .await
                                .map_err(|_| Error::Invalid("Sync request timed out".into()))??;
                        let response = if request.protocol != PROTOCOL_VERSION
                            || matches!(&request.command, Command::Status | Command::Changed)
                        {
                            if request.protocol == PROTOCOL_VERSION
                                && matches!(&request.command, Command::Changed)
                            {
                                dirty.store(true, std::sync::atomic::Ordering::SeqCst);
                                generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            }
                            let mut status = status.read().unwrap().clone();
                            let progress = progress.snapshot();
                            status.active_transfers = progress.active;
                            status.transferred_bytes = progress.completed;
                            status.transfer_bytes = progress.total;
                            Response {
                                protocol: PROTOCOL_VERSION,
                                owner: id,
                                generation: generation.load(std::sync::atomic::Ordering::SeqCst),
                                status,
                                content: None,
                                error: (request.protocol != PROTOCOL_VERSION).then(|| {
                                    "incompatible Sync protocol; upgrade both desktop and CLI"
                                        .into()
                                }),
                            }
                        } else {
                            let (reply, response) = tokio::sync::oneshot::channel();
                            sender
                                .send((request.command, reply))
                                .await
                                .map_err(|_| Error::Invalid("Sync owner exited; retry".into()))?;
                            response
                                .await
                                .map_err(|_| Error::Invalid("Sync owner exited; retry".into()))?
                        };
                        tokio::time::timeout(
                            Duration::from_secs(5),
                            write_frame(&mut stream, &response),
                        )
                        .await
                        .map_err(|_| Error::Invalid("Sync response timed out".into()))??;
                        Ok(())
                    }
                    .await;
                    let _ = result;
                });
            }
        });
        runtime.status();
        Ok(Some(Self {
            _lock: lock,
            listener_task,
            requests,
            socket,
            runtime,
            id,
            generation,
            quiescing: false,
            dirty,
            changes,
        }))
    }

    pub async fn execute(&mut self, command: Command) -> Response {
        let result = match command {
            Command::Status => Ok(None),
            Command::Unlock { path } => self.runtime.unlock_from_file(&path).await.map(|()| None),
            Command::ConfigureCredentials {
                backend,
                unlock_file,
            } => self
                .runtime
                .configure_credentials(backend, unlock_file.as_deref())
                .await
                .map(|()| None),
            Command::Quiesce => {
                self.quiescing = true;
                self.runtime.shutdown().await.map(|()| None)
            }
            Command::Changed => {
                self.generation
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.runtime.changed();
                Ok(None)
            }
            Command::Action { action } => self.runtime.action(action).await,
            Command::Once { timeout_seconds } => self
                .runtime
                .once(Duration::from_secs(timeout_seconds.clamp(1, 3600)))
                .await
                .map(|()| None),
            Command::Resolve {
                entity,
                local,
                cloud,
                selected,
            } => self
                .runtime
                .resolve(entity, local, cloud, selected)
                .await
                .map(|()| None),
        };
        self.drain_changes();
        let (content, error) = match result {
            Ok(content) => (content, None),
            Err(error) => {
                self.runtime.report_error(error.clone());
                (None, Some(error))
            }
        };
        Response {
            protocol: PROTOCOL_VERSION,
            owner: self.id,
            generation: self.generation.load(std::sync::atomic::Ordering::SeqCst),
            status: self.runtime.status(),
            content,
            error,
        }
    }

    pub async fn step(&mut self) -> Result<()> {
        if self.dirty.swap(false, std::sync::atomic::Ordering::SeqCst) {
            self.runtime.changed();
        }
        tokio::select! {
            request = self.requests.recv() => {
                let Some((command, reply)) = request else { return Err(Error::Invalid("Sync listener exited".into())); };
                let response = self.execute(command).await;
                let _ = reply.send(response);
            },
            result = self.runtime.advance() => {
                self.drain_changes();
                if let Err(error) = result { self.runtime.report_error(error); }
                self.runtime.status();
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
        Ok(())
    }

    fn drain_changes(&mut self) {
        let mut changed = false;
        while self.changes.try_recv().is_ok() {
            changed = true;
        }
        if changed {
            self.generation
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.runtime.changed();
        }
    }

    pub fn is_quiescing(&self) -> bool {
        self.quiescing
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.runtime.shutdown().await.map_err(Error::Invalid)
    }
}

fn check_peer(stream: &UnixStream) -> Result<()> {
    if stream.peer_cred()?.uid() != rustix::process::geteuid().as_raw() {
        return Err(Error::Authentication);
    }
    Ok(())
}
async fn read_frame<T: serde::de::DeserializeOwned>(stream: &mut UnixStream) -> Result<T> {
    let length = stream.read_u32().await? as usize;
    if length > MAX_FRAME {
        return Err(Error::Invalid("Sync message exceeds size limit".into()));
    }
    let mut bytes = vec![0; length];
    stream.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes)
        .map_err(|_| Error::Invalid("invalid Sync protocol message".into()))
}
async fn write_frame(stream: &mut UnixStream, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| Error::Invalid("invalid Sync protocol message".into()))?;
    if bytes.len() > MAX_FRAME {
        return Err(Error::Invalid("Sync message exceeds size limit".into()));
    }
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    Ok(())
}

pub fn diagnostics(vault: &Path) -> serde_json::Value {
    let directory = staging_root().and_then(|root| directory(&root, vault));
    let Ok(directory) = directory else {
        return serde_json::json!({ "environment": "staging", "binding": "unavailable" });
    };
    let backend = fs::read(directory.join("credential-backend.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let connection = fs::read(directory.join("connection.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let ownership = match OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(directory.join("owner.lock"))
    {
        Ok(lock) => {
            if lock.try_lock().is_ok() {
                "idle"
            } else {
                "owned"
            }
        }
        Err(_) => "idle",
    };
    let label = directory
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| format!("io.loofah.sync.staging.{}", &name[..32]));
    let service = dirs::home_dir().zip(label).is_some_and(|(home, label)| {
        if cfg!(target_os = "macos") {
            home.join("Library/LaunchAgents")
                .join(format!("{label}.plist"))
                .exists()
        } else {
            home.join(".config/systemd/user")
                .join(format!("{label}.service"))
                .exists()
        }
    });
    serde_json::json!({ "environment": "staging", "directory": directory, "connection": connection, "credentials": backend, "credential_access": "not_probed", "ownership": ownership, "managed_service_installed": service, "recovery_metadata": directory.join("replica").exists() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn single_owner_stale_socket_and_protocol_validation() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let vault = root.join("vault");
        fs::create_dir(&vault).unwrap();
        let directory = root.join("state");
        let store = Arc::new(hypr_vault_write::SessionStore::new(vault));
        let acquire = || {
            Owner::acquire(
                &directory,
                store.clone(),
                Arc::new(credentials::Locked),
                crate::remote::AuthorizationClient::Cli,
            )
        };
        let mut first = acquire().await.unwrap().unwrap();
        assert!(acquire().await.unwrap().is_none());
        let initial = first.execute(Command::Status).await;
        let next = first.execute(Command::Status).await;
        assert_eq!(initial.generation, next.generation);
        let directory_copy = directory.clone();
        let client = tokio::spawn(async move {
            request(&directory_copy, Command::Status)
                .await
                .unwrap()
                .unwrap()
        });
        assert_eq!(client.await.unwrap().owner, initial.owner);
        let socket = first.socket.clone();
        let client = tokio::spawn(async move {
            let mut stream = UnixStream::connect(socket).await.unwrap();
            write_frame(
                &mut stream,
                &Request {
                    protocol: 999,
                    command: Command::Status,
                },
            )
            .await
            .unwrap();
            read_frame::<Response>(&mut stream).await.unwrap()
        });
        assert!(client.await.unwrap().error.unwrap().contains("upgrade"));
        first.shutdown().await.unwrap();
        drop(first);
        // Crash leftovers are removed only after acquiring the authoritative owner lock.
        let stale = UnixListener::bind(socket_path(&directory).unwrap()).unwrap();
        drop(stale);
        let second = acquire().await.unwrap().unwrap();
        assert_ne!(second.id, initial.owner);
    }

    #[test]
    fn canonical_vault_aliases_share_a_binding_and_missing_recovery_blocks_access() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let vault = root.join("vault");
        fs::create_dir(&vault).unwrap();
        std::os::unix::fs::symlink(&vault, root.join("alias")).unwrap();
        assert_eq!(
            directory(&root, &vault).unwrap(),
            directory(&root, &root.join("alias")).unwrap()
        );
        fs::write(vault.join(".loofah-sync-pending"), b"missing journal").unwrap();
        assert!(recover(&vault).is_err());
    }
}
