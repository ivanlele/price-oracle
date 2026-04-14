use super::feeds_contract::{FeedContract, FeedContractError};

#[derive(Debug, thiserror::Error)]
pub enum CrossRateError {
    #[error("contract error: {0}")]
    Contract(#[from] FeedContractError),
    #[error("no contract provides pair {from}/{to}")]
    MissingPair { from: String, to: String },
    #[error("invalid target path: {0}")]
    InvalidPath(String),
    #[error("division by zero in conversion step")]
    DivisionByZero,
}

/// A resolved step in the conversion path.
#[derive(Debug)]
struct ConversionStep {
    contract_index: usize,
    /// Precomputed 10^decimals for this step's contract.
    scale: u128,
    /// Whether to divide (invert) rather than multiply.
    invert: bool,
}

/// Parsed pair from a contract description like "BTC / USD".
#[derive(Debug)]
pub struct ContractPair {
    pub base: String,
    pub quote: String,
    pub decimals: u8,
}

pub struct CrossRateConverter {
    steps: Vec<ConversionStep>,
    /// Which contract indices are actually needed (deduplicated).
    needed_contracts: Vec<usize>,
}

/// Parse a contract description like "BTC / USD" into (base, quote).
fn parse_contract_description(desc: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = desc.split('/').map(|s| s.trim()).collect();
    if parts.len() == 2 {
        Some((parts[0].to_uppercase(), parts[1].to_uppercase()))
    } else {
        None
    }
}

/// Query all contracts for their on-chain description and decimals.
pub async fn build_contract_pairs(
    contracts: &[FeedContract],
) -> Result<Vec<ContractPair>, CrossRateError> {
    let mut pairs = Vec::with_capacity(contracts.len());
    for contract in contracts {
        let desc = contract.description().await?;
        let decimals = contract.decimals().await?;
        let (base, quote) = parse_contract_description(&desc).ok_or_else(|| {
            CrossRateError::InvalidPath(format!("cannot parse contract description: {desc}"))
        })?;
        pairs.push(ContractPair {
            base,
            quote,
            decimals,
        });
    }
    Ok(pairs)
}

/// Parse a config target path like "BTC/USD/USDT" into currency hops.
fn parse_target_path(description: &str) -> Vec<String> {
    description
        .split('/')
        .map(|s| s.trim().to_uppercase())
        .collect()
}

impl CrossRateConverter {
    /// Build a converter from the feed description and the shared contract pool.
    ///
    /// Resolves the conversion path by matching hops to available pairs.
    /// Precomputes scales and tracks which contracts are needed.
    ///
    /// For target path `BTC/USD/USDT` with contracts:
    /// - Contract 0: "BTC / USD" (8 decimals)
    /// - Contract 1: "USDT / USD" (8 decimals)
    ///
    /// Step 1: BTC -> USD — contract 0 is BTC/USD -> Multiply
    /// Step 2: USD -> USDT — contract 1 is USDT/USD (inverted) -> Divide
    pub fn new(target_description: &str, pairs: &[ContractPair]) -> Result<Self, CrossRateError> {
        let hops = parse_target_path(target_description);
        if hops.len() < 2 {
            return Err(CrossRateError::InvalidPath(target_description.to_string()));
        }

        let mut steps = Vec::with_capacity(hops.len() - 1);
        let mut needed_contracts = Vec::new();

        for window in hops.windows(2) {
            let from = &window[0];
            let to = &window[1];

            let (idx, invert, decimals) = pairs
                .iter()
                .enumerate()
                .find_map(|(idx, pair)| {
                    if pair.base == *from && pair.quote == *to {
                        Some((idx, false, pair.decimals))
                    } else if pair.base == *to && pair.quote == *from {
                        Some((idx, true, pair.decimals))
                    } else {
                        None
                    }
                })
                .ok_or_else(|| CrossRateError::MissingPair {
                    from: from.clone(),
                    to: to.clone(),
                })?;

            if !needed_contracts.contains(&idx) {
                needed_contracts.push(idx);
            }

            steps.push(ConversionStep {
                contract_index: idx,
                scale: 10u128.pow(decimals as u32),
                invert,
            });
        }

        Ok(Self {
            steps,
            needed_contracts,
        })
    }

    /// Compute the cross rate. Only fetches prices from contracts this
    /// converter actually needs, not the entire pool.
    pub async fn convert(&self, contracts: &[FeedContract]) -> Result<u64, CrossRateError> {
        // Fetch only the contracts we need.
        let mut prices = vec![0u64; contracts.len()];
        for &idx in &self.needed_contracts {
            let round_data = contracts[idx].latest_round_data().await?;
            prices[idx] = round_data.answer as u64;
        }

        // Start at the first step's scale (representing 1.0 in fixed-point),
        // so every step — including the first — uses the same multiply/divide logic.
        let mut result = self.steps[0].scale;

        for step in &self.steps {
            let price = prices[step.contract_index] as u128;
            if price == 0 && step.invert {
                return Err(CrossRateError::DivisionByZero);
            }
            if step.invert {
                result = result * step.scale / price;
                continue;
            }
            result = result * price / step.scale;
        }

        Ok(result as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_contract_description() {
        let (base, quote) = parse_contract_description("BTC / USD").unwrap();
        assert_eq!(base, "BTC");
        assert_eq!(quote, "USD");
    }

    #[test]
    fn test_parse_target_path() {
        let path = parse_target_path("BTC/USD/USDT");
        assert_eq!(path, vec!["BTC", "USD", "USDT"]);
    }

    #[test]
    fn test_parse_target_path_simple() {
        let path = parse_target_path("BTC/USD");
        assert_eq!(path, vec!["BTC", "USD"]);
    }
}
