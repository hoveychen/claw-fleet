mod app;
mod config;
mod db;
mod error;

use anyhow::Context;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fleet_cloud_control_plane=info".into()),
        )
        .init();

    let config = config::Config::from_env()?;
    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("bind {}", config.listen_addr))?;
    tracing::info!(address = %config.listen_addr, "Fleet Cloud control plane listening");
    axum::serve(listener, app::router())
        .await
        .context("serve API")
}
