use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::TimekeeperConfig;
use crate::handlers::state::DbState;
use simplex::provider::SimplicityNetwork;
use simplex::signer::Signer;
use simplex::simplicityhl::elements::hashes::Hash;
use simplex::simplicityhl::elements::{self, AssetId, ContractHash, OutPoint, Txid};
use simplex::transaction::{
    FinalTransaction, PartialInput, PartialOutput, RequiredSignature,
    partial_input::{IssuanceInput, ProgramInput},
    utxo::UTXO,
};

use crate::artifacts::timestamp_covenant::TimestampCovenantProgram;
use crate::artifacts::timestamp_covenant::derived_timestamp_covenant::{
    TimestampCovenantArguments, TimestampCovenantWitness,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] sqlx::Error),
    #[error("signer error: {0}")]
    Signer(String),
    #[error("no L-BTC UTXOs available")]
    NoUtxos,
    #[error("no supply UTXO available")]
    NoSupplyUtxo,
    #[error("broadcast error: {0}")]
    Broadcast(String),
}

/// SHA-256 hash of a script, matching the Simplicity `output_script_hash` jet.
pub fn script_hash(spk: &elements::Script) -> [u8; 32] {
    elements::hashes::sha256::Hash::hash(spk.as_bytes()).to_byte_array()
}

/// Build the covenant script_pubkey parameterised by the issuer's script hash.
pub fn covenant_spk(issuer_script_hash: [u8; 32], network: &SimplicityNetwork) -> elements::Script {
    let args = TimestampCovenantArguments { issuer_script_hash };
    let program = TimestampCovenantProgram::new(args);
    program.get_program().get_script_pubkey(network)
}

pub struct Timekeeper {
    signer: Signer,
    db: DbState,
    asset_id: AssetId,
    issuer_spk: elements::Script,
    issuer_script_hash: [u8; 32],
    covenant_spk: elements::Script,
    publish_interval_seconds: u64,
    return_to_issuer_interval_seconds: u64,
    finalization_timeout_seconds: u64,
    /// Guards tick and return-to-issuer so only one runs at a time,
    /// and each waits for TX finalization before releasing.
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

impl Timekeeper {
    async fn init(config: &TimekeeperConfig, db: DbState) -> Result<Self, Error> {
        let max_supply = config.max_supply;
        let contract_json = config.contract_json.clone();
        let publish_interval_seconds = config.publish_interval_seconds;
        let return_to_issuer_interval_seconds = config.return_to_issuer_interval_seconds;
        let finalization_timeout_seconds = config.finalization_timeout_seconds;

        // Check if already issued
        if let Some(existing) = db.get_timekeeper_asset().await? {
            tracing::info!("Timestamp asset already issued: {}", existing.asset_id);
            let asset_id: AssetId = existing.asset_id.parse().expect("valid asset_id in DB");

            let tk = {
                let signer = config.signer();
                let issuer_spk = signer.get_address().script_pubkey();
                let issuer_script_hash = script_hash(&issuer_spk);
                let network = *signer.get_provider().get_network();
                let covenant_spk = covenant_spk(issuer_script_hash, &network);
                Self {
                    signer,
                    db,
                    asset_id,
                    issuer_spk,
                    issuer_script_hash,
                    covenant_spk,
                    publish_interval_seconds,
                    return_to_issuer_interval_seconds,
                    finalization_timeout_seconds,
                    tx_lock: tokio::sync::Mutex::new(()),
                }
            };
            return Ok(tk);
        }

        let (tk, issuance_txid, contract_hash_hex, now) = {
            let signer = config.signer();
            let issuer_spk = signer.get_address().script_pubkey();
            let issuer_script_hash = script_hash(&issuer_spk);
            let network = *signer.get_provider().get_network();
            let covenant_spk = covenant_spk(issuer_script_hash, &network);

            tracing::info!("Issuing timestamp asset with max supply {max_supply}...");
            let (asset_id, issuance_txid) =
                Self::issue_asset(&signer, &issuer_spk, max_supply, &contract_json)?;

            let now = now_unix();
            let contract_hash_hex = hex::encode(compute_contract_hash(&contract_json));

            let tk = Self {
                signer,
                db,
                asset_id,
                issuer_spk,
                issuer_script_hash,
                covenant_spk,
                publish_interval_seconds,
                return_to_issuer_interval_seconds,
                finalization_timeout_seconds,
                tx_lock: tokio::sync::Mutex::new(()),
            };

            (tk, issuance_txid, contract_hash_hex, now)
        };

        tk.db
            .insert_timekeeper_asset(
                &tk.asset_id.to_string(),
                &issuance_txid.to_string(),
                &contract_hash_hex,
                now,
            )
            .await?;

        tk.db
            .insert_timekeeper_supply_utxo(&issuance_txid.to_string(), 0, max_supply as i64, now)
            .await?;

        tracing::info!(
            "Timestamp asset issued: {}, txid: {issuance_txid}",
            tk.asset_id
        );

        tk.wait_for_finalization(&issuance_txid).await?;

        Ok(tk)
    }

