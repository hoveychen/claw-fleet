use anyhow::Context;
use fleet_cloud_control_plane::{app, config, db, runner_gateway};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::io::BufReader;
use std::path::Path;

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
    let retention_pool = pool.clone();
    let retention_owner = format!("control-plane-{}", uuid::Uuid::now_v7().simple());
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(24 * 60 * 60));
        loop {
            interval.tick().await;
            if let Err(error) = fleet_cloud_control_plane::services::artifacts::run_retention(
                &retention_pool,
                &retention_owner,
            )
            .await
            {
                tracing::warn!(%error, "Fleet Cloud retention job failed");
            }
        }
    });
    let worker_pool = pool.clone();
    let worker_pepper = config.api_key_pepper.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
        loop {
            interval.tick().await;
            if let Err(error) = fleet_cloud_control_plane::services::webhooks::process_due_batch(
                &worker_pool,
                &worker_pepper,
                50,
            )
            .await
            {
                tracing::warn!(%error, "Fleet Cloud webhook delivery batch failed");
            }
        }
    });
    let usage_pool = pool.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(5 * 60));
        loop {
            interval.tick().await;
            let projects = sqlx::query_as::<_, (String, String)>(
                "SELECT organization_id,id FROM projects ORDER BY id",
            )
            .fetch_all(&usage_pool)
            .await;
            match projects {
                Ok(projects) => {
                    let now = chrono::Utc::now();
                    for (organization_id, project_id) in projects {
                        for hour in [now, now - chrono::Duration::hours(1)] {
                            if let Err(error) = fleet_cloud_control_plane::services::governance::aggregate_usage_hour(
                                &usage_pool,
                                &organization_id,
                                &project_id,
                                hour,
                            )
                            .await
                            {
                                tracing::warn!(%error, %project_id, "Fleet Cloud usage aggregation failed");
                            }
                        }
                    }
                }
                Err(error) => tracing::warn!(%error, "Fleet Cloud usage project scan failed"),
            }
        }
    });
    let mut state = app::AppState::new(pool.clone(), config.api_key_pepper);
    if let Some(gateway) = config.runner_gateway.as_ref() {
        let ca_pem =
            std::fs::read_to_string(&gateway.client_ca_certificate).with_context(|| {
                format!(
                    "read Runner client CA {}",
                    gateway.client_ca_certificate.display()
                )
            })?;
        let ca_key_pem =
            std::fs::read_to_string(&gateway.client_ca_private_key).with_context(|| {
                format!(
                    "read Runner client CA key {}",
                    gateway.client_ca_private_key.display()
                )
            })?;
        state = state.with_runner_identity_issuer(
            runner_gateway::identity::RunnerIdentityIssuer::from_pem(&ca_pem, &ca_key_pem)?,
        );
    }
    let listener = tokio::net::TcpListener::bind(config.listen_addr)
        .await
        .with_context(|| format!("bind {}", config.listen_addr))?;
    tracing::info!(address = %config.listen_addr, "Fleet Cloud control plane listening");
    if let Some(gateway) = config.runner_gateway {
        let (server_chain, server_key) =
            load_identity(&gateway.server_certificate, &gateway.server_private_key)?;
        let client_ca = load_certificates(&gateway.client_ca_certificate)?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Runner client CA file is empty"))?;
        let acceptor =
            runner_gateway::connection::tls_acceptor(server_chain, server_key, client_ca)?;
        let runner_listener = tokio::net::TcpListener::bind(gateway.listen_addr)
            .await
            .with_context(|| format!("bind Runner gateway {}", gateway.listen_addr))?;
        tracing::info!(address=%gateway.listen_addr,"Fleet Cloud mTLS Runner gateway listening");
        let runner_pool = pool.clone();
        let gateway_task = tokio::spawn(runner_gateway::connection::serve(
            runner_listener,
            acceptor,
            runner_pool,
        ));
        let stale_pool = pool.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(15));
            loop {
                interval.tick().await;
                let _ = runner_gateway::registry::mark_stale_offline(&stale_pool).await;
            }
        });
        tokio::select! {
            result=axum::serve(listener,app::router(state))=>result.context("serve API"),
            result=gateway_task=>result.context("join Runner gateway")?.context("serve Runner gateway"),
        }
    } else {
        tracing::warn!("Runner gateway disabled because TLS paths are not configured");
        axum::serve(listener, app::router(state))
            .await
            .context("serve API")
    }
}

fn load_certificates(path: &Path) -> anyhow::Result<Vec<CertificateDer<'static>>> {
    let bytes =
        std::fs::read(path).with_context(|| format!("read certificate {}", path.display()))?;
    Ok(rustls_pemfile::certs(&mut BufReader::new(bytes.as_slice())).collect::<Result<_, _>>()?)
}

fn load_identity(
    certificate: &Path,
    key: &Path,
) -> anyhow::Result<(Vec<CertificateDer<'static>>, PrivateKeyDer<'static>)> {
    let chain = load_certificates(certificate)?;
    anyhow::ensure!(
        !chain.is_empty(),
        "Runner gateway server certificate is empty"
    );
    let bytes =
        std::fs::read(key).with_context(|| format!("read private key {}", key.display()))?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(bytes.as_slice()))?
        .ok_or_else(|| anyhow::anyhow!("Runner gateway private key is empty"))?;
    Ok((chain, key))
}
