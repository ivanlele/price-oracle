use std::sync::Arc;

use crate::elements::secp256k1_zkp::{All, Secp256k1, SecretKey};
use axum::extract::FromRef;
use sqlx::PgPool;
use sqlx::postgres::PgPoolOptions;

use crate::config::{Config, DbConfig, SignerConfig};

#[derive(Clone, Debug)]
pub struct SignerState {
    pub secret_key: SecretKey,
    pub secp: Arc<Secp256k1<All>>,
}

impl SignerState {
    pub fn from_config(config: &SignerConfig) -> Result<Self, String> {
        let secret_key_bytes = hex::decode(&config.private_key)
            .map_err(|e| format!("Invalid private key hex: {}", e))?;

        let secret_key = SecretKey::from_slice(&secret_key_bytes)
            .map_err(|e| format!("Invalid private key: {}", e))?;

        Ok(Self {
            secret_key,
            secp: Arc::new(Secp256k1::new()),
        })
    }
}

#[derive(Clone, Debug)]
pub struct DbState {
    pub pool: PgPool,
}

impl DbState {
    pub async fn from_config(config: &DbConfig) -> Result<Self, String> {
        let connection_string = format!(
            "postgres://{}:{}@{}/{}",
            config.username, config.password, config.url, config.database
        );

        let pool = PgPoolOptions::new()
            .max_connections(config.max_connections)
            .connect(&connection_string)
            .await
            .map_err(|e| format!("Failed to connect to database: {}", e))?;

        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| format!("Failed to run migrations: {}", e))?;

        Ok(Self { pool })
    }
}

#[derive(Clone, Debug)]
pub struct AppState {
    pub signer: SignerState,
    pub db: DbState,
    pub timekeeper_info: TimekeeperInfo,
}

impl FromRef<AppState> for SignerState {
    fn from_ref(state: &AppState) -> Self {
        state.signer.clone()
    }
}

impl FromRef<AppState> for DbState {
    fn from_ref(state: &AppState) -> Self {
        state.db.clone()
    }
}

#[derive(Clone, Debug)]
pub struct TimekeeperInfo {
    pub issuer_spk_hex: String,
}

impl FromRef<AppState> for TimekeeperInfo {
    fn from_ref(state: &AppState) -> Self {
        state.timekeeper_info.clone()
    }
}

impl AppState {
    pub async fn from_config(config: &Config) -> Result<Self, String> {
        let signer = SignerState::from_config(&config.service.signer)?;
        let db = DbState::from_config(&config.service.db).await?;

        let timekeeper_info = {
            let tk_signer = config.service.timekeeper.signer();
            let issuer_spk = tk_signer.get_address().script_pubkey();
            TimekeeperInfo {
                issuer_spk_hex: hex::encode(issuer_spk.as_bytes()),
            }
        };

        Ok(Self {
            signer,
            db,
            timekeeper_info,
        })
    }
}
