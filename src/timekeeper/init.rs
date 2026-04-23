use std::sync::Arc;

use electrsd::bitcoind::bitcoincore_rpc::{Auth, Client as RpcClient};
use simplex::signer::Signer;
use simplex::simplicityhl::elements::hashes::Hash;
use simplex::simplicityhl::elements::{self, AssetId, ContractHash};
use simplex::transaction::{
    FinalTransaction, PartialInput, PartialOutput, RequiredSignature, partial_input::IssuanceInput,
};

use crate::config::TimekeeperConfig;
use crate::handlers::state::DbState;

use super::{Error, Timekeeper, utils};

impl Timekeeper {
    pub(crate) async fn init(config: &TimekeeperConfig, db: DbState) -> Result<Self, Error> {
        let max_supply = config.max_supply;
        let contract_json = config.contract_json.clone();
        let publish_interval_seconds = config.publish_interval_seconds;
        let return_to_issuer_interval_seconds = config.return_to_issuer_interval_seconds;
        let finalization_timeout_seconds = config.finalization_timeout_seconds;

        if let Some(existing) = db.get_timekeeper_asset().await? {
            tracing::info!("Timestamp asset already issued: {}", existing.asset_id);
            let asset_id: AssetId = existing.asset_id.parse().expect("valid asset_id in DB");

            let signer = config.signer();
            let rpc = Arc::new(
                RpcClient::new(
                    &config.rpc_url,
                    Auth::UserPass(config.rpc_user.clone(), config.rpc_password.clone()),
                )
                .map_err(|e| Error::Signer(format!("create rpc client: {e}")))?,
            );
            let issuer_spk = signer.get_address().script_pubkey();
            let issuer_script_hash = utils::script_hash(&issuer_spk);
            let network = *signer.get_provider().get_network();
            let tick_asset_id = asset_id.into_inner().to_byte_array();
            let covenant_spk = utils::covenant_spk(issuer_script_hash, tick_asset_id, &network);

            return Ok(Self {
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
            });
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
            let issuer_script_hash = utils::script_hash(&issuer_spk);
            let network = *signer.get_provider().get_network();

            tracing::info!("Issuing timestamp asset with max supply {max_supply}...");
            let (asset_id, issuance_txid) =
                Self::issue_asset(&signer, &issuer_spk, max_supply, &contract_json)?;

            let tick_asset_id = asset_id.into_inner().to_byte_array();
            let covenant_spk = utils::covenant_spk(issuer_script_hash, tick_asset_id, &network);

            let now = utils::now_unix();
            let contract_hash_hex = hex::encode(utils::compute_contract_hash(&contract_json));

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

    fn issue_asset(
        signer: &Signer,
        issuer_spk: &elements::Script,
        max_supply: u64,
        contract_json: &str,
    ) -> Result<(AssetId, elements::Txid), Error> {
        let contract_hash_bytes = utils::compute_contract_hash(contract_json);

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
}
