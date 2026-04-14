mod cli;
mod config;
mod crawler;
mod db;
mod handlers;

use cli::{Cli, Commands};
use config::Config;

use std::path::PathBuf;

use axum::Router;
use clap::Parser;
use log::{error, info};
use tokio::net::TcpListener;
use tower_http::cors::CorsLayer;

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

    let app_state = handlers::state::AppState::from_config(&config).await?;

    let crawler = crawler::Crawler::new(
        &config.service.feed_crawler,
        app_state.signer.clone(),
        app_state.db.clone(),
    )
    .await
    .map_err(|e| format!("Failed to initialize crawler: {}", e))?;

    crawler
        .start()
        .await
        .map_err(|e| format!("Crawler failed: {}", e))?;

    let app = Router::new()
        .merge(handlers::routes(app_state))
        .layer(CorsLayer::permissive());

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
