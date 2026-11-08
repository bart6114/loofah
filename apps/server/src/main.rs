use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "loofah-server", version = env!("LOOFAH_VERSION"), about = "Loofah headless backend (experimental)", subcommand_negates_reqs = true, args_conflicts_with_subcommands = true)]
struct Args {
    #[arg(long, required = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Maintenance>,
}

#[derive(Subcommand)]
enum Maintenance {
    /// Copy a stopped Local vault into a fresh catalogued target in new server storage.
    AdoptVault {
        #[arg(long)]
        storage_root: PathBuf,
        #[arg(long)]
        vault_id: String,
        #[arg(long)]
        source_vault: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Snapshot stopped storage, keeping provider credentials in a separate private bundle.
    Backup {
        #[arg(long)]
        storage_root: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        secrets_output: PathBuf,
    },
    /// Verify file digests, catalog schema and the private bundle pairing without starting a service.
    VerifyBackup {
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long)]
        secrets: Option<PathBuf>,
    },
    /// Restore verified bundles to a new, nonexistent storage directory.
    Restore {
        #[arg(long)]
        snapshot: PathBuf,
        #[arg(long)]
        secrets: Option<PathBuf>,
        #[arg(long)]
        storage_root: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    if let Some(command) = args.command {
        use loofah_server::maintenance;
        let report = match command {
            Maintenance::AdoptVault {
                storage_root,
                vault_id,
                source_vault,
                output,
            } => {
                let report =
                    maintenance::adopt_vault(&storage_root, &vault_id, &source_vault, &output)
                        .await?;
                println!("{}", serde_json::to_string(&report)?);
                return Ok(());
            }
            Maintenance::Backup {
                storage_root,
                output,
                secrets_output,
            } => maintenance::backup(&storage_root, &output, &secrets_output).await?,
            Maintenance::VerifyBackup { snapshot, secrets } => {
                maintenance::verify(&snapshot, secrets.as_deref()).await?
            }
            Maintenance::Restore {
                snapshot,
                secrets,
                storage_root,
            } => maintenance::restore(&snapshot, secrets.as_deref(), &storage_root).await?,
        };
        println!("{}", serde_json::to_string(&report)?);
        return Ok(());
    }
    let config: loofah_server::config::Config = serde_json::from_slice(&std::fs::read(
        args.config
            .expect("Clap requires --config without a maintenance command"),
    )?)?;
    let service = loofah_server::service::Service::open(config).await?;
    let listener = tokio::net::TcpListener::bind(service.config.listen).await?;
    tracing::info!(address = %listener.local_addr()?, "Loofah server listening behind HTTPS termination");
    let stop = service.stop.clone();
    let shutdown = service.clone();
    let server = axum::serve(listener, loofah_server::routes::router(service.clone()))
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("SIGTERM handler");
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            stop.cancel();
            // Drain actual storage work while Axum finishes network connections.
            tokio::spawn(async move {
                if let Err(error) = shutdown.shutdown().await {
                    tracing::error!(%error, "shutdown deadline or drain error; owned drain continues");
                }
            });
        });
    let server_result = server.await;
    if let Err(error) = service.shutdown().await {
        tracing::error!(%error, "shutdown deadline or drain error; waiting for actual owned operations before exit");
    }
    let drained = service.wait_for_shutdown().await;
    server_result?;
    drained?;
    Ok(())
}
