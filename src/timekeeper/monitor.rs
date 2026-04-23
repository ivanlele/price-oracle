use std::collections::HashMap;

use electrsd::bitcoind::bitcoincore_rpc::RpcApi;
use serde::Deserialize;
use serde_json::Value;

use super::{Error, Timekeeper};

pub(super) const BLOCK_MONITOR_INTERVAL_SECONDS: u64 = 2;

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

impl Timekeeper {
    pub(crate) async fn scan_new_blocks_for_tick_spends(&self) -> Result<(), Error> {
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
                    self.persist_last_scanned_block(tip_height, &tip_hash)
                        .await?;
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
                self.persist_last_scanned_block(tip_height, &tip_hash)
                    .await?;
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
            self.persist_last_scanned_block(tip_height, &tip_hash)
                .await?;
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
                    self.persist_last_scanned_block(tip_height, &tip_hash)
                        .await?;
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
            let tick_txid = match tick.txid.parse() {
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
            self.db
                .mark_timekeeper_tick_utxos_unspent(&unspent_ids)
                .await?;
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

                if let Some(tick_id) =
                    tracked_outpoints.remove(&(prev_txid.to_ascii_lowercase(), prev_vout))
                {
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
            .upsert_timekeeper_monitor_state(height as i64, hash, super::now_unix())
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
}
