pub mod price_feed;
pub mod price_feed_listings;
pub mod signer;
pub mod state;
pub mod timekeeper;

use axum::{Json, Router, response::IntoResponse, routing::get};
use serde::Serialize;
use serde_json::json;
use utoipa::{OpenApi, ToSchema};

use state::AppState;

// -- Response schemas for OpenAPI docs --

#[allow(dead_code)]
#[derive(Serialize, ToSchema)]
pub struct VersionResponse {
    pub version: String,
}

#[allow(dead_code)]
#[derive(Serialize, ToSchema)]
pub struct ErrorResponse {
    pub error: String,
}

#[allow(dead_code)]
#[derive(Serialize, ToSchema)]
pub struct PriceFeedResponse {
    pub id: u32,
    pub feed_type: String,
    pub description: String,
    pub price: u64,
    pub timestamp: u32,
    pub valid_until: u32,
    pub signature: String,
}

#[allow(dead_code)]
#[derive(Serialize, ToSchema)]
pub struct PriceFeedListItem {
    pub id: u32,
    pub description: String,
}

#[allow(dead_code)]
#[derive(Serialize, ToSchema)]
pub struct PriceFeedListResponse {
    pub items: Vec<PriceFeedListItem>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[allow(dead_code)]
#[derive(Serialize, ToSchema)]
pub struct PublicKeyResponse {
    pub public_key: String,
}

#[allow(dead_code)]
#[derive(Serialize, ToSchema)]
pub struct TimekeeperTickItem {
    pub txid: String,
    pub vout: i32,
    pub amount: i64,
    pub created_at: i64,
}

#[allow(dead_code)]
#[derive(Serialize, ToSchema)]
pub struct TimekeeperTickListResponse {
    pub items: Vec<TimekeeperTickItem>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

#[allow(dead_code)]
#[derive(Serialize, ToSchema)]
pub struct IssuerSpkResponse {
    pub issuer_spk: String,
}

// -- OpenAPI definition --

#[allow(dead_code)]
#[derive(OpenApi)]
#[openapi(
    servers(
        (url = "http://localhost:30445", description = "Local development server"),
    ),
    paths(
        version,
        price_feed::get_price_feed,
        price_feed_listings::list_price_feeds,
        signer::get_public_key,
        timekeeper::list_tick_utxos,
        timekeeper::get_issuer_spk,
    ),
    components(schemas(
        VersionResponse,
        ErrorResponse,
        PriceFeedResponse,
        PriceFeedListItem,
        PriceFeedListResponse,
        PublicKeyResponse,
        TimekeeperTickItem,
        TimekeeperTickListResponse,
        IssuerSpkResponse,
    )),
    tags(
        (name = "General", description = "General service endpoints"),
        (name = "Price Feeds", description = "Price feed data"),
        (name = "Signer", description = "Signer information"),
        (name = "Timekeeper", description = "Timekeeper tick UTXOs"),
    )
)]
pub struct ApiDoc;

#[utoipa::path(
    get,
    path = "/price-oracle/version",
    responses(
        (status = 200, description = "Service version", body = VersionResponse),
    ),
    tag = "General"
)]
async fn version() -> impl IntoResponse {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

pub fn routes(app_state: AppState) -> Router {
    Router::new()
        .route("/price-oracle/version", get(version))
        .route("/price-oracle/feed/{id}", get(price_feed::get_price_feed))
        .route(
            "/price-oracle/feeds",
            get(price_feed_listings::list_price_feeds),
        )
        .route("/price-oracle/public-key", get(signer::get_public_key))
        .route(
            "/price-oracle/timekeeper/issuer-spk",
            get(timekeeper::get_issuer_spk),
        )
        .route(
            "/price-oracle/timekeeper/ticks",
            get(timekeeper::list_tick_utxos),
        )
        .with_state(app_state)
}
