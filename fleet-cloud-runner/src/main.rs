mod config;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fleet_cloud_runner=info".into()),
        )
        .init();

    let config = config::Config::from_env()?;
    tracing::info!(cloud_url = %config.cloud_url, "Fleet Cloud Runner scaffold ready");
    Ok(())
}
