use std::net::{IpAddr, Ipv4Addr, SocketAddr};

#[derive(Debug, Clone)]
pub struct Config {
    pub listen_addr: SocketAddr,
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
        Ok(Self { listen_addr })
    }
}
