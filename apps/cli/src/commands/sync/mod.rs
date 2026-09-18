use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use clap::{Subcommand, ValueEnum};
use uuid::Uuid;
use vault_sync::{
    credentials::{self, Backend},
    owner::{self, Command as Request, Owner, Response},
    remote::{AuthorizationClient, Environment},
    runtime::{SyncAction as Action, SyncEntity},
};
use zeroize::Zeroizing;

use crate::{Error, Result, output};

mod service;

#[derive(Debug, clap::Args)]
pub struct Args {
    /// Credential backend for a new connection; persisted and never silently changed
    #[arg(long, value_enum)]
    credentials: Option<CredentialBackend>,
    /// Owner-only file containing the local credential-store unlock secret
    #[arg(long, conflicts_with = "unlock_fd")]
    unlock_file: Option<PathBuf>,
    /// Inherited file descriptor containing the local unlock secret
    #[arg(long, conflicts_with = "unlock_file")]
    unlock_fd: Option<u32>,
    #[command(subcommand)]
    command: Command,
}
#[derive(Clone, Copy, Debug, ValueEnum)]
enum CredentialBackend {
    Keychain,
    SecretService,
    EncryptedFile,
}
impl From<CredentialBackend> for Backend {
    fn from(value: CredentialBackend) -> Self {
        match value {
            CredentialBackend::Keychain => Self::Keychain,
            CredentialBackend::SecretService => Self::SecretService,
            CredentialBackend::EncryptedFile => Self::EncryptedFile,
        }
    }
}
#[derive(Debug, Subcommand)]
enum Command {
    /// Display a browser authorization URL and user code; setup stays paused
    Connect,
    /// Disconnect this account, preserving local files and recovery material
    Disconnect {
        #[arg(long)]
        yes: bool,
    },
    Status,
    Recovery {
        #[command(subcommand)]
        command: Recovery,
    },
    Pair {
        #[command(subcommand)]
        command: Pair,
    },
    /// Reconcile and drain current work without changing the saved pause preference
    Once {
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout: u64,
    },
    /// Run foreground synchronization under a terminal or external supervisor
    Watch,
    /// Enable a managed per-user background service and resume sync
    Start,
    /// Pause synchronization and disable the managed background service
    Stop,
    Pause,
    Resume,
    Devices {
        #[command(subcommand)]
        command: Devices,
    },
    Conflicts {
        #[command(subcommand)]
        command: Conflicts,
    },
    History {
        #[command(subcommand)]
        command: History,
    },
    /// Reconcile a changed recovery generation, preserving previous state
    Reconcile {
        #[arg(long)]
        yes: bool,
    },
}
#[derive(Debug, Subcommand)]
enum Recovery {
    Create { path: PathBuf },
    Import { path: PathBuf },
}
#[derive(Debug, Subcommand)]
enum Pair {
    Receive,
    Approve { code: String },
}
#[derive(Debug, Subcommand)]
enum Devices {
    List,
    Rename {
        id: Uuid,
        name: String,
    },
    Revoke {
        id: Uuid,
        #[arg(long)]
        yes: bool,
    },
}
#[derive(Debug, Subcommand)]
enum Conflicts {
    List,
    Show {
        entity: String,
    },
    /// Resolve only the two reviewed revisions; stale choices are rejected
    Resolve {
        entity: String,
        #[arg(long)]
        local: Uuid,
        #[arg(long)]
        cloud: Uuid,
        #[arg(long)]
        retain: Uuid,
        #[arg(long)]
        yes: bool,
    },
}
#[derive(Debug, Subcommand)]
enum History {
    List {
        entity: String,
        #[arg(long, requires = "before_revision")]
        before_time: Option<u64>,
        #[arg(long, requires = "before_time")]
        before_revision: Option<Uuid>,
    },
    Show {
        entity: String,
        revision: Uuid,
    },
    Export {
        entity: String,
        revision: Uuid,
        destination: PathBuf,
    },
    Restore {
        entity: String,
        revision: Uuid,
        #[arg(long)]
        yes: bool,
    },
    Purge {
        revision: Uuid,
        #[arg(long)]
        yes: bool,
    },
}

