use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: SocketAddr,
    pub database_url: String,
    pub api_key_pepper: Vec<u8>,
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
        Ok(Self {
            listen_addr,
            database_url,
            api_key_pepper,
        })
    }
}
