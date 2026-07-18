#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub cloud_url: String,
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        let cloud_url = std::env::var("FLEET_CLOUD_URL")
            .unwrap_or_else(|_| "https://fleet-cloud.muveeai.com".into());
        Ok(Self { cloud_url })
    }
}
