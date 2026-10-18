use std::path::PathBuf;

use clap::{Parser, Subcommand};
use loofah_server::{Config, Server};

#[derive(Parser)]
#[command(name = "loofah-server")]
struct Args {
    #[arg(long, env = "LOOFAH_SERVER_CONFIG")]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Serve {
        #[arg(long, default_value = "127.0.0.1:8947")]
        listen: std::net::SocketAddr,
    },
    DisableUser {
        user_id: String,
        #[arg(long)]
        enable: bool,
    },
    Import {
        source: PathBuf,
        vault_id: String,
    },
    Recover,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    let config: Config = serde_json::from_slice(&std::fs::read(&args.config)?)?;
    let public = url::Url::parse(&config.public_url)?;
    if public.scheme() != "https" && !matches!(public.host_str(), Some("127.0.0.1" | "localhost")) {
        return Err("public_url must use HTTPS outside loopback development".into());
    }
    match args.command.unwrap_or(Command::Serve {
        listen: "127.0.0.1:8947".parse()?,
    }) {
        Command::Serve { listen } => {
            let server = Server::open(config)?;
            let listener = tokio::net::TcpListener::bind(listen).await?;
            tracing::info!(address = %listener.local_addr()?, "vault service listening");
            let stopping = server.stopping.clone();
            axum::serve(listener, loofah_server::http::router(server))
                .with_graceful_shutdown(async move {
                    let mut terminate =
                        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                            .expect("register SIGTERM handler");
                    tokio::select! { _=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{} }
                    stopping.send_replace(true);
                })
                .await?;
        }
        Command::DisableUser { user_id, enable } => {
            let server = Server::open(config)?;
            let mut registry = server.registry.write().await;
            let mut next = registry.clone();
            next.users
                .get_mut(&user_id)
                .ok_or("user not found")?
                .disabled = !enable;
            next.sessions
                .retain(|_, session| session.user_id != user_id);
            server.save_registry(&next).await?;
            *registry = next;
        }
        Command::Recover => {
            let server = Server::open(config)?;
            let ids = server
                .registry
                .read()
                .await
                .vaults
                .keys()
                .cloned()
                .collect::<Vec<_>>();
            for id in ids {
                server.runtime(&id).await?;
            }
        }
        Command::Import { source, vault_id } => {
            hypr_vault_access::validate_id(&vault_id)?;
            let server = Server::open(config)?;
            if !server.registry.read().await.vaults.contains_key(&vault_id) {
                return Err("create the destination vault before importing".into());
            }
            let destination = server.config.data_dir.join("vaults").join(&vault_id);
            if destination.read_dir()?.next().is_some() {
                return Err("destination vault must be empty".into());
            }
            let source = source.canonicalize()?;
            let data = server.config.data_dir.canonicalize()?;
            if data.starts_with(&source) || source.starts_with(&data) {
                return Err("import source must be outside the service data directory".into());
            }
            let temporary = tempfile::tempdir_in(&server.config.data_dir)?;
            copy(&source, temporary.path())?;
            let store = hypr_vault_write::SessionStore::new(temporary.path().to_path_buf());
            let layout = store.normalize_startup_layout().await?;
            if !layout.migration.failed.is_empty() || !layout.migration.skipped.is_empty() {
                return Err("import migration has unresolved issues".into());
            }
            let report = store.rebuild_index_from_startup_layout(layout).await?;
            if !report.errors.is_empty() {
                return Err("import contains invalid sessions".into());
            }
            // Device settings are not shared vault configuration.
            let _ = std::fs::remove_file(temporary.path().join("config.json"));
            std::fs::remove_dir(&destination)?;
            std::fs::rename(temporary.path(), &destination)?;
            std::fs::File::open(destination.parent().unwrap())?.sync_all()?;
        }
    }
    Ok(())
}

fn copy(source: &std::path::Path, destination: &std::path::Path) -> std::io::Result<()> {
    for entry in source.read_dir()? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if kind.is_symlink() {
            return Err(std::io::Error::other("imports cannot contain symlinks"));
        }
        if kind.is_dir() {
            std::fs::create_dir(&target)?;
            copy(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
            std::fs::File::open(target)?.sync_all()?;
        }
    }
    std::fs::File::open(destination)?.sync_all()
}
