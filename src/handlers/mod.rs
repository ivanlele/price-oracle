use std::sync::Arc;

use elements::secp256k1_zkp::{All, Secp256k1, SecretKey};

use axum::{Json, Router, response::IntoResponse, routing::get};

use serde_json::json;

async fn version() -> impl IntoResponse {
    Json(json!({
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

pub fn routes(signer_state: SignerState) -> Router {
    Router::new()
        .route("/simplicity-unchained/version", get(version))
        .with_state(signer_state)
}

#[derive(Clone, Debug)]
pub struct SignerState {
    pub secret_key: SecretKey,
    pub secp: Arc<Secp256k1<All>>,
}

impl SignerState {
    pub fn new(secret_key_hex: &str) -> Result<Self, String> {
        let secret_key_bytes =
            hex::decode(secret_key_hex).map_err(|e| format!("Invalid private key hex: {}", e))?;

        let secret_key = SecretKey::from_slice(&secret_key_bytes)
            .map_err(|e| format!("Invalid private key: {}", e))?;

        Ok(Self {
            secret_key,
            secp: Arc::new(Secp256k1::new()),
        })
    }
}
