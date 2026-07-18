use anyhow::Context;
use fleet_cloud_control_plane::{app, config, db};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fleet_cloud_control_plane=info".into()),
        )
        .init();

    let config = config::Config::from_env()?;
    let pool = db::connect(&config.database_url)
        .await
        .context("connect PostgreSQL")?;
    sqlx::migrate!()
        .run(&pool)
        .await
        .context("run database migrations")?;
    let state = app::AppState::new(pool, config.api_key_pepper);
    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("bind {}", config.listen_addr))?;
    tracing::info!(address = %config.listen_addr, "Fleet Cloud control plane listening");
    axum::serve(listener, app::router(state))
        .await
        .context("serve API")
}