    /// Spawn the timekeeper as a background task
    pub fn start(config: TimekeeperConfig, db: DbState) {
        tokio::spawn(async move {
            let tk = match Self::init(&config, db).await {
                Ok(tk) => tk,
                Err(e) => {
                    tracing::error!("Timekeeper init failed: {e}");
                    return;
                }
            };

            tk.run().await;
        });
    }

    async fn run(self) {
        tracing::info!("Starting timekeeper with asset {}", self.asset_id);

        let tick_interval = self.publish_interval_seconds;
        let return_interval = self.return_to_issuer_interval_seconds;

        let this = std::sync::Arc::new(self);
        let this_tick = this.clone();
        let this_return = this.clone();

        // Background timestamp issuance loop
        tokio::spawn(async move {
            loop {
                match this_tick.tick().await {
                    Ok(_) => {}
                    Err(e) => tracing::error!("Timestamp tick failed: {e}"),
                }

                tokio::time::sleep(Duration::from_secs(tick_interval)).await;
            }
        });

        // Background return-to-issuer loop
        tokio::spawn(async move {
            loop {
                match this_return.return_expired_ticks().await {
                    Ok(_) => {}
                    Err(e) => tracing::error!("Return-to-issuer failed: {e}"),
                }

                tokio::time::sleep(Duration::from_secs(return_interval)).await;
            }
        });
    }

    async fn wait_for_finalization(&self, txid: &Txid) -> Result<(), Error> {
        poll_for_confirmation(self, txid).await
    }

    /// One-time asset issuance. Sends the full supply to the issuer's address.
    fn issue_asset(
        signer: &Signer,
        issuer_spk: &elements::Script,
        max_supply: u64,
        contract_json: &str,
    ) -> Result<(AssetId, elements::Txid), Error> {
        let contract_hash_bytes = compute_contract_hash(contract_json);

        let utxos = signer
            .get_utxos()
            .map_err(|e| Error::Signer(e.to_string()))?;
        let utxo = utxos.into_iter().next().ok_or(Error::NoUtxos)?;

        let outpoint = utxo.outpoint;
        let contract_hash = ContractHash::from_byte_array(contract_hash_bytes);
        let entropy = AssetId::generate_asset_entropy(outpoint, contract_hash);
        let asset_id = AssetId::from_entropy(entropy);

        let mut ft = FinalTransaction::new();
        let issuance = IssuanceInput::new(max_supply, contract_hash_bytes);
        ft.add_issuance_input(
            PartialInput::new(utxo),
            issuance,
            RequiredSignature::NativeEcdsa,
        );

        ft.add_output(PartialOutput::new(issuer_spk.clone(), max_supply, asset_id));

        let txid = signer
            .broadcast(&ft)
            .map_err(|e| Error::Broadcast(e.to_string()))?;

        Ok((asset_id, txid))
    }

