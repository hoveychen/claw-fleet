use fleet_cloud_runner::{
    config, identity, journal::CommandJournal, outbox::EventOutbox, transport,
};
use fleet_cloud_wire::runner::{ClientHello, RunnerCapability};

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "fleet_cloud_runner=info".into()),
        )
        .init();
    let config = config::Config::from_env()?;
    std::fs::create_dir_all(&config.state_directory)?;
    let tls = identity::client_config(
        &std::fs::read(&config.ca_certificate)?,
        &std::fs::read(&config.client_certificate)?,
        &std::fs::read(&config.client_private_key)?,
    )?;
    let mut journal = CommandJournal::open(&config.state_directory.join("commands.sqlite"))?;
    let outbox = EventOutbox::open(&config.state_directory.join("outbox.sqlite"))?;
    let hello = ClientHello {
        protocol_version: fleet_cloud_wire::RUNNER_PROTOCOL_VERSION,
        runner_id: config.runner_id,
        build_version: env!("CARGO_PKG_VERSION").into(),
        platform: std::env::consts::OS.into(),
        architecture: std::env::consts::ARCH.into(),
        max_concurrency: config.max_concurrency,
        capabilities: config
            .capabilities
            .into_iter()
            .map(|name| RunnerCapability { name, version: 1 })
            .collect(),
        last_cloud_cursor: None,
        outbox_first_sequence: None,
        outbox_last_sequence: None,
    };
    tracing::info!(cloud_url=%config.cloud_url,"Fleet Cloud Runner starting outbound mTLS transport");
    transport::run_forever(&config.cloud_url, tls, hello, &mut journal, &outbox).await
}
