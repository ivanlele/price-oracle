use serde::Deserialize;
use std::path::PathBuf;

use electrsd::bitcoind::bitcoincore_rpc::Auth;
use simplex::provider::SimplexProvider;
use simplex::provider::SimplicityNetwork;
use simplex::signer::Signer;

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
    pub timekeeper: TimekeeperConfig,
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
    pub max_connections: u32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TimekeeperConfig {
    pub rpc_url: String,
    pub rpc_user: String,
    pub rpc_password: String,
    pub esplora_url: String,
    pub signer_mnemonic: String,
    pub publish_interval_seconds: u64,
    pub return_to_issuer_interval_seconds: u64,
    pub finalization_timeout_seconds: u64,
    pub max_supply: u64,
    pub contract_json: String,
}

impl TimekeeperConfig {
    pub fn signer(&self) -> Signer {
        let auth = Auth::UserPass(self.rpc_user.clone(), self.rpc_password.clone());

        let provider = SimplexProvider::new(
            self.esplora_url.clone(),
            self.rpc_url.clone(),
            auth,
            SimplicityNetwork::default_regtest(),
        );

        Signer::new(&self.signer_mnemonic, Box::new(provider))
    }
}

#[derive(Debug, Deserialize)]
pub struct FeedCrawlerConfig {
    pub interval_seconds: u64,
    pub validity_seconds: u64,
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
