use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub cloud_url: String,
    pub runner_id: String,
    pub ca_certificate: PathBuf,
    pub client_certificate: PathBuf,
    pub client_private_key: PathBuf,
    pub state_directory: PathBuf,
    pub max_concurrency: u16,
    pub capabilities: Vec<String>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let max_concurrency = std::env::var("FLEET_RUNNER_MAX_CONCURRENCY")
            .unwrap_or_else(|_| "1".into())
            .parse()?;
        anyhow::ensure!(
            max_concurrency > 0,
            "FLEET_RUNNER_MAX_CONCURRENCY must be positive"
        );
        Ok(Self {
            cloud_url: std::env::var("FLEET_CLOUD_RUNNER_URL")?,
            runner_id: std::env::var("FLEET_RUNNER_ID")?,
            ca_certificate: std::env::var_os("FLEET_RUNNER_CA_CERT")
                .ok_or_else(|| anyhow::anyhow!("FLEET_RUNNER_CA_CERT is required"))?
                .into(),
            client_certificate: std::env::var_os("FLEET_RUNNER_TLS_CERT")
                .ok_or_else(|| anyhow::anyhow!("FLEET_RUNNER_TLS_CERT is required"))?
                .into(),
            client_private_key: std::env::var_os("FLEET_RUNNER_TLS_KEY")
                .ok_or_else(|| anyhow::anyhow!("FLEET_RUNNER_TLS_KEY is required"))?
                .into(),
            state_directory: std::env::var_os("FLEET_RUNNER_STATE_DIR")
                .map(Into::into)
                .unwrap_or_else(|| PathBuf::from(".fleet-runner")),
            max_concurrency,
            capabilities: std::env::var("FLEET_RUNNER_CAPABILITIES")
                .unwrap_or_else(|_| "claude_code,codex".into())
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .collect(),
        })
    }
}
