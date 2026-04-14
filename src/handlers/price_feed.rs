use axum::{Json, extract::Path, extract::State, http::StatusCode, response::IntoResponse};
use serde_json::json;

use super::state::DbState;

#[utoipa::path(
    get,
    path = "/price-oracle/feed/{id}",
    params(
        ("id" = u32, Path, description = "Price feed ID")
    ),
    responses(
        (status = 200, description = "Price feed found", body = super::PriceFeedResponse),
        (status = 404, description = "Price feed not found", body = super::ErrorResponse),
        (status = 500, description = "Internal server error", body = super::ErrorResponse),
    ),
    tag = "Price Feeds"
)]
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
