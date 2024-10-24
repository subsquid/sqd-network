use std::time::Duration;

use libp2p::StreamProtocol;

use sqd_contract_client::Network;

pub const OLD_PING_TOPIC: &str = "/subsquid/worker_pings/1.0.0";
pub const HEARTBEAT_TOPIC: &str = "/sqd/worker_heartbeats/1.1.0";
pub const PORTAL_LOGS_TOPIC: &str = "/sqd/portal_logs/1.1.0";

pub const ID_PROTOCOL: &str = "/subsquid/1.0.0";
pub const QUERY_PROTOCOL: &str = "/sqd/query/1.1.0";
pub const WORKER_LOGS_PROTOCOL: &str = "/sqd/worker_logs/1.1.0";
pub const PONG_PROTOCOL: &str = "/subsquid/pong/1.0.0";

pub const MAX_QUERY_SIZE: u64 = 512 * 1024;
pub const MAX_QUERY_RESULT_SIZE: u64 = 100 * 1024 * 1024;
pub const MAX_LOGS_REQUEST_SIZE: u64 = 100;
pub const MAX_LOGS_RESPONSE_SIZE: u64 = 10 * 1024 * 1024;
pub const MAX_PONG_SIZE: u64 = 10 * 1024 * 1024;
pub const MAX_PUBSUB_MSG_SIZE: usize = 65536;

pub const HEARTBEATS_MIN_INTERVAL: Duration = Duration::from_secs(20);
pub const APPROX_EPOCH_LEN: Duration = Duration::from_secs(1200);

pub const fn dht_protocol(network: Network) -> StreamProtocol {
    match network {
        Network::Tethys => StreamProtocol::new("/subsquid/dht/tethys/1.0.0"),
        Network::Mainnet => StreamProtocol::new("/subsquid/dht/mainnet/1.0.0"),
    }
}
