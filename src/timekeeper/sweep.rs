use simplex::simplicityhl::elements::{OutPoint, Txid};
use simplex::transaction::{
    FinalTransaction, PartialInput, PartialOutput, RequiredSignature, partial_input::ProgramInput,
    utxo::UTXO,
};

use crate::artifacts::timestamp_covenant::TimestampCovenantProgram;
use crate::artifacts::timestamp_covenant::derived_timestamp_covenant::{
    TimestampCovenantArguments, TimestampCovenantWitness,
};

use super::{Error, Timekeeper};

impl Timekeeper {
    pub(crate) async fn return_expired_ticks(&self) -> Result<(), Error> {
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

    pub(crate) async fn return_expired_ticks_locked(
        &self,
    ) -> Result<Option<(Txid, usize, u64)>, Error> {
        let expired = self
            .db
            .get_expired_timekeeper_tick_utxos(self.return_to_issuer_interval_seconds as i64)
            .await?;

        if expired.is_empty() {
            return Ok(None);
        }

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
