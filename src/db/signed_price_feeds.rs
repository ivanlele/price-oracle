use std::fmt;

use sqlx::Row;

use crate::handlers::state::DbState;

#[derive(Debug, Clone, PartialEq)]
pub enum FeedType {
    ExchangeRate,
    CrossRate,
}

impl fmt::Display for FeedType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeedType::ExchangeRate => write!(f, "exchange_rate"),
            FeedType::CrossRate => write!(f, "cross_rate"),
        }
    }
}

impl FeedType {
    pub fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "exchange_rate" => Ok(FeedType::ExchangeRate),
            "cross_rate" => Ok(FeedType::CrossRate),
            other => Err(format!("unknown feed type: {}", other)),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SignedPriceFeed {
    pub id: u32,
    pub feed_type: FeedType,
    pub description: String,
    pub price: u64,
    pub timestamp: u32,
    pub valid_until: u32,
    pub signature: Vec<u8>,
}

impl SignedPriceFeed {
    fn from_row(row: &sqlx::postgres::PgRow) -> Result<Self, sqlx::Error> {
        let feed_type_str: String = row.try_get("feed_type")?;
        let feed_type =
            FeedType::from_str(&feed_type_str).map_err(|e| sqlx::Error::Decode(e.into()))?;

        Ok(Self {
            id: row.try_get::<i64, _>("id")? as u32,
            feed_type,
            description: row.try_get("description")?,
            price: row.try_get::<i64, _>("price")? as u64,
            timestamp: row.try_get::<i64, _>("timestamp")? as u32,
            valid_until: row.try_get::<i64, _>("valid_until")? as u32,
            signature: row.try_get("signature")?,
        })
    }
}

#[allow(unused)]
impl DbState {
    pub async fn insert_signed_price_feed(
        &self,
        feed: &SignedPriceFeed,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO signed_price_feeds (id, feed_type, description, price, timestamp, valid_until, signature)
             VALUES ($1, $2, $3, $4, $5, $6, $7)",
        )
        .bind(feed.id as i64)
        .bind(feed.feed_type.to_string())
        .bind(&feed.description)
        .bind(feed.price as i64)
        .bind(feed.timestamp as i64)
        .bind(feed.valid_until as i64)
        .bind(&feed.signature)
        .execute(&self.pool)
        .await?;

        Ok(())
    }

    pub async fn get_signed_price_feed(
        &self,
        id: u32,
    ) -> Result<Option<SignedPriceFeed>, sqlx::Error> {
        sqlx::query(
            "SELECT id, feed_type, description, price, timestamp, valid_until, signature
             FROM signed_price_feeds WHERE id = $1",
        )
        .bind(id as i64)
        .fetch_optional(&self.pool)
        .await?
        .map(|row| SignedPriceFeed::from_row(&row))
        .transpose()
    }

    pub async fn get_all_signed_price_feeds(&self) -> Result<Vec<SignedPriceFeed>, sqlx::Error> {
        sqlx::query(
            "SELECT id, feed_type, description, price, timestamp, valid_until, signature
             FROM signed_price_feeds",
        )
        .fetch_all(&self.pool)
        .await?
        .iter()
        .map(SignedPriceFeed::from_row)
        .collect()
    }

    pub async fn update_signed_price_feed(
        &self,
        feed: &SignedPriceFeed,
    ) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE signed_price_feeds
             SET feed_type = $2, description = $3, price = $4, timestamp = $5, valid_until = $6, signature = $7
             WHERE id = $1",
        )
        .bind(feed.id as i64)
        .bind(feed.feed_type.to_string())
        .bind(&feed.description)
        .bind(feed.price as i64)
        .bind(feed.timestamp as i64)
        .bind(feed.valid_until as i64)
        .bind(&feed.signature)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    pub async fn delete_signed_price_feed(&self, id: u32) -> Result<bool, sqlx::Error> {
        let result = sqlx::query("DELETE FROM signed_price_feeds WHERE id = $1")
            .bind(id as i64)
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}
