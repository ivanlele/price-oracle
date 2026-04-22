use electrsd::bitcoind::bitcoincore_rpc::RpcApi;
use serde_json::Value;
use simplex::simplicityhl::elements::Txid;
use simplex::transaction::{
    FinalTransaction, PartialInput, PartialOutput, RequiredSignature, utxo::UTXO,
};

use super::{Error, TickResult, Timekeeper};

impl Timekeeper {
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

    pub(crate) fn total_issuer_asset_amount(&self) -> Result<u64, Error> {
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

    pub(crate) fn validate_utxo_exists(&self, txid: &Txid, vout: u32) -> Result<bool, Error> {
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

    pub async fn tick(&self) -> Result<Option<TickResult>, Error> {
        let _guard = self.tx_lock.lock().await;

        let timestamp = super::now_unix() as u64;
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

        let now = super::now_unix();
        let change_amount =
            supply_amount
                .checked_sub(timestamp)
                .ok_or(Error::InsufficientSupply {
                    available: supply_amount,
                    required: timestamp,
                })?;

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
}
