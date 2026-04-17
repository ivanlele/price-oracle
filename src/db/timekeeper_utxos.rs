use sqlx::Row;

use crate::handlers::state::DbState;

#[allow(unused)]
#[derive(Debug, Clone)]
pub struct TimekeeperUtxo {
    pub id: i32,
    pub txid: String,
    pub vout: i32,
    pub amount: i64,
    pub created_at: i64,
    pub spent: bool,
}

impl TimekeeperUtxo {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            txid: row.try_get("txid")?,
            vout: row.try_get("vout")?,
            amount: row.try_get("amount")?,
            created_at: row.try_get("created_at")?,
            spent: row.try_get("spent")?,
        })
    }
}

impl DbState {
    pub async fn get_current_timekeeper_supply_utxo(
        &self,
    ) -> Result<Option<TimekeeperUtxo>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT * FROM timekeeper_supply_utxos WHERE spent = FALSE ORDER BY id DESC LIMIT 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        match row {
            Some(row) => Ok(Some(TimekeeperUtxo::from_row(&row)?)),
            None => Ok(None),
        }
    }

    pub async fn insert_timekeeper_supply_utxo(
        &self,
        txid: &str,
        vout: i32,
        amount: i64,
        created_at: i64,
    ) -> Result<TimekeeperUtxo, sqlx::Error> {
        let row = sqlx::query(
            "INSERT INTO timekeeper_supply_utxos (txid, vout, amount, created_at) \
             VALUES ($1, $2, $3, $4) \
             RETURNING *",
        )
        .bind(txid)
        .bind(vout)
        .bind(amount)
        .bind(created_at)
        .fetch_one(&self.pool)
        .await?;

        TimekeeperUtxo::from_row(&row)
    }

    pub async fn spend_timekeeper_supply_utxo(
        &self,
        txid: &str,
        vout: i32,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "UPDATE timekeeper_supply_utxos SET spent = TRUE WHERE txid = $1 AND vout = $2",
        )
        .bind(txid)
        .bind(vout)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}

impl DbState {
    pub async fn insert_timekeeper_tick_utxo(
        &self,
        txid: &str,
        vout: i32,
        amount: i64,
        created_at: i64,
    ) -> Result<TimekeeperUtxo, sqlx::Error> {
        let row = sqlx::query(
            "INSERT INTO timekeeper_tick_utxos (txid, vout, amount, created_at) \
             VALUES ($1, $2, $3, $4) \
             RETURNING *",
        )
        .bind(txid)
        .bind(vout)
        .bind(amount)
        .bind(created_at)
        .fetch_one(&self.pool)
        .await?;

        TimekeeperUtxo::from_row(&row)
    }

    pub async fn get_expired_timekeeper_tick_utxos(
        &self,
        max_age_seconds: i64,
    ) -> Result<Vec<TimekeeperUtxo>, sqlx::Error> {
        let cutoff = crate::timekeeper::now_unix() - max_age_seconds;
        let rows = sqlx::query(
            "SELECT * FROM timekeeper_tick_utxos \
             WHERE spent = FALSE AND created_at <= $1 \
             ORDER BY created_at ASC",
        )
        .bind(cutoff)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(TimekeeperUtxo::from_row).collect()
    }

    pub async fn mark_timekeeper_tick_utxos_spent(&self, ids: &[i32]) -> Result<(), sqlx::Error> {
        if ids.is_empty() {
            return Ok(());
        }
        sqlx::query("UPDATE timekeeper_tick_utxos SET spent = TRUE WHERE id = ANY($1)")
            .bind(ids)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}