    /// Issue a new timestamp UTXO
    pub async fn tick(&self) -> Result<TickResult, Error> {
        let _guard = self.tx_lock.lock().await;

        let timestamp = now_unix() as u64;

        let utxo_row = self
            .db
            .get_current_timekeeper_supply_utxo()
            .await?
            .ok_or(Error::NoSupplyUtxo)?;

        let result = self.build_sign_broadcast_tick(timestamp, &utxo_row)?;

        self.db
            .spend_timekeeper_supply_utxo(&utxo_row.txid, utxo_row.vout)
            .await?;

        let now = now_unix();
        let change_amount = utxo_row.amount as u64 - timestamp;

        // Output 0 is the change (back to issuer)
        self.db
            .insert_timekeeper_supply_utxo(&result.txid, 0, change_amount as i64, now)
            .await?;

        // Output 1 is the tick UTXO (at covenant address)
        self.db
            .insert_timekeeper_tick_utxo(&result.txid, 1, timestamp as i64, now)
            .await?;

        tracing::info!(
            txid = %result.txid,
            timestamp,
            asset_id = %result.asset_id,
            remaining = change_amount,
            "Timestamp issued"
        );

        let txid: Txid = result.txid.parse().expect("valid txid just broadcast");
        self.wait_for_finalization(&txid).await?;

        Ok(result)
    }

    fn build_sign_broadcast_tick(
        &self,
        timestamp: u64,
        supply_row: &crate::db::timekeeper_utxos::TimekeeperUtxo,
    ) -> Result<TickResult, Error> {
        let supply_amount = supply_row.amount as u64;

        // Fetch the supply UTXO (at the issuer's address)
        let supply_txid: Txid = supply_row.txid.parse().expect("valid txid in DB");
        let tx = self
            .signer
            .get_provider()
            .fetch_transaction(&supply_txid)
            .map_err(|e| Error::Signer(format!("fetch supply tx: {e}")))?;

        let supply_vout = supply_row.vout as u32;
        let supply_txout = tx
            .output
            .get(supply_vout as usize)
            .ok_or(Error::NoSupplyUtxo)?
            .clone();

        let supply_utxo = UTXO {
            outpoint: OutPoint {
                txid: supply_txid,
                vout: supply_vout,
            },
            txout: supply_txout,
            secrets: None,
        };

        let change_amount = supply_amount - timestamp;

        let mut ft = FinalTransaction::new();

        ft.add_input(
            PartialInput::new(supply_utxo),
            RequiredSignature::NativeEcdsa,
        );

        ft.add_output(PartialOutput::new(
            self.issuer_spk.clone(),
            change_amount,
            self.asset_id,
        ));

        ft.add_output(PartialOutput::new(
            self.covenant_spk.clone(),
            timestamp,
            self.asset_id,
        ));

        let txid = self
            .signer
            .broadcast(&ft)
            .map_err(|e| Error::Broadcast(e.to_string()))?;

        Ok(TickResult {
            txid: txid.to_string(),
            timestamp,
            asset_id: self.asset_id.to_string(),
        })
    }

    /// Sweep expired tick UTXOs back to the issuer.
    async fn return_expired_ticks(&self) -> Result<(), Error> {
        let _guard = self.tx_lock.lock().await;

        let expired = self
            .db
            .get_expired_timekeeper_tick_utxos(self.return_to_issuer_interval_seconds as i64)
            .await?;

        if expired.is_empty() {
            return Ok(());
        }

        let covenant_row = self
            .db
            .get_current_timekeeper_supply_utxo()
            .await?
            .ok_or(Error::NoSupplyUtxo)?;

        let (txid, new_supply) = self.build_broadcast_return(&covenant_row, &expired)?;

        self.db
            .spend_timekeeper_supply_utxo(&covenant_row.txid, covenant_row.vout)
            .await?;

        let now = now_unix();
        self.db
            .insert_timekeeper_supply_utxo(&txid.to_string(), 0, new_supply as i64, now)
            .await?;

        let ids: Vec<i32> = expired.iter().map(|t| t.id).collect();
        self.db.mark_timekeeper_tick_utxos_spent(&ids).await?;

        let count = expired.len();
        tracing::info!(
            txid = %txid,
            returned = count,
            new_supply,
            "Returned expired tick UTXOs to supply"
        );

        self.wait_for_finalization(&txid).await?;

        Ok(())
    }

