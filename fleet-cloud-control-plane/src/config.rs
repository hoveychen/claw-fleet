use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct RunnerGatewayConfig {
    pub listen_addr: SocketAddr,
    pub server_certificate: PathBuf,
    pub server_private_key: PathBuf,
    pub client_ca_certificate: PathBuf,
    pub client_ca_private_key: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ArtifactS3Config {
    pub endpoint: String,
    pub bucket: String,
    pub region: String,
    pub access_key: String,
    pub secret_key: String,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub database_url: String,
    pub api_key_pepper: Vec<u8>,
    pub runner_gateway: Option<RunnerGatewayConfig>,
    pub artifact_s3: Option<ArtifactS3Config>,
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
            std::env::var_os("FLEET_CLOUD_RUNNER_CLIENT_CA_KEY"),
        ) {
            (None, None, None, None) => None,
            (Some(certificate), Some(key), Some(ca), Some(ca_key)) => Some(RunnerGatewayConfig {
                listen_addr: std::env::var("FLEET_CLOUD_RUNNER_LISTEN_ADDR")
                    .unwrap_or_else(|_| "0.0.0.0:8091".into())
                    .parse()?,
                server_certificate: certificate.into(),
                server_private_key: key.into(),
                client_ca_certificate: ca.into(),
                client_ca_private_key: ca_key.into(),
            }),
            _ => anyhow::bail!(
                "Runner TLS cert, key, client CA, and client CA key must be configured together"
            ),
        };
        let artifact_s3 = match (
            std::env::var("FLEET_CLOUD_ARTIFACT_S3_ENDPOINT").ok(),
            std::env::var("FLEET_CLOUD_ARTIFACT_S3_BUCKET").ok(),
            std::env::var("FLEET_CLOUD_ARTIFACT_S3_REGION").ok(),
            std::env::var("FLEET_CLOUD_ARTIFACT_S3_ACCESS_KEY").ok(),
            std::env::var("FLEET_CLOUD_ARTIFACT_S3_SECRET_KEY").ok(),
        ) {
            (None, None, None, None, None) => None,
            (Some(endpoint), Some(bucket), Some(region), Some(access_key), Some(secret_key)) => {
                Some(ArtifactS3Config {
                    endpoint,
                    bucket,
                    region,
                    access_key,
                    secret_key,
                })
            }
            _ => anyhow::bail!("Artifact S3 endpoint, bucket, region, access key, and secret key must be configured together"),
        };
        Ok(Self {
            listen_addr,
            database_url,
            api_key_pepper,
            runner_gateway,
            artifact_s3,
        })
    }
}
