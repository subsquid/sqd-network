use ethers::{
    contract::{ContractError, MulticallError},
    prelude::{AbiError, Middleware},
};
use libp2p::identity::ParseError;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("Invalid RPC URL: {0:?}")]
    InvalidRpcUrl(#[from] url::ParseError),
    #[error("Websocket client error: {0}")]
    WsClient(#[from] ethers::providers::WsClientError),
    #[error("Invalid Peer ID: {0:?}")]
    InvalidPeerId(#[from] ParseError),
    #[error("Contract error: {0}")]
    Contract(String),
    #[error("RPC provider error: {0}")]
    Provider(#[from] ethers::providers::ProviderError),
    #[error("Unsupported RPC protocol")]
    InvalidProtocol,
    #[error("Transaction receipt missing")]
    TxReceiptMissing,
    #[error("Block not found")]
    BlockNotFound,
}

impl<M: Middleware> From<ContractError<M>> for ClientError {
    fn from(err: ContractError<M>) -> Self {
        let decoded = match err {
            ContractError::Revert(ethers::types::Bytes(ref bs)) => {
                decode_error(bs).unwrap_or_else(|| err.to_string())
            }
            _ => err.to_string(),
        };
        Self::Contract(decoded)
    }
}

impl<M: Middleware> From<MulticallError<M>> for ClientError {
    fn from(err: MulticallError<M>) -> Self {
        Self::Contract(err.to_string())
    }
}

impl From<AbiError> for ClientError {
    fn from(err: AbiError) -> Self {
        Self::Contract(err.to_string())
    }
}

impl From<serde_json::Error> for ClientError {
    fn from(err: serde_json::Error) -> Self {
        Self::Contract(err.to_string())
    }
}

impl From<std::io::Error> for ClientError {
    fn from(err: std::io::Error) -> Self {
        Self::Contract(err.to_string())
    }
}

fn decode_error(msg: &[u8]) -> Option<String> {
    if msg.len() < 64 {
        return None;
    }

    // check function selector
    if let Ok(funsel) = <[u8; 4]>::try_from(&msg[..4]) {
        if u32::from_be_bytes(funsel) != 0x08c379a0 {
            return None;
        }
    } else {
        return None;
    }

    // check offset
    if let Ok(offset) = <[u8; 8]>::try_from(&msg[28..36]) {
        if u64::from_be_bytes(offset) != 32 {
            return None;
        }
    } else {
        return None;
    }

    Some(String::from_utf8_lossy(&msg[36..]).to_string())
}