    /// build and broadcast the return-to-issuer transaction.
    fn build_broadcast_return(
        &self,
        supply_row: &crate::db::timekeeper_utxos::TimekeeperUtxo,
        expired: &[crate::db::timekeeper_utxos::TimekeeperUtxo],
    ) -> Result<(Txid, u64), Error> {
        // Fetch the supply UTXO (at the issuer's address)
        let supply_txid: Txid = supply_row.txid.parse().expect("valid txid in DB");
        let supply_tx = self
            .signer
            .get_provider()
            .fetch_transaction(&supply_txid)
            .map_err(|e| Error::Signer(format!("fetch supply tx: {e}")))?;

        let supply_vout = supply_row.vout as u32;
        let supply_txout = supply_tx
            .output
            .get(supply_vout as usize)
            .ok_or(Error::NoSupplyUtxo)?
            .clone();

        let supply_utxo = UTXO {
            outpoint: OutPoint {
                txid: supply_txid,
                vout: supply_vout,
            },
            txout: supply_txout,
            secrets: None,
        };

        let mut ft = FinalTransaction::new();
        let mut total_returned: u64 = 0;

        ft.add_input(
            PartialInput::new(supply_utxo),
            RequiredSignature::NativeEcdsa,
        );

        let args = TimestampCovenantArguments {
            issuer_script_hash: self.issuer_script_hash,
        };

        for tick in expired {
            let tick_txid: Txid = tick.txid.parse().expect("valid txid in DB");
            let tick_tx = self
                .signer
                .get_provider()
                .fetch_transaction(&tick_txid)
                .map_err(|e| Error::Signer(format!("fetch tick tx: {e}")))?;

            let tick_vout = tick.vout as u32;
            let tick_txout = tick_tx
                .output
                .get(tick_vout as usize)
                .ok_or(Error::NoSupplyUtxo)?
                .clone();

            let tick_utxo = UTXO {
                outpoint: OutPoint {
                    txid: tick_txid,
                    vout: tick_vout,
                },
                txout: tick_txout,
                secrets: None,
            };

            let prog = TimestampCovenantProgram::new(args.clone());
            let pi = ProgramInput::new(
                Box::new(prog.get_program().clone()),
                Box::new(TimestampCovenantWitness {}),
            );
            ft.add_program_input(PartialInput::new(tick_utxo), pi, RequiredSignature::None);

            total_returned += tick.amount as u64;
        }

        let new_supply = supply_row.amount as u64 + total_returned;

        // back to issuer address
        ft.add_output(PartialOutput::new(
            self.issuer_spk.clone(),
            new_supply,
            self.asset_id,
        ));

        let txid = self
            .signer
            .broadcast(&ft)
            .map_err(|e| Error::Broadcast(e.to_string()))?;

        Ok((txid, new_supply))
    }
}

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

async fn poll_for_confirmation(tk: &Timekeeper, txid: &Txid) -> Result<(), Error> {
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

fn compute_contract_hash(contract_json: &str) -> [u8; 32] {
    let ordered: BTreeMap<String, serde_json::Value> =
        serde_json::from_str(contract_json).expect("valid JSON");
    let canonical = serde_json::to_string(&ordered).expect("serialize");
    elements::hashes::sha256::Hash::hash(canonical.as_bytes()).to_byte_array()
}
