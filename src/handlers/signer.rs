use axum::{Json, extract::State, response::IntoResponse};
use elements::secp256k1_zkp::PublicKey;
use serde_json::json;

use super::state::SignerState;

pub async fn get_public_key(State(signer): State<SignerState>) -> impl IntoResponse {
    let public_key = PublicKey::from_secret_key(&signer.secp, &signer.secret_key);
    Json(json!({
        "public_key": public_key.to_string(),
    }))
}
