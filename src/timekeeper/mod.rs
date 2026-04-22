use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::config::TimekeeperConfig;
use crate::handlers::state::DbState;
use electrsd::bitcoind::bitcoincore_rpc::{Auth, Client as RpcClient, RpcApi};
use serde::Deserialize;
use serde_json::Value;
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

const BLOCK_MONITOR_INTERVAL_SECONDS: u64 = 2;

#[derive(Debug, Deserialize)]
struct BlockTxVin {
    txid: Option<String>,
    vout: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct BlockTx {
    txid: String,
    vin: Vec<BlockTxVin>,
}

#[derive(Debug, Deserialize)]
struct VerboseBlock {
    tx: Vec<BlockTx>,
}

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
    /// Guards tick and return-to-issuer so only one runs at a time,
    /// and each waits for TX finalization before releasing.
    tx_lock: tokio::sync::Mutex<()>,
}

impl Timekeeper {
    /// Find the supply UTXO (the one at the issuer address with the timestamp asset).
    fn get_supply_utxo(&self) -> Result<UTXO, Error> {
        let utxos = self
            .signer
            .get_utxos()
            .map_err(|e| Error::Signer(e.to_string()))?;

        let supply_utxo = utxos
            .into_iter()
            .filter(|u| u.explicit_asset() == self.asset_id)
            .max_by_key(|u| u.explicit_amount())
            .ok_or(Error::NoSupplyUtxo)?;

        Ok(supply_utxo)
    }

    fn total_issuer_asset_amount(&self) -> Result<u64, Error> {
        let utxos = self
            .signer
            .get_utxos()
            .map_err(|e| Error::Signer(e.to_string()))?;

        Ok(utxos
            .into_iter()
            .filter(|u| u.explicit_asset() == self.asset_id)
            .map(|u| u.explicit_amount())
            .sum())
    }

    /// Validate that an outpoint is currently unspent using bitcoind `gettxout`.
    fn validate_utxo_exists(&self, txid: &Txid, vout: u32) -> Result<bool, Error> {
        let response: Value = self
            .rpc
            .call(
                "gettxout",
                &[
                    Value::String(txid.to_string()),
                    Value::from(vout),
                    Value::Bool(true),
                ],
            )
            .map_err(|e| Error::Signer(format!("gettxout {txid}:{vout}: {e}")))?;

        Ok(!response.is_null())
    }

    async fn supply_utxo_for_tick(&self, timestamp: u64) -> Result<Option<UTXO>, Error> {
        let supply_utxo = self.get_supply_utxo()?;

        if supply_utxo.explicit_amount() >= timestamp {
            return Ok(Some(supply_utxo));
        }

        self.replenish_supply_for_tick(timestamp, supply_utxo.explicit_amount())
            .await
    }

