mod cli;
mod config;
mod handlers;

use cli::{Cli, Commands};
use config::Config;

use std::path::PathBuf;

use axum::Router;
use clap::Parser;
use log::{error, info};
use tokio::net::TcpListener;

use crate::handlers::SignerState;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start { config } => {
            start(config).await;
        }
    }
}

async fn start(config_path: PathBuf) {
    let config = match Config::from_file(config_path) {
        Ok(config) => config,
        Err(e) => {
            error!("Failed to load config: {}", e);
            return;
        }
    };

    // Initialize signer state from config
    let signer_state = match SignerState::new(&config.service.private_key) {
        Ok(state) => state,
        Err(e) => {
            error!("Failed to initialize signer state: {}", e);
            return;
        }
    };

    let app = Router::new().merge(handlers::routes(signer_state));

    let bind_addr = format!("0.0.0.0:{}", config.service.port);
    info!("Starting service on {}...", bind_addr);

    let listener = match TcpListener::bind(&bind_addr).await {
        Ok(l) => l,
        Err(e) => {
            error!("Failed to bind to {}: {}", bind_addr, e);
            return;
        }
    };

    if let Err(e) = axum::serve(listener, app).await {
        error!("Server error: {}", e);
    }
}
