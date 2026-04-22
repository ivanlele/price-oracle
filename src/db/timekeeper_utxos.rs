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

    pub async fn get_all_unspent_timekeeper_tick_utxos(
        &self,
    ) -> Result<Vec<TimekeeperUtxo>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM timekeeper_tick_utxos \
             WHERE spent = FALSE \
             ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(TimekeeperUtxo::from_row).collect()
    }

    pub async fn get_all_timekeeper_tick_utxos(&self) -> Result<Vec<TimekeeperUtxo>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM timekeeper_tick_utxos \
             ORDER BY created_at ASC",
        )
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(TimekeeperUtxo::from_row).collect()
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
        self.set_timekeeper_tick_utxo_spent_state(ids, true).await
    }

    pub async fn mark_timekeeper_tick_utxos_unspent(&self, ids: &[i32]) -> Result<(), sqlx::Error> {
        self.set_timekeeper_tick_utxo_spent_state(ids, false).await
    }

    async fn set_timekeeper_tick_utxo_spent_state(
        &self,
        ids: &[i32],
        spent: bool,
    ) -> Result<(), sqlx::Error> {
        if ids.is_empty() {
            return Ok(());
        }
        sqlx::query("UPDATE timekeeper_tick_utxos SET spent = $2 WHERE id = ANY($1)")
            .bind(ids)
            .bind(spent)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn get_unspent_timekeeper_tick_utxos(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<TimekeeperUtxo>, sqlx::Error> {
        let rows = sqlx::query(
            "SELECT * FROM timekeeper_tick_utxos \
             WHERE spent = FALSE \
             ORDER BY created_at DESC \
             LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?;

        rows.iter().map(TimekeeperUtxo::from_row).collect()
    }

    pub async fn count_unspent_timekeeper_tick_utxos(&self) -> Result<i64, sqlx::Error> {
        let row =
            sqlx::query("SELECT COUNT(*) as count FROM timekeeper_tick_utxos WHERE spent = FALSE")
                .fetch_one(&self.pool)
                .await?;

        row.try_get("count")
    }
}
