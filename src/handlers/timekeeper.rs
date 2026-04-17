use axum::{Json, extract::Query, extract::State, http::StatusCode, response::IntoResponse};
use serde::Deserialize;
use serde_json::json;
use utoipa::IntoParams;
use validator::Validate;

use super::state::DbState;

#[derive(Debug, Deserialize, Validate, IntoParams)]
pub struct PaginationParams {
    #[serde(default = "default_limit")]
    #[validate(range(min = 1, max = 100, message = "limit must be between 1 and 100"))]
    pub limit: u32,
    #[serde(default)]
    pub offset: u32,
}

fn default_limit() -> u32 {
    20
}

#[utoipa::path(
    get,
    path = "/price-oracle/timekeeper/ticks",
    params(PaginationParams),
    responses(
        (status = 200, description = "Paginated list of unspent tick UTXOs", body = super::TimekeeperTickListResponse),
        (status = 400, description = "Invalid query parameters", body = super::ErrorResponse),
        (status = 500, description = "Internal server error", body = super::ErrorResponse),
    ),
    tag = "Timekeeper"
)]
pub async fn list_tick_utxos(
    State(db): State<DbState>,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    if let Err(errors) = params.validate() {
        let error_msg = errors.to_string();
        return (StatusCode::BAD_REQUEST, Json(json!({ "error": error_msg }))).into_response();
    }

    let limit = params.limit as i64;
    let offset = params.offset as i64;

    let total = match db.count_unspent_timekeeper_tick_utxos().await {
        Ok(count) => count,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    match db.get_unspent_timekeeper_tick_utxos(limit, offset).await {
        Ok(utxos) => {
            let items: Vec<_> = utxos
                .iter()
                .map(|u| {
                    json!({
                        "txid": u.txid,
                        "vout": u.vout,
                        "amount": u.amount,
                        "created_at": u.created_at,
                    })
                })
                .collect();

            Json(json!({
                "items": items,
                "total": total,
                "limit": limit,
                "offset": offset,
            }))
            .into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
