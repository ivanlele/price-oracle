use std::sync::Arc;

use crate::handlers::state::DbState;
use electrsd::bitcoind::bitcoincore_rpc::Client as RpcClient;
use simplex::signer::Signer;
use simplex::simplicityhl::elements::{self, AssetId};

mod init;
mod lifecycle;
mod monitor;
mod sweep;
mod tick;
mod utils;

#[allow(unused_imports)]
pub use utils::{covenant_spk, now_unix, script_hash};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("rpc error: {0}")]
    Rpc(String),
    #[error("signer error: {0}")]
    Signer(String),
    #[error("no L-BTC UTXOs available")]
    NoUtxos,
    #[error("no supply UTXO available")]
    NoSupplyUtxo,
    #[error("insufficient timestamp supply: available {available}, required {required}")]
    InsufficientSupply { available: u64, required: u64 },
    #[error("broadcast error: {0}")]
    Broadcast(String),
}

pub struct Timekeeper {
    signer: Signer,
    rpc: Arc<RpcClient>,
    db: DbState,
    asset_id: AssetId,
    issuer_spk: elements::Script,
    issuer_script_hash: [u8; 32],
    covenant_spk: elements::Script,
    publish_interval_seconds: u64,
    return_to_issuer_interval_seconds: u64,
    finalization_timeout_seconds: u64,
    tx_lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for Timekeeper {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Timekeeper")
            .field("asset_id", &self.asset_id)
            .finish_non_exhaustive()
    }
}

unsafe impl Send for Timekeeper {}
unsafe impl Sync for Timekeeper {}

#[derive(Debug, Clone, serde::Serialize)]
pub struct TickResult {
    pub txid: String,
    pub timestamp: u64,
    pub asset_id: String,
}
