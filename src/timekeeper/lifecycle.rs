use std::sync::Arc;
use std::time::Duration;

use crate::config::TimekeeperConfig;
use crate::handlers::state::DbState;

use super::{Timekeeper, monitor, utils};

impl Timekeeper {
    /// Spawn the timekeeper as a background task.
    pub fn start(config: TimekeeperConfig, db: DbState) {
        tokio::spawn(async move {
            let tk = match Self::init(&config, db).await {
                Ok(tk) => tk,
                Err(e) => {
                    tracing::error!("Timekeeper init failed: {e}");
                    return;
                }
            };

            tk.run().await;
        });
    }

    async fn run(self) {
        tracing::info!("Starting timekeeper with asset {}", self.asset_id);

        let tick_interval = self.publish_interval_seconds;
        let return_interval = self.return_to_issuer_interval_seconds;

        let this = Arc::new(self);
        let this_tick = this.clone();
        let this_return = this.clone();
        let this_monitor = this.clone();

        tokio::spawn(async move {
            loop {
                if let Err(e) = this_monitor.scan_new_blocks_for_tick_spends().await {
                    tracing::error!("Timekeeper block monitor failed: {e}");
                }

                tokio::time::sleep(Duration::from_secs(monitor::BLOCK_MONITOR_INTERVAL_SECONDS))
                    .await;
            }
        });

        tokio::spawn(async move {
            loop {
                match this_tick.tick().await {
                    Ok(Some(_)) => {}
                    Ok(None) => {}
                    Err(e) => tracing::error!("Timestamp tick failed: {e}"),
                }

                tokio::time::sleep(Duration::from_secs(tick_interval)).await;
            }
        });

        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(return_interval)).await;

                match this_return.return_expired_ticks().await {
                    Ok(_) => {}
                    Err(e) => tracing::error!("Return-to-issuer failed: {e}"),
                }
            }
        });
    }

    pub(crate) async fn wait_for_finalization(
        &self,
        txid: &simplex::simplicityhl::elements::Txid,
    ) -> Result<(), super::Error> {
        utils::poll_for_confirmation(self, txid).await
    }
}
