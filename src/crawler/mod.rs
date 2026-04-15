pub mod cross_rate_converter;
pub mod feeds_contract;

use std::time::Duration;

use elements::secp256k1_zkp::{
    Keypair, Message,
    hashes::{Hash, sha256},
};
use hex_literal::hex;
use log::{error, info};

use crate::config::FeedCrawlerConfig;
use crate::db::price_feed_listings::PriceFeedListing;
use crate::db::signed_price_feeds::{FeedType, SignedPriceFeed};
use crate::handlers::state::{DbState, SignerState};

use cross_rate_converter::{CrossRateConverter, build_contract_pairs};
use feeds_contract::{FeedContract, FeedContractError};

const FEED_MESSAGE_SUFFIX: [u8; 44] = hex!(
    "7d17e21ff2908408473658adab09a690ede3e6d74112222f79737296447475c9031e7388931bb03890c1e79c"
);

#[derive(Debug, thiserror::Error)]
pub enum CrawlerError {
    #[error("feed contract error: {0}")]
    FeedContract(#[from] FeedContractError),
    #[error("cross rate error: {0}")]
    CrossRate(#[from] cross_rate_converter::CrossRateError),
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
}

enum FeedSource {
    /// Single pair — index into the shared contracts pool.
    Exchange { contract_index: usize },
    /// Cross rate — converter that references the shared contracts pool.
    Cross { converter: CrossRateConverter },
}

struct FeedEntry {
    id: u32,
    description: String,
    feed_type: FeedType,
    source: FeedSource,
}

pub struct Crawler {
    contracts: Vec<FeedContract>,
    feeds: Vec<FeedEntry>,
    signer: SignerState,
    db: DbState,
    interval_seconds: u64,
    validity_seconds: u64,
}

fn resolve_feed_source(
    feed_desc: &str,
    pairs: &[cross_rate_converter::ContractPair],
) -> Result<(FeedType, FeedSource), CrawlerError> {
    let hops: Vec<&str> = feed_desc.split('/').collect();
    if hops.len() != 2 {
        let converter = CrossRateConverter::new(feed_desc, pairs)?;
        return Ok((FeedType::CrossRate, FeedSource::Cross { converter }));
    }

    // Simple exchange rate — find the matching contract.
    let from = hops[0].trim().to_uppercase();
    let to = hops[1].trim().to_uppercase();
    let idx = pairs
        .iter()
        .position(|p| (p.base == from && p.quote == to) || (p.base == to && p.quote == from))
        .ok_or_else(|| cross_rate_converter::CrossRateError::MissingPair {
            from: from.clone(),
            to: to.clone(),
        })?;
    Ok((
        FeedType::ExchangeRate,
        FeedSource::Exchange {
            contract_index: idx,
        },
    ))
}

impl Crawler {
    pub async fn new(
        config: &FeedCrawlerConfig,
        signer: SignerState,
        db: DbState,
    ) -> Result<Self, CrawlerError> {
        // Build the shared contract pool from all addresses.
        let contracts: Result<Vec<_>, _> = config
            .addresses
            .iter()
            .map(|addr| FeedContract::new(addr, &config.rpc_url))
            .collect();
        let contracts = contracts?;

        // Query on-chain descriptions and decimals once.
        let pairs = build_contract_pairs(&contracts).await?;

        let mut feeds = Vec::new();
        for (i, feed_desc) in config.feeds.iter().enumerate() {
            let (feed_type, source) = resolve_feed_source(feed_desc, &pairs)?;
            feeds.push(FeedEntry {
                id: i as u32,
                description: feed_desc.clone(),
                feed_type,
                source,
            });
        }

        Ok(Self {
            contracts,
            feeds,
            signer,
            db,
            interval_seconds: config.interval_seconds,
            validity_seconds: config.validity_seconds,
        })
    }

    async fn init_feeds(&self) -> Result<(), CrawlerError> {
        let listings = self.db.get_price_feed_listings(i64::MAX, 0).await?;

        for feed in &self.feeds {
            let already_listed = listings.iter().any(|l| l.description == feed.description);
            if !already_listed {
                let listing = PriceFeedListing {
                    id: feed.id,
                    description: feed.description.clone(),
                };
                self.db.insert_price_feed_listing(&listing).await?;
                info!("Created price feed listing: {}", feed.description);
            }

            if self.db.get_signed_price_feed(feed.id).await?.is_some() {
                continue;
            }
            info!(
                "Feed '{}' not yet in signed_price_feeds, will be created on first crawl",
                feed.description
            );
        }
        Ok(())
    }

    async fn crawl(&self) -> Result<(), CrawlerError> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let timestamp = now as u32;
        let valid_until = (now + self.validity_seconds) as u32;
        let keypair = Keypair::from_secret_key(&self.signer.secp, &self.signer.secret_key);

        for feed in &self.feeds {
            let price = match &feed.source {
                FeedSource::Exchange { contract_index } => {
                    let round_data = self.contracts[*contract_index].latest_round_data().await?;
                    round_data.answer as u64
                }
                FeedSource::Cross { converter } => converter.convert(&self.contracts).await?,
            };

            // Build message: id(4) || price(8) || timestamp(4) || valid_until(4) || suffix(44) = 64 bytes
            let mut message_data = [0u8; 64];
            message_data[0..4].copy_from_slice(&feed.id.to_be_bytes());
            message_data[4..12].copy_from_slice(&price.to_be_bytes());
            message_data[12..16].copy_from_slice(&timestamp.to_be_bytes());
            message_data[16..20].copy_from_slice(&valid_until.to_be_bytes());
            message_data[20..64].copy_from_slice(&FEED_MESSAGE_SUFFIX);

            let hash = sha256::Hash::hash(&message_data);
            let message = Message::from_digest(hash.to_byte_array());
            let signature = self
                .signer
                .secp
                .sign_schnorr_no_aux_rand(&message, &keypair);

            let signed_feed = SignedPriceFeed {
                id: feed.id,
                feed_type: feed.feed_type.clone(),
                description: feed.description.clone(),
                price,
                timestamp,
                valid_until,
                signature: signature.as_ref().to_vec(),
            };

            self.db.insert_signed_price_feed(&signed_feed).await?;

            info!(
                "Crawled feed '{}': price={}, timestamp={}",
                feed.description, price, timestamp
            );
        }

        Ok(())
    }

    pub async fn start(self) -> Result<(), CrawlerError> {
        self.init_feeds().await?;

        self.crawl().await?;
        info!("Initial feed crawl completed, starting scheduled crawling");

        let interval_seconds = self.interval_seconds;
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(interval_seconds)).await;
                info!("Running scheduled feed crawl");
                if let Err(e) = self.crawl().await {
                    error!("Feed crawl failed: {e}");
                }
            }
        });

        Ok(())
    }
}
