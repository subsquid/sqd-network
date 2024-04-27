use libp2p::{noise, swarm::DialError, TransportError};

pub use libp2p::{
    identity::{Keypair, ParseError as IdParseError, PublicKey},
    Multiaddr, PeerId,
};
use tokio::sync::mpsc;

#[cfg(feature = "metrics")]
pub use prometheus_client::registry::Registry;

mod actors;
mod behaviour;
mod builder;
mod cli;
mod codec;
#[cfg(feature = "metrics")]
pub mod metrics;
pub mod protocol;
pub mod util;

pub use actors::{
    gateway::{GatewayConfig, GatewayEvent, GatewayTransportHandle},
    logs_collector::{LogsCollectorConfig, LogsCollectorEvent, LogsCollectorTransportHandle},
    scheduler::{SchedulerConfig, SchedulerEvent, SchedulerTransportHandle},
    worker::{WorkerConfig, WorkerEvent, WorkerTransportHandle},
};
pub use builder::{P2PTransportBuilder, QuicConfig};
pub use cli::{BootNode, TransportArgs};

#[derive(thiserror::Error, Debug)]
#[error("Queue full")]
pub struct QueueFull;

impl<T> From<mpsc::error::TrySendError<T>> for QueueFull {
    fn from(_: mpsc::error::TrySendError<T>) -> Self {
        Self
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Libp2p transport creation failed: {0}")]
    Transport(String),
    #[error("Listening failed: {0:?}")]
    Listen(#[from] TransportError<std::io::Error>),
    #[error("Dialing failed: {0:?}")]
    Dial(#[from] DialError),
}

impl From<noise::Error> for Error {
    fn from(e: noise::Error) -> Self {
        Self::Transport(e.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Transport(e.to_string())
    }
}
