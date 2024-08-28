use crate::ClientError;
use ethers::{
    prelude::{Http, JsonRpcClient, JsonRpcError, Provider, ProviderError, RpcError, Ws},
    utils::__serde_json::Error,
};
use libp2p::futures::TryFutureExt;
use serde::{de::DeserializeOwned, Serialize};
use std::{
    fmt::{Debug, Display, Formatter},
    future::Future,
    pin::Pin,
    sync::Arc,
};
use url::Url;

#[derive(Debug, Clone)]
pub enum Transport {
    Http(Http),
    Ws(Ws),
}

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    Http(#[from] ethers::providers::HttpClientError),
    Ws(#[from] ethers::providers::WsClientError),
}

impl Display for TransportError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Http(e) => Display::fmt(e, f),
            Self::Ws(e) => Display::fmt(e, f),
        }
    }
}

impl RpcError for TransportError {
    fn as_error_response(&self) -> Option<&JsonRpcError> {
        match self {
            Self::Http(e) => e.as_error_response(),
            Self::Ws(e) => e.as_error_response(),
        }
    }

    fn as_serde_error(&self) -> Option<&Error> {
        match self {
            Self::Http(e) => e.as_serde_error(),
            Self::Ws(e) => e.as_serde_error(),
        }
    }
}

impl From<TransportError> for ProviderError {
    fn from(err: TransportError) -> Self {
        match err {
            TransportError::Http(e) => e.into(),
            TransportError::Ws(e) => e.into(),
        }
    }
}

impl Transport {
    pub async fn connect(rpc_url: &str) -> Result<Arc<Provider<Self>>, ClientError> {
        let transport = if rpc_url.starts_with("http") {
            Self::Http(Http::new(Url::parse(rpc_url)?))
        } else if rpc_url.starts_with("ws") {
            Self::Ws(Ws::connect(rpc_url).await?)
        } else {
            return Err(ClientError::InvalidProtocol);
        };
        Ok(Arc::new(Provider::new(transport)))
    }
}

impl JsonRpcClient for Transport {
    type Error = TransportError;

    fn request<'life0, 'life1, 'async_trait, T, R>(
        &'life0 self,
        method: &'life1 str,
        params: T,
    ) -> Pin<Box<dyn Future<Output = Result<R, Self::Error>> + Send + 'async_trait>>
    where
        T: Debug + Serialize + Send + Sync + 'async_trait,
        R: DeserializeOwned + Send + 'async_trait,
        'life0: 'async_trait,
        'life1: 'async_trait,
        Self: 'async_trait,
    {
        log::debug!("Calling method {method} with params {params:?}");
        match self {
            Self::Http(provider) => Box::pin(provider.request(method, params).map_err(Into::into)),
            Self::Ws(provider) => Box::pin(provider.request(method, params).map_err(Into::into)),
        }
    }
}
