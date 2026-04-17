use sqlx::Row;

use crate::handlers::state::DbState;

#[allow(unused)]
#[derive(Debug, Clone)]
pub struct TimekeeperAsset {
    pub id: i32,
    pub asset_id: String,
    pub issuance_txid: String,
    pub contract_hash: String,
    pub created_at: i64,
}

impl TimekeeperAsset {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            asset_id: row.try_get("asset_id")?,
            issuance_txid: row.try_get("issuance_txid")?,
            contract_hash: row.try_get("contract_hash")?,
            created_at: row.try_get("created_at")?,
        })
    }
}

impl DbState {
    pub async fn get_timekeeper_asset(&self) -> Result<Option<TimekeeperAsset>, sqlx::Error> {
        let row = sqlx::query("SELECT id, asset_id, issuance_txid, contract_hash, created_at FROM timekeeper_assets ORDER BY id LIMIT 1")
            .fetch_optional(&self.pool)
            .await?;

        match row {
            Some(row) => Ok(Some(TimekeeperAsset::from_row(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn insert_timekeeper_asset(
        &self,
        asset_id: &str,
        issuance_txid: &str,
        contract_hash: &str,
        created_at: i64,
    ) -> Result<TimekeeperAsset, sqlx::Error> {
        let row = sqlx::query(
            "INSERT INTO timekeeper_assets (asset_id, issuance_txid, contract_hash, created_at) \
             VALUES ($1, $2, $3, $4) \
             RETURNING id, asset_id, issuance_txid, contract_hash, created_at",
        )
        .bind(asset_id)
        .bind(issuance_txid)
        .bind(contract_hash)
        .bind(created_at)
        .fetch_one(&self.pool)
        .await?;

        TimekeeperAsset::from_row(&row)
    }
}