fn sync_error(error: impl std::fmt::Display) -> Error {
    let reason = error.to_string();
    let code = if reason.contains("locked") || reason.contains("unlock") {
        "sync_needs_unlock"
    } else if reason.contains("protocol") {
        "sync_protocol_incompatible"
    } else if reason.contains("quota") {
        "sync_quota"
    } else if reason.contains("timed out") {
        "sync_timeout"
    } else if reason.contains("conflict") || reason.contains("Conflict") {
        "sync_conflict"
    } else if reason.contains("recording") {
        "sync_recording_deferred"
    } else if reason.contains("recovery") {
        "sync_recovery_required"
    } else {
        "sync_failed"
    };
    Error::Sync { code, reason }
}
fn entity(value: &str) -> Result<SyncEntity> {
    Ok(match value {
        "people" => SyncEntity::People,
        "tags" => SyncEntity::Tags,
        "tasks" => SyncEntity::Tasks,
        _ => {
            let id = value
                .strip_prefix("session:")
                .ok_or_else(|| sync_error("entity must be session:ID, people, tags, or tasks"))?;
            let entity = vault_sync::scope::Entity::Session(id.into());
            entity.root().map_err(sync_error)?;
            SyncEntity::Session { id: id.into() }
        }
    })
}
fn confirm(yes: bool, action: &str) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(sync_error(format!(
            "{action} requires --yes in noninteractive use"
        )));
    }
    eprint!("{action}? Type yes to continue: ");
    std::io::stderr().flush().map_err(sync_error)?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .map_err(sync_error)?;
    if answer.trim() != "yes" {
        return Err(sync_error("cancelled; no changes made"));
    }
    Ok(())
}
fn initialize(path: &Path) -> Result<PathBuf> {
    if !path.exists() {
        std::fs::create_dir_all(path).map_err(sync_error)?;
    }
    let path = path.canonicalize().map_err(sync_error)?;
    if !path.join("sessions").is_dir() && !path.join("config.json").is_file() {
        if std::fs::read_dir(&path)
            .map_err(sync_error)?
            .next()
            .is_some()
        {
            return Err(sync_error(
                "destination is not an empty directory or an existing Loofah vault",
            ));
        }
        std::fs::create_dir(path.join("sessions")).map_err(sync_error)?;
    }
    owner::recover(&path).map_err(sync_error)?;
    Ok(path)
}
fn unlock(args: &Args) -> Result<Option<Zeroizing<Vec<u8>>>> {
    if let Some(path) = &args.unlock_file {
        return credentials::file::read_secret(path)
            .map(Some)
            .map_err(sync_error);
    }
    if let Some(fd) = args.unlock_fd {
        use std::io::Read;
        // Opening the inherited descriptor does not take ownership of the caller's descriptor.
        let mut bytes = Zeroizing::new(Vec::new());
        std::fs::File::open(format!("/dev/fd/{fd}"))
            .map_err(sync_error)?
            .take(4097)
            .read_to_end(&mut bytes)
            .map_err(sync_error)?;
        if bytes.is_empty() || bytes.len() > 4096 {
            return Err(sync_error("unlock secret must contain 1–4096 bytes"));
        }
        return Ok(Some(bytes));
    }
    Ok(None)
}

pub async fn run(args: &crate::Args, sync: &Args) -> Result<u8> {
    if !cfg!(feature = "staging") {
        return Err(sync_error(
            "CLI Sync is invitation-only staging; use a loof-staging build",
        ));
    }
    let vault = if matches!(&sync.command, Command::Connect) {
        initialize(
            args.vault_path
                .as_deref()
                .ok_or_else(|| sync_error("connect requires an explicit --vault-path"))?,
        )?
    } else {
        crate::vault::open(args)?
    };
    let directory = owner::directory(&owner::staging_root().map_err(sync_error)?, &vault)
        .map_err(sync_error)?;
    let mut session = Session {
        directory: directory.clone(),
        owner: None,
    };
    if owner::request(&directory, Request::Status)
        .await
        .map_err(sync_error)?
        .is_none()
    {
        let store: Arc<dyn credentials::CredentialStore> = if !directory
            .join("connection.json")
            .exists()
            && !matches!(&sync.command, Command::Connect)
        {
            Arc::new(credentials::Locked)
        } else {
            let mut secret = unlock(sync)?;
            let mut store = credentials::open(
                &directory,
                Environment::Staging,
                sync.credentials.map(Into::into),
                secret.as_deref().map(|bytes| bytes.as_slice()),
            );
            if matches!(&store, Err(vault_sync::Error::NeedsUnlock))
                && secret.is_none()
                && std::io::stdin().is_terminal()
            {
                eprintln!(
                    "Unlock the local encrypted credential store. This is separate from your account password and recovery kit."
                );
                secret = Some(Zeroizing::new(
                    rpassword::prompt_password("Local unlock secret: ")
                        .map_err(sync_error)?
                        .into_bytes(),
                ));
                store = credentials::open(
                    &directory,
                    Environment::Staging,
                    sync.credentials.map(Into::into),
                    secret.as_deref().map(|bytes| bytes.as_slice()),
                );
            }
            match store {
                Ok(store) => store,
                Err(vault_sync::Error::NeedsUnlock)
                    if matches!(
                        &sync.command,
                        Command::Watch | Command::Status | Command::Pause | Command::Stop
                    ) =>
                {
                    Arc::new(credentials::Locked)
                }
                Err(error) => return Err(sync_error(error)),
            }
        };
        session.owner = Owner::acquire(
            &directory,
            Arc::new(hypr_vault_write::SessionStore::new(vault.clone())),
            store,
            AuthorizationClient::Cli,
        )
        .await
        .map_err(sync_error)?;
    }
    if session.owner.is_none() {
        if sync.unlock_fd.is_some() {
            return Err(sync_error(
                "an attached owner needs --unlock-file; inherited descriptors are supported by the foreground owner",
            ));
        }
        if sync.credentials.is_some()
            || sync.unlock_file.is_some()
            || matches!(&sync.command, Command::Connect)
        {
            session
                .call(Request::ConfigureCredentials {
                    backend: sync.credentials.map(Into::into),
                    unlock_file: sync.unlock_file.as_deref().map(absolute).transpose()?,
                })
                .await?;
        }
    }
    let result = run_command(&mut session, &vault, args.json, sync).await;
    if let Some(owner) = &mut session.owner {
        owner.shutdown().await.map_err(sync_error)?;
    }
    result
}

