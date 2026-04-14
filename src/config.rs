use serde::Deserialize;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to read config file: {0}")]
    Io(std::io::Error),
    #[error("failed to parse config file: {0}")]
    Toml(toml::de::Error),
}

#[derive(Debug, Deserialize)]
pub struct Config {
    pub service: ServiceConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServiceConfig {
    pub port: u16,
    pub signer: SignerConfig,
    pub db: DbConfig,
    pub feed_crawler: FeedCrawlerConfig,
}

#[derive(Debug, Deserialize)]
pub struct SignerConfig {
    pub private_key: String,
}

#[derive(Debug, Deserialize)]
pub struct DbConfig {
    pub url: String,
    pub username: String,
    pub password: String,
    pub database: String,
}

#[derive(Debug, Deserialize)]
pub struct FeedCrawlerConfig {
    pub interval_seconds: u64,
    pub rpc_url: String,
    pub feeds: Vec<String>,
    pub addresses: Vec<String>,
}

impl Config {
    pub fn from_file(path: PathBuf) -> Result<Self, Error> {
        let contents = std::fs::read_to_string(path).map_err(Error::Io)?;
        let config: Config = toml::from_str(&contents).map_err(Error::Toml)?;

        Ok(config)
    }
}
