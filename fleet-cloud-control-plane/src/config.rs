use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RunnerGatewayConfig {
    pub listen_addr: SocketAddr,
    pub server_certificate: PathBuf,
    pub server_private_key: PathBuf,
    pub client_ca_certificate: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub database_url: String,
    pub api_key_pepper: Vec<u8>,
    pub runner_gateway: Option<RunnerGatewayConfig>,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let listen_addr = match std::env::var("FLEET_CLOUD_LISTEN_ADDR") {
            Ok(value) => value.parse()?,
            Err(std::env::VarError::NotPresent) => {
                SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 8090)
            }
            Err(error) => return Err(error.into()),
        };
        let database_url = std::env::var("DATABASE_URL")?;
        let api_key_pepper = std::env::var("FLEET_CLOUD_API_KEY_PEPPER")?.into_bytes();
        anyhow::ensure!(
            api_key_pepper.len() >= 32,
            "FLEET_CLOUD_API_KEY_PEPPER must be at least 32 bytes"
        );
        let runner_gateway = match (
            std::env::var_os("FLEET_CLOUD_RUNNER_TLS_CERT"),
            std::env::var_os("FLEET_CLOUD_RUNNER_TLS_KEY"),
            std::env::var_os("FLEET_CLOUD_RUNNER_CLIENT_CA"),
        ) {
            (None, None, None) => None,
            (Some(certificate), Some(key), Some(ca)) => Some(RunnerGatewayConfig {
                listen_addr: std::env::var("FLEET_CLOUD_RUNNER_LISTEN_ADDR")
                    .unwrap_or_else(|_| "0.0.0.0:8091".into())
                    .parse()?,
                server_certificate: certificate.into(),
                server_private_key: key.into(),
                client_ca_certificate: ca.into(),
            }),
            _ => anyhow::bail!("Runner TLS cert, key, and client CA must be configured together"),
        };
        Ok(Self {
            listen_addr,
            database_url,
            api_key_pepper,
            runner_gateway,
        })
    }
}
