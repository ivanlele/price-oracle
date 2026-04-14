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

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Start { config } => {
            if let Err(e) = start(config).await {
                error!("{}", e);
            }
        }
    }
}

async fn start(config_path: PathBuf) -> Result<(), String> {
    let config =
        Config::from_file(config_path).map_err(|e| format!("Failed to load config: {}", e))?;

    let routes = handlers::routes(&config)
        .await
        .map_err(|e| format!("Failed to initialize routes: {}", e))?;

    let app = Router::new().merge(routes);

    let bind_addr = format!("0.0.0.0:{}", config.service.port);
    info!("Starting service on {}...", bind_addr);

    let listener = TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| format!("Failed to bind to {}: {}", bind_addr, e))?;

    axum::serve(listener, app)
        .await
        .map_err(|e| format!("Server error: {}", e))?;

    Ok(())
}