    async fn replenish_supply_for_tick(
        &self,
        timestamp: u64,
        supply_amount: u64,
    ) -> Result<Option<UTXO>, Error> {
        tracing::info!(
            timestamp,
            supply_amount,
            "Timestamp exceeds available supply, attempting return-to-issuer before ticking"
        );

        let Some((txid, count, new_supply)) = self.return_expired_ticks_locked().await? else {
            tracing::info!(
                timestamp,
                supply_amount,
                "Skipping tick: insufficient supply and no expired ticks available to return"
            );
            return Ok(None);
        };

        tracing::info!(
            txid = %txid,
            returned = count,
            new_supply,
            "Returned expired tick UTXOs to replenish supply before ticking"
        );
        self.wait_for_finalization(&txid).await?;

        let replenished_supply = self.get_supply_utxo()?;
        let replenished_amount = replenished_supply.explicit_amount();

        if replenished_amount < timestamp {
            tracing::info!(
                timestamp,
                supply_amount = replenished_amount,
                "Skipping tick: supply is still insufficient after return-to-issuer"
            );
            return Ok(None);
        }

        Ok(Some(replenished_supply))
    }
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
                let rpc = Arc::new(
                    RpcClient::new(
                        &config.rpc_url,
                        Auth::UserPass(config.rpc_user.clone(), config.rpc_password.clone()),
                    )
                    .map_err(|e| Error::Signer(format!("create rpc client: {e}")))?,
                );
                let issuer_spk = signer.get_address().script_pubkey();
                let issuer_script_hash = script_hash(&issuer_spk);
                let network = *signer.get_provider().get_network();
                let tick_asset_id = asset_id.into_inner().to_byte_array();
                let covenant_spk = covenant_spk(issuer_script_hash, tick_asset_id, &network);
                Self {
                    signer,
                    rpc,
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
            let rpc = Arc::new(
                RpcClient::new(
                    &config.rpc_url,
                    Auth::UserPass(config.rpc_user.clone(), config.rpc_password.clone()),
                )
                .map_err(|e| Error::Signer(format!("create rpc client: {e}")))?,
            );
            let issuer_spk = signer.get_address().script_pubkey();
            let issuer_script_hash = script_hash(&issuer_spk);
            let network = *signer.get_provider().get_network();

            tracing::info!("Issuing timestamp asset with max supply {max_supply}...");
            let (asset_id, issuance_txid) =
                Self::issue_asset(&signer, &issuer_spk, max_supply, &contract_json)?;

            let tick_asset_id = asset_id.into_inner().to_byte_array();
            let covenant_spk = covenant_spk(issuer_script_hash, tick_asset_id, &network);

            let now = now_unix();
            let contract_hash_hex = hex::encode(compute_contract_hash(&contract_json));

            let tk = Self {
                signer,
                rpc,
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
        let this_monitor = this.clone();

        tokio::spawn(async move {
            loop {
                if let Err(e) = this_monitor.scan_new_blocks_for_tick_spends().await {
                    tracing::error!("Timekeeper block monitor failed: {e}");
                }

                tokio::time::sleep(Duration::from_secs(BLOCK_MONITOR_INTERVAL_SECONDS)).await;
            }
        });

        // Background timestamp issuance loop
        tokio::spawn(async move {
            loop {
                match this_tick.tick().await {
                    Ok(Some(_)) => {}
                    Ok(None) => {}
                    Err(e) => tracing::error!("Timestamp tick failed: {e}"),
                }

                tokio::time::sleep(Duration::from_secs(tick_interval)).await;
            }
        });

        // Background return-to-issuer loop
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(return_interval)).await;

                match this_return.return_expired_ticks().await {
                    Ok(_) => {}
                    Err(e) => tracing::error!("Return-to-issuer failed: {e}"),
                }
            }
        });
    }

    async fn wait_for_finalization(&self, txid: &Txid) -> Result<(), Error> {
        poll_for_confirmation(self, txid).await
    }

    async fn scan_new_blocks_for_tick_spends(&self) -> Result<(), Error> {
        let tip_height = self.get_block_count()?;
        let state = self.db.get_timekeeper_monitor_state().await?;

        let start_height = match state {
            Some(state) => {
                let last_scanned_height = state.last_scanned_height.max(0) as u64;
                if last_scanned_height > tip_height {
                    tracing::warn!(
                        last_scanned_height,
                        tip_height,
                        "Stored timekeeper monitor height is ahead of the chain tip; resetting to tip"
                    );
                    let tip_hash = self.get_block_hash(tip_height)?;
                    self.persist_last_scanned_block(tip_height, &tip_hash).await?;
                    return Ok(());
                }

                let current_hash = self.get_block_hash(last_scanned_height)?;
                if current_hash != state.last_scanned_hash {
                    tracing::warn!(
                        last_scanned_height,
                        stored_hash = %state.last_scanned_hash,
                        current_hash = %current_hash,
                        "Detected timekeeper monitor reorg; reconciling tracked tick state against current chain"
                    );
                    self.reconcile_all_tick_state().await?;
                    self.persist_last_scanned_block(last_scanned_height, &current_hash)
                        .await?;
                }

                last_scanned_height.saturating_add(1)
            }
            None => {
                self.reconcile_all_tick_state().await?;
                let tip_hash = self.get_block_hash(tip_height)?;
                self.persist_last_scanned_block(tip_height, &tip_hash).await?;
                tracing::info!(
                    tip_height,
                    "Initialized timekeeper block monitor cursor at current tip"
                );
                return Ok(());
            }
        };

        if start_height > tip_height {
            return Ok(());
        }

        let tracked_ticks = self.db.get_all_unspent_timekeeper_tick_utxos().await?;
        if tracked_ticks.is_empty() {
            let tip_hash = self.get_block_hash(tip_height)?;
            self.persist_last_scanned_block(tip_height, &tip_hash).await?;
            return Ok(());
        }

        let mut tracked_outpoints: HashMap<(String, u32), i32> = tracked_ticks
            .into_iter()
            .map(|tick| ((tick.txid.to_ascii_lowercase(), tick.vout as u32), tick.id))
            .collect();

        for height in start_height..=tip_height {
            let spent_ids = self
                .find_spent_tick_ids_in_block(height, &mut tracked_outpoints)
                .await?;
            if !spent_ids.is_empty() {
                self.db.mark_timekeeper_tick_utxos_spent(&spent_ids).await?;
            }

            let block_hash = self.get_block_hash(height)?;
            self.persist_last_scanned_block(height, &block_hash).await?;

            if tracked_outpoints.is_empty() {
                if height < tip_height {
                    let tip_hash = self.get_block_hash(tip_height)?;
                    self.persist_last_scanned_block(tip_height, &tip_hash).await?;
                }
                break;
            }
        }

        Ok(())
    }

    async fn reconcile_all_tick_state(&self) -> Result<(), Error> {
        let tracked_ticks = self.db.get_all_timekeeper_tick_utxos().await?;
        if tracked_ticks.is_empty() {
            return Ok(());
        }

        let mut spent_ids = Vec::new();
        let mut unspent_ids = Vec::new();
        for tick in tracked_ticks {
            let tick_txid: Txid = match tick.txid.parse() {
                Ok(txid) => txid,
                Err(_) => {
                    spent_ids.push(tick.id);
                    continue;
                }
            };

            if self.validate_utxo_exists(&tick_txid, tick.vout as u32)? {
                unspent_ids.push(tick.id);
            } else {
                spent_ids.push(tick.id);
            }
        }

        if !spent_ids.is_empty() {
            self.db.mark_timekeeper_tick_utxos_spent(&spent_ids).await?;
        }

        if !unspent_ids.is_empty() {
            self.db.mark_timekeeper_tick_utxos_unspent(&unspent_ids).await?;
        }

        tracing::info!(
            spent = spent_ids.len(),
            unspent = unspent_ids.len(),
            "Reconciled timekeeper ticks against current chain state"
        );

        Ok(())
    }

    async fn find_spent_tick_ids_in_block(
        &self,
        height: u64,
        tracked_outpoints: &mut HashMap<(String, u32), i32>,
    ) -> Result<Vec<i32>, Error> {
        let block = self.get_block(height)?;
        let mut spent_ids = Vec::new();

        for tx in block.tx {
            for vin in tx.vin {
                let (Some(prev_txid), Some(prev_vout)) = (vin.txid, vin.vout) else {
                    continue;
                };

                if let Some(tick_id) = tracked_outpoints.remove(&(prev_txid.to_ascii_lowercase(), prev_vout)) {
                    tracing::info!(
                        height,
                        spending_txid = %tx.txid,
                        spent_tick_txid = %prev_txid,
                        spent_tick_vout = prev_vout,
                        "Observed timekeeper tick spend in new block"
                    );
                    spent_ids.push(tick_id);
                }
            }
        }

        Ok(spent_ids)
    }

    async fn persist_last_scanned_block(&self, height: u64, hash: &str) -> Result<(), Error> {
        self.db
            .upsert_timekeeper_monitor_state(height as i64, hash, now_unix())
            .await?;
        Ok(())
    }

    fn get_block_count(&self) -> Result<u64, Error> {
        self.rpc
            .call("getblockcount", &[])
            .map_err(|e| Error::Rpc(format!("getblockcount: {e}")))
    }

    fn get_block(&self, height: u64) -> Result<VerboseBlock, Error> {
        let block_hash = self.get_block_hash(height)?;

        self.rpc
            .call("getblock", &[Value::String(block_hash), Value::from(2)])
            .map_err(|e| Error::Rpc(format!("getblock {height}: {e}")))
    }

    fn get_block_hash(&self, height: u64) -> Result<String, Error> {
        self.rpc
            .call("getblockhash", &[Value::from(height)])
            .map_err(|e| Error::Rpc(format!("getblockhash {height}: {e}")))
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
    pub async fn tick(&self) -> Result<Option<TickResult>, Error> {
        let _guard = self.tx_lock.lock().await;

        let timestamp = now_unix() as u64;
        let Some(supply_utxo) = self.supply_utxo_for_tick(timestamp).await? else {
            return Ok(None);
        };
        let supply_amount = supply_utxo.explicit_amount();

        tracing::info!(
            timestamp,
            supply_txid = %supply_utxo.outpoint.txid,
            supply_vout = supply_utxo.outpoint.vout,
            supply_amount,
            "About to issue tick with supply UTXO"
        );

        let result = self.build_sign_broadcast_tick(timestamp, &supply_utxo)?;

        let now = now_unix();
        let change_amount =
            supply_amount
                .checked_sub(timestamp)
                .ok_or(Error::InsufficientSupply {
                    available: supply_amount,
                    required: timestamp,
                })?;

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

        Ok(Some(result))
    }

    fn build_sign_broadcast_tick(
        &self,
        timestamp: u64,
        supply_utxo: &UTXO,
    ) -> Result<TickResult, Error> {
        let supply_amount = supply_utxo.explicit_amount();
        let change_amount =
            supply_amount
                .checked_sub(timestamp)
                .ok_or(Error::InsufficientSupply {
                    available: supply_amount,
                    required: timestamp,
                })?;

        // Validate that supply UTXO still exists before trying to spend it.
        if !self.validate_utxo_exists(&supply_utxo.outpoint.txid, supply_utxo.outpoint.vout)? {
            tracing::error!(
                supply_txid = %supply_utxo.outpoint.txid,
                supply_vout = supply_utxo.outpoint.vout,
                "Supply UTXO no longer available when building tick (may have been spent)"
            );
            return Err(Error::NoSupplyUtxo);
        }

        let mut ft = FinalTransaction::new();

        ft.add_input(
            PartialInput::new(supply_utxo.clone()),
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

        let txid = self.signer.broadcast(&ft).map_err(|e| {
            tracing::error!(
                error = %e,
                input_txid = %supply_utxo.outpoint.txid,
                input_vout = supply_utxo.outpoint.vout,
                input_amount = supply_amount,
                "Failed to broadcast tick transaction"
            );
            Error::Broadcast(e.to_string())
        })?;

        Ok(TickResult {
            txid: txid.to_string(),
            timestamp,
            asset_id: self.asset_id.to_string(),
        })
    }

    /// Sweep expired tick UTXOs back to the issuer.
    async fn return_expired_ticks(&self) -> Result<(), Error> {
        let _guard = self.tx_lock.lock().await;
        let Some((txid, count, new_supply)) = self.return_expired_ticks_locked().await? else {
            return Ok(());
        };

        tracing::info!(
            txid = %txid,
            returned = count,
            new_supply,
            "Returned expired tick UTXOs to supply"
        );
        match self.wait_for_finalization(&txid).await {
            Ok(_) => Ok(()),
            Err(e) => {
                tracing::warn!(
                    txid = %txid,
                    error = %e,
                    "Return-to-issuer transaction failed to confirm"
                );
                Err(e)
            }
        }
    }

    async fn return_expired_ticks_locked(&self) -> Result<Option<(Txid, usize, u64)>, Error> {
        let expired = self
            .db
            .get_expired_timekeeper_tick_utxos(self.return_to_issuer_interval_seconds as i64)
            .await?;

        if expired.is_empty() {
            return Ok(None);
        }

        // Filter out expired ticks whose transactions cannot be fetched (may already be spent)
        let mut valid_expired = Vec::new();
        let mut invalid_ids = Vec::new();
        for tick in expired {
            let tick_txid: Txid = match tick.txid.parse() {
                Ok(id) => id,
                Err(_) => {
                    invalid_ids.push(tick.id);
                    continue;
                }
            };
            if self.validate_utxo_exists(&tick_txid, tick.vout as u32)? {
                valid_expired.push(tick);
            } else {
                tracing::warn!(
                    txid = %tick_txid,
                    vout = tick.vout,
                    "Skipping expired tick: outpoint is spent or unavailable"
                );
                invalid_ids.push(tick.id);
            }
        }

        // Mark unavailable UTXOs as spent to avoid repeated attempts
        if !invalid_ids.is_empty() {
            self.db
                .mark_timekeeper_tick_utxos_spent(&invalid_ids)
                .await?;
        }

        if valid_expired.is_empty() {
            return Ok(None);
        }

        let tick = valid_expired
            .into_iter()
            .min_by_key(|tick| tick.created_at)
            .expect("non-empty valid_expired");

        let (txid, new_supply) = self.build_broadcast_return(&tick)?;

        self.db.mark_timekeeper_tick_utxos_spent(&[tick.id]).await?;

        Ok(Some((txid, 1, new_supply)))
    }

    /// Build and broadcast a return-to-issuer transaction for one expired tick.
    fn build_broadcast_return(
        &self,
        tick: &crate::db::timekeeper_utxos::TimekeeperUtxo,
    ) -> Result<(Txid, u64), Error> {
        let mut ft = FinalTransaction::new();

        let args = TimestampCovenantArguments {
            issuer_script_hash: self.issuer_script_hash,
            tick_asset_id: self.asset_id.into_inner().to_byte_array(),
        };

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

        let prog = TimestampCovenantProgram::new(args);
        let pi = ProgramInput::new(
            Box::new(prog.get_program().clone()),
            Box::new(TimestampCovenantWitness {}),
        );
        ft.add_program_input(PartialInput::new(tick_utxo), pi, RequiredSignature::None);

        ft.add_output(PartialOutput::new(
            self.issuer_spk.clone(),
            tick.amount as u64,
            self.asset_id,
        ));

        let new_supply = self.total_issuer_asset_amount()? + tick.amount as u64;

        let txid = self.signer.broadcast(&ft).map_err(|e| {
            tracing::error!(
                error = %e,
                tick_txid = %tick.txid,
                tick_vout = tick.vout,
                tick_amount = tick.amount,
                "Failed to broadcast return transaction"
            );
            Error::Broadcast(e.to_string())
        })?;

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
