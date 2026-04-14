use axum::{Json, extract::Path, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::json;

use super::state::DbState;

pub async fn get_price_feed(State(db): State<DbState>, Path(id): Path<u32>) -> impl IntoResponse {
    match db.get_signed_price_feed(id).await {
        Ok(Some(feed)) => (
            StatusCode::OK,
            Json(json!({
                "id": feed.id,
                "feed_type": feed.feed_type.to_string(),
                "description": feed.description,
                "price": feed.price,
                "timestamp": feed.timestamp,
                "valid_until": feed.valid_until,
                "signature": hex::encode(&feed.signature),
            })),
        )
            .into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "price feed not found" })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
