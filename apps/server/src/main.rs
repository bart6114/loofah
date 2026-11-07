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
        });
    server.await?;
    service.shutdown().await?;
    Ok(())
}
