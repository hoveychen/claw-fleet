use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use anyhow::Context;
use fleet_cloud_control_plane::github_adapter::{
    run_status_sync, webhook_router, FleetApiClient, GithubApiClient, GithubWebhookState,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fleet_cloud_control_plane=info".into()),
        )
        .init();
    let listen_addr = std::env::var("FLEET_GITHUB_LISTEN_ADDR")
        .map(|value| value.parse())
        .unwrap_or_else(|_| Ok(SocketAddr::new(IpAddr::V4(Ipv4Addr::UNSPECIFIED), 8080)))?;
    let fleet_base_url = required("FLEET_CLOUD_API_URL")?;
    let project_id = required("FLEET_CLOUD_PROJECT_ID")?;
    let fleet_api_key = required("FLEET_CLOUD_PROJECT_API_KEY")?;
    let repository = required("FLEET_GITHUB_REPOSITORY")?;
    let webhook_secret = required("FLEET_GITHUB_WEBHOOK_SECRET")?.into_bytes();
    let fleet = FleetApiClient::new(fleet_base_url, project_id.clone(), fleet_api_key)
        .map_err(anyhow::Error::msg)?;
    let github = GithubApiClient::new(
        std::env::var("FLEET_GITHUB_API_URL").unwrap_or_else(|_| "https://api.github.com".into()),
        required("FLEET_GITHUB_APP_ID")?,
        required("FLEET_GITHUB_INSTALLATION_ID")?,
        &required("FLEET_GITHUB_PRIVATE_KEY")?,
    )
    .map_err(anyhow::Error::msg)?;
    let state = GithubWebhookState {
        webhook_secret: Arc::new(webhook_secret),
        repository: Arc::new(repository),
        project_id: Arc::new(project_id),
        fleet: fleet.clone(),
    };
    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .with_context(|| format!("bind GitHub adapter {listen_addr}"))?;
    tracing::info!(address=%listen_addr, "Fleet Cloud GitHub adapter listening");
    let sync = tokio::spawn(run_status_sync(
        fleet,
        github,
        state.repository.as_ref().clone(),
        required("FLEET_CLOUD_CONSOLE_URL")?,
        std::time::Duration::from_secs(5),
    ));
    tokio::select! {
        result = axum::serve(listener, webhook_router(state)) => result?,
        result = sync => result.context("GitHub status sync task stopped")?,
    }
    Ok(())
}

fn required(name: &str) -> anyhow::Result<String> {
    std::env::var(name).with_context(|| format!("{name} is required"))
}
