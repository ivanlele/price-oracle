use alloy::contract::Error as ContractError;
use alloy::primitives::Address;
use alloy::providers::{ProviderBuilder, RootProvider};
use alloy::sol;

sol! {
    #[sol(rpc)]
    interface EACAggregatorProxy {
        function latestAnswer() external view returns (int256);
        function latestRoundData() external view returns (uint80 roundId, int256 answer, uint256 startedAt, uint256 updatedAt, uint80 answeredInRound);
        function description() external view returns (string memory);
        function decimals() external view returns (uint8);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FeedContractError {
    #[error("invalid RPC URL: {0}")]
    InvalidUrl(String),
    #[error("invalid address: {0}")]
    InvalidAddress(String),
    #[error("contract call failed: {0}")]
    ContractCall(#[from] ContractError),
    #[error("value overflow: {0}")]
    ValueOverflow(String),
}

pub struct FeedContract {
    contract: EACAggregatorProxy::EACAggregatorProxyInstance<RootProvider>,
}

#[allow(unused)]
pub struct LatestRoundData {
    pub round_id: u128,
    pub answer: i128,
    pub started_at: u64,
    pub updated_at: u64,
    pub answered_in_round: u128,
}

impl FeedContract {
    pub fn new(address: &str, rpc_url: &str) -> Result<Self, FeedContractError> {
        let address: Address = address
            .parse()
            .map_err(|e| FeedContractError::InvalidAddress(format!("{e}")))?;
        let url: reqwest::Url = rpc_url
            .parse()
            .map_err(|e| FeedContractError::InvalidUrl(format!("{e}")))?;
        let provider = ProviderBuilder::new()
            .disable_recommended_fillers()
            .connect_http(url);
        let contract = EACAggregatorProxy::new(address, provider);
        Ok(Self { contract })
    }

    #[allow(unused)]
    pub async fn latest_answer(&self) -> Result<i128, FeedContractError> {
        let answer = self.contract.latestAnswer().call().await?;
        answer
            .try_into()
            .map_err(|e| FeedContractError::ValueOverflow(format!("{e}")))
    }

    pub async fn latest_round_data(&self) -> Result<LatestRoundData, FeedContractError> {
        let EACAggregatorProxy::latestRoundDataReturn {
            roundId,
            answer,
            startedAt,
            updatedAt,
            answeredInRound,
        } = self.contract.latestRoundData().call().await?;

        let answer = answer
            .try_into()
            .map_err(|e| FeedContractError::ValueOverflow(format!("{e}")))?;

        Ok(LatestRoundData {
            round_id: roundId.to::<u128>(),
            answer,
            started_at: startedAt.to::<u64>(),
            updated_at: updatedAt.to::<u64>(),
            answered_in_round: answeredInRound.to::<u128>(),
        })
    }

    pub async fn description(&self) -> Result<String, FeedContractError> {
        let desc = self.contract.description().call().await?;
        Ok(desc)
    }

    pub async fn decimals(&self) -> Result<u8, FeedContractError> {
        let dec = self.contract.decimals().call().await?;
        Ok(dec)
    }
}
