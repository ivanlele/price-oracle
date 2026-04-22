use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use simplex::provider::SimplicityNetwork;
use simplex::simplicityhl::elements::hashes::Hash;
use simplex::simplicityhl::elements::{self, Txid};

use crate::artifacts::timestamp_covenant::TimestampCovenantProgram;
use crate::artifacts::timestamp_covenant::derived_timestamp_covenant::TimestampCovenantArguments;

use super::{Error, Timekeeper};

/// SHA-256 hash of a script, matching the Simplicity `output_script_hash` jet.
pub fn script_hash(spk: &elements::Script) -> [u8; 32] {
    elements::hashes::sha256::Hash::hash(spk.as_bytes()).to_byte_array()
}

/// Build the covenant script_pubkey parameterised by the issuer's script hash and tick asset ID.
pub fn covenant_spk(
    issuer_script_hash: [u8; 32],
    tick_asset_id: [u8; 32],
    network: &SimplicityNetwork,
) -> elements::Script {
    let args = TimestampCovenantArguments {
        issuer_script_hash,
        tick_asset_id,
    };
    let program = TimestampCovenantProgram::new(args);
    program.get_program().get_script_pubkey(network)
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

pub(crate) async fn poll_for_confirmation(tk: &Timekeeper, txid: &Txid) -> Result<(), Error> {
    let timeout_seconds = tk.finalization_timeout_seconds;
    tracing::info!("Waiting for tx {txid} to be confirmed (timeout {timeout_seconds}s)...");
    let max_attempts = timeout_seconds / 2;

    for _ in 1..=max_attempts {
        tokio::time::sleep(Duration::from_secs(2)).await;
        if tk.signer.get_provider().fetch_transaction(txid).is_ok() {
            tracing::info!("Tx {txid} confirmed");
            return Ok(());
        }
    }

    Err(Error::Broadcast(format!(
        "Timeout waiting for tx {txid} to be confirmed after {timeout_seconds}s"
    )))
}

pub(crate) fn compute_contract_hash(contract_json: &str) -> [u8; 32] {
    let ordered: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(contract_json).expect("valid JSON");
    let canonical = serde_json::to_string(&ordered).expect("serialize");
    elements::hashes::sha256::Hash::hash(canonical.as_bytes()).to_byte_array()
}
