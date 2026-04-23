use sqlx::Row;

use crate::handlers::state::DbState;

#[derive(Debug, Clone)]
pub struct TimekeeperMonitorState {
    pub last_scanned_height: i64,
    pub last_scanned_hash: String,
}

impl DbState {
    pub async fn get_timekeeper_monitor_state(
        &self,
    ) -> Result<Option<TimekeeperMonitorState>, sqlx::Error> {
        let row = sqlx::query(
            "SELECT last_scanned_height, last_scanned_hash FROM timekeeper_monitor_state WHERE id = 1",
        )
        .fetch_optional(&self.pool)
        .await?;

        row.map(|row| {
            Ok(TimekeeperMonitorState {
                last_scanned_height: row.try_get("last_scanned_height")?,
                last_scanned_hash: row.try_get("last_scanned_hash")?,
            })
        })
        .transpose()
    }

    pub async fn upsert_timekeeper_monitor_state(
        &self,
        last_scanned_height: i64,
        last_scanned_hash: &str,
        updated_at: i64,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO timekeeper_monitor_state (id, last_scanned_height, last_scanned_hash, updated_at) \
             VALUES (1, $1, $2, $3) \
             ON CONFLICT (id) DO UPDATE SET \
                 last_scanned_height = EXCLUDED.last_scanned_height, \
                 last_scanned_hash = EXCLUDED.last_scanned_hash, \
                 updated_at = EXCLUDED.updated_at",
        )
        .bind(last_scanned_height)
        .bind(last_scanned_hash)
        .bind(updated_at)
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}
