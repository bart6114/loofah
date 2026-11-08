use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "loofah-server", version = env!("LOOFAH_VERSION"), about = "Loofah headless backend (experimental)")]
struct Args {
    #[arg(long)]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    let args = Args::parse();
    let config: loofah_server::config::Config =
        serde_json::from_slice(&std::fs::read(args.config)?)?;
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