struct Session {
    directory: PathBuf,
    owner: Option<Owner>,
}
impl Session {
    async fn call(&mut self, command: Request) -> Result<Response> {
        let response = if let Some(owner) = &mut self.owner {
            owner.execute(command).await
        } else {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            loop {
                if let Some(response) = owner::request(&self.directory, command.clone())
                    .await
                    .map_err(sync_error)?
                {
                    break response;
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(sync_error("Sync owner exited; retry the command"));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        };
        if let Some(error) = &response.error {
            return Err(sync_error(error));
        }
        Ok(response)
    }
    async fn action(&mut self, action: Action) -> Result<Response> {
        self.call(Request::Action { action }).await
    }
    async fn step(&mut self) -> Result<()> {
        if let Some(owner) = &mut self.owner {
            owner.step().await.map_err(sync_error)
        } else {
            tokio::time::sleep(Duration::from_millis(250)).await;
            Ok(())
        }
    }
}
fn emit(response: &Response, json: bool) -> Result<()> {
    if json {
        output::emit(&output::json("sync", response, None)?);
    } else {
        output::emit(&serde_json::to_string_pretty(response).map_err(sync_error)?);
    }
    Ok(())
}
async fn run_command(session: &mut Session, vault: &Path, json: bool, args: &Args) -> Result<u8> {
    let response = match &args.command {
        Command::Connect => session.action(Action::ConnectCli).await?,
        Command::Disconnect { yes } => {
            confirm(*yes, "Disconnect this device's account")?;
            session.action(Action::Disconnect).await?
        }
        Command::Status
        | Command::Conflicts {
            command: Conflicts::List,
        } => session.call(Request::Status).await?,
        Command::Recovery {
            command: Recovery::Create { path },
        } => {
            session
                .action(Action::SaveRecoveryKit {
                    path: absolute(path)?,
                })
                .await?
        }
        Command::Recovery {
            command: Recovery::Import { path },
        } => {
            session
                .action(Action::ImportRecoveryKit {
                    path: absolute(path)?,
                })
                .await?
        }
        Command::Pair {
            command: Pair::Receive,
        } => session.action(Action::PairNewMac).await?,
        Command::Pair {
            command: Pair::Approve { code },
        } => {
            session
                .action(Action::ApproveMac { code: code.clone() })
                .await?
        }
        Command::Pause | Command::Stop => {
            let result = session.action(Action::Pause).await?;
            if matches!(&args.command, Command::Stop) {
                service::stop(&session.directory)?;
            }
            result
        }
        Command::Resume => session.action(Action::Resume).await?,
        Command::Start => {
            let result = session.action(Action::Resume).await?;
            if let Err(error) =
                service::start(vault, &session.directory, args.unlock_file.as_deref())
            {
                let _ = session.action(Action::Pause).await;
                return Err(error);
            }
            result
        }
        Command::Once { timeout } => {
            session
                .call(Request::Once {
                    timeout_seconds: *timeout,
                })
                .await?
        }
        Command::Reconcile { yes } => {
            confirm(
                *yes,
                "Reconcile this recovery generation and archive previous Sync state",
            )?;
            session.action(Action::ReconcileRecovery).await?
        }
        Command::Devices { command } => match command {
            Devices::List => session.action(Action::RefreshDevices).await?,
            Devices::Rename { id, name } => {
                session
                    .action(Action::RenameDevice {
                        id: id.to_string(),
                        name: name.clone(),
                    })
                    .await?
            }
            Devices::Revoke { id, yes } => {
                confirm(*yes, &format!("Revoke device {id}"))?;
                session
                    .action(Action::RevokeDevice { id: id.to_string() })
                    .await?
            }
        },
        Command::History { command } => match command {
            History::List {
                entity: value,
                before_time,
                before_revision,
            } => {
                session
                    .action(Action::History {
                        entity: entity(value)?,
                        before: before_time
                            .zip(*before_revision)
                            .map(|(time, revision)| (time, revision.to_string())),
                    })
                    .await?
            }
            History::Show {
                entity: value,
                revision,
            } => {
                session
                    .action(Action::Preview {
                        entity: entity(value)?,
                        revision: revision.to_string(),
                    })
                    .await?
            }
            History::Export {
                entity: value,
                revision,
                destination,
            } => {
                session
                    .action(Action::Export {
                        entity: entity(value)?,
                        revision: revision.to_string(),
                        destination: absolute(destination)?,
                    })
                    .await?
            }
            History::Restore {
                entity: value,
                revision,
                yes,
            } => {
                confirm(
                    *yes,
                    "Restore this version as a new head, preserving unsynced work",
                )?;
                session
                    .action(Action::Restore {
                        entity: entity(value)?,
                        revision: revision.to_string(),
                    })
                    .await?
            }
            History::Purge { revision, yes } => {
                confirm(*yes, "Logically purge this version from history")?;
                session
                    .action(Action::PurgeVersion {
                        revision: revision.to_string(),
                    })
                    .await?
            }
        },
        Command::Conflicts {
            command:
                Conflicts::Resolve {
                    entity: value,
                    local,
                    cloud,
                    retain,
                    yes,
                },
        } => {
            confirm(*yes, "Resolve these reviewed revisions")?;
            session
                .call(Request::Resolve {
                    entity: entity(value)?,
                    local: *local,
                    cloud: *cloud,
                    selected: *retain,
                })
                .await?
        }
        Command::Conflicts {
            command: Conflicts::Show { entity: value },
        } => {
            let expected = entity(value)?;
            let response = session.call(Request::Status).await?;
            let conflict = response
                .status
                .conflicts
                .iter()
                .find(|conflict| conflict.entity == expected)
                .ok_or_else(|| sync_error("no conflict for this entity"))?;
            let mut revisions = Vec::new();
            for revision in [&conflict.local, &conflict.cloud] {
                let version = session
                    .action(Action::Preview {
                        entity: expected.clone(),
                        revision: revision.clone(),
                    })
                    .await?;
                revisions
                    .push(serde_json::json!({ "revision": revision, "content": version.content }));
            }
            output::emit(&output::json("sync", &revisions, None)?);
            return Ok(0);
        }
        Command::Watch => {
            let mut termination =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .map_err(sync_error)?;
            let mut previous = None;
            loop {
                tokio::select! { result = session.step() => { result?; }, _ = tokio::signal::ctrl_c() => break, _ = termination.recv() => break }
                let response = session.call(Request::Status).await?;
                if previous.as_ref() != Some(&response.status) {
                    previous = Some(response.status.clone());
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({ "schema_version": 1, "event": "status", "data": response })
                        );
                    } else {
                        eprintln!(
                            "Sync: {}{}",
                            response.status.phase,
                            response
                                .status
                                .error
                                .as_ref()
                                .map(|error| format!(" — {error}"))
                                .unwrap_or_default()
                        );
                    }
                }
            }
            return Ok(0);
        }
    };
    if let Some(url) = &response.status.browser_url {
        eprintln!(
            "Authorize from any browser: {url}\nUser code: {}",
            response.status.user_code.as_deref().unwrap_or("")
        );
    }
    if let Some(code) = &response.status.pairing_code {
        eprintln!("Approve on your trusted device: loof-staging sync pair approve {code}");
    }
    if matches!(&args.command, Command::Connect | Command::Pair { .. }) {
        loop {
            tokio::select! {
                result = session.step() => { result?; },
                _ = tokio::signal::ctrl_c() => { session.action(Action::Cancel).await?; return Err(sync_error("cancelled; run connect or pair again to retry")); }
            }
            let current = session.call(Request::Status).await?;
            if !matches!(current.status.phase.as_str(), "authorizing" | "pairing") {
                if let Some(error) = &current.status.error {
                    return Err(sync_error(error));
                }
                emit(&current, json)?;
                return Ok(0);
            }
        }
    }
    emit(&response, json)?;
    Ok(0)
}

pub async fn notify_committed(vault: &Path) {
    let Ok(root) = owner::staging_root() else {
        return;
    };
    let Ok(directory) = owner::directory(&root, vault) else {
        return;
    };
    let _ = tokio::time::timeout(
        Duration::from_secs(2),
        owner::request(&directory, Request::Changed),
    )
    .await;
}

fn absolute(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        Ok(std::env::current_dir().map_err(sync_error)?.join(path))
    }
}
