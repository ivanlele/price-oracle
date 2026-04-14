use sqlx::Row;

use crate::handlers::state::DbState;

#[derive(Debug, Clone)]
pub struct PriceFeedListing {
    pub id: u32,
    pub description: String,
}

impl PriceFeedListing {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get::<i64, _>("id")? as u32,
            description: row.try_get("description")?,
        })
    }
}

#[allow(unused)]
impl DbState {
    pub async fn insert_price_feed_listing(
        &self,
        listing: &PriceFeedListing,
    ) -> Result<(), sqlx::Error> {
        sqlx::query("INSERT INTO price_feed_listings (id, description) VALUES ($1, $2)")
            .bind(listing.id as i64)
            .bind(&listing.description)
            .execute(&self.pool)
            .await?;

        Ok(())
    }

    pub async fn get_price_feed_listings(
        &self,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<PriceFeedListing>, sqlx::Error> {
        sqlx::query(
            "SELECT id, description FROM price_feed_listings ORDER BY id LIMIT $1 OFFSET $2",
        )
        .bind(limit)
        .bind(offset)
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(PriceFeedListing::from_row)
        .collect()
    }

    pub async fn count_price_feed_listings(&self) -> Result<i64, sqlx::Error> {
        let row = sqlx::query("SELECT COUNT(*) as count FROM price_feed_listings")
            .fetch_one(&self.pool)
            .await?;

        row.try_get("count")
    }
}
