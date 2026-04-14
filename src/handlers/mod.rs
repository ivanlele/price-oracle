pub mod state;

use axum::{Json, Router, response::IntoResponse, routing::get};
use serde_json::json;

use crate::config::Config;
use state::AppState;

async fn version() -> impl IntoResponse {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

pub async fn routes(config: &Config) -> Result<Router, String> {
    let app_state = AppState::from_config(config).await?;

    let router = Router::new()
        .route("/price-oracle/version", get(version))
        .with_state(app_state);

    Ok(router)
}
