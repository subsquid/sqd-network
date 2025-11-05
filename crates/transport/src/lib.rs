// sqd-network-transport, the transport layer for the SQD Network.
// Copyright (C) 2024 Subsquid Labs GmbH

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::fmt::{Display, Formatter};

use libp2p::{swarm::DialError, TransportError};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub use libp2p::{
    identity::{Keypair, ParseError as IdParseError, PublicKey},
    request_response::ResponseChannel,
    Multiaddr, PeerId,
};

#[cfg(feature = "metrics")]
pub use libp2p::metrics::{Metrics, Recorder};
#[cfg(feature = "metrics")]
pub use prometheus_client::registry::Registry;

#[cfg(feature = "actors")]
mod actors;
#[cfg(feature = "behaviour")]
mod behaviour;
#[cfg(feature = "actors")]
mod builder;
mod cli;
#[cfg(feature = "proto")]
mod codec;
#[cfg(feature = "metrics")]
pub mod metrics;
pub mod protocol;
pub mod util;

#[cfg(feature = "logs-collector")]
pub use crate::actors::logs_collector::{
    LogsCollectorBehaviour, LogsCollectorConfig, LogsCollectorEvent, LogsCollectorTransport,
};
#[cfg(feature = "observer")]
pub use crate::actors::observer::{
    ObserverBehaviour, ObserverConfig, ObserverEvent, ObserverTransportHandle,
};
#[cfg(feature = "pings-collector")]
pub use crate::actors::pings_collector::{
    Heartbeat, PingsCollectorBehaviour, PingsCollectorConfig, PingsCollectorTransportHandle,
};
#[cfg(feature = "portal")]
pub use crate::actors::portal::{
    PortalBehaviour, PortalConfig, PortalTransport, PortalTransportHandle, QueryFailure,
};
#[cfg(feature = "portal-logs-collector")]
pub use crate::actors::portal_logs_collector::{
    PortalLogsCollectorBehaviour, PortalLogsCollectorConfig, PortalLogsCollectorEvent,
    PortalLogsCollectorTransportHandle,
};
#[cfg(feature = "worker")]
pub use crate::actors::worker::{
    WorkerBehaviour, WorkerConfig, WorkerEvent, WorkerTransportHandle,
};
#[cfg(feature = "actors")]
pub use behaviour::base::BaseConfig;
#[cfg(feature = "pubsub")]
pub use behaviour::pubsub::{PubsubBehaviour, PubsubMsg};
#[cfg(feature = "actors")]
pub use behaviour::stream_client::{ClientConfig, Timeout as StreamClientTimeout};
#[cfg(feature = "behaviour")]
pub use behaviour::{
    addr_cache::AddressCache,
    keep_alive::KeepAliveBehaviour,
    node_whitelist::{WhitelistBehavior, WhitelistConfig},
    wrapped::{BehaviourWrapper, Wrapped},
};
#[cfg(feature = "actors")]
pub use builder::P2PTransportBuilder;
pub use cli::{BootNode, TransportArgs};
use util::parse_env_var;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransportConfig {
    /// Maximum transmission unit to use during MTU discovery (default: 1452).
    pub mtu_discovery_max: u16,
    /// Interval for sending keep-alive packets in milliseconds (default: 5000).
    pub keep_alive_interval_ms: u32,
    /// Timeout after which connections are closed if they were not receiving keep-alive packets, in milliseconds (default: 60000).
    pub max_idle_timeout_ms: u32,
    /// Timeout after which connections are closed if they are not used by any protocol, in milliseconds (default: 10000).
    pub idle_connection_timeout_ms: u32,
}

impl TransportConfig {
    pub fn from_env() -> Self {
        let mtu_discovery_max = parse_env_var("MTU_DISCOVERY_MAX", 1452);
        let keep_alive_interval_ms = parse_env_var("KEEP_ALIVE_INTERVAL_MS", 5000);
        let max_idle_timeout_ms = parse_env_var("MAX_IDLE_TIMEOUT_MS", 60000);
        let idle_connection_timeout_ms = parse_env_var("IDLE_CONNECTION_TIMEOUT_MS", 120000);
        Self {
            mtu_discovery_max,
            keep_alive_interval_ms,
            max_idle_timeout_ms,
            idle_connection_timeout_ms,
        }
    }
}

#[derive(thiserror::Error, Debug)]
#[error("Queue full")]
pub struct QueueFull;

impl<T> From<mpsc::error::TrySendError<T>> for QueueFull {
    fn from(_: mpsc::error::TrySendError<T>) -> Self {
        Self // FIXME: `Closed` variant should not be converted to `QueueFull`
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
    #[error("{0}")]
    Contract(#[from] sqd_contract_client::ClientError),
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::Transport(e.to_string())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AgentInfo {
    pub name: &'static str,
    pub version: &'static str,
}

impl Display for AgentInfo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.name, self.version)
    }
}

#[macro_export]
macro_rules! get_agent_info {
    () => {
        AgentInfo {
            name: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        }
    };
}

#[cfg(feature = "behaviour")]
#[cfg(feature = "metrics")]
pub(crate) fn record_event<T>(event: &T)
where
    Metrics: Recorder<T>,
{
    if let Some(metrics) = metrics::LIBP2P_METRICS.get() {
        metrics.record(event);
    }
}

#[cfg(feature = "behaviour")]
#[cfg(not(feature = "metrics"))]
pub(crate) fn record_event<T>(_event: T) {}
