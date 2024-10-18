use std::time::Duration;

use libp2p::StreamProtocol;

use sqd_contract_client::Network;

pub const PING_TOPIC: &str = "/subsquid/worker_pings/1.0.0";

pub const ID_PROTOCOL: &str = "/subsquid/1.0.0";
pub const QUERY_PROTOCOL: &str = "/subsquid/query/1.0.0";
pub const GATEWAY_LOGS_PROTOCOL: &str = "/subsquid/gateway-logs/1.0.0";
pub const PONG_PROTOCOL: &str = "/subsquid/pong/1.0.0";

pub const MAX_QUERY_SIZE: u64 = 1024 * 1024;
pub const MAX_QUERY_RESULT_SIZE: u64 = 100 * 1024 * 1024;
pub const MAX_GATEWAY_LOG_SIZE: u64 = 1024 * 1024;
pub const MAX_PONG_SIZE: u64 = 10 * 1024 * 1024;
pub const MAX_PUBSUB_MSG_SIZE: usize = 65536;
pub const KEEP_LAST_WORKER_LOGS: u64 = 100;

pub const PINGS_MIN_INTERVAL: Duration = Duration::from_secs(20);
pub const LOGS_MIN_INTERVAL: Duration = Duration::from_secs(120);
pub const LOGS_COLLECTED_MIN_INTERVAL: Duration = Duration::from_secs(60);
pub const EPOCH_SEAL_TIMEOUT: Duration = Duration::from_secs(600);
pub const APPROX_EPOCH_LEN: Duration = Duration::from_secs(1200);

pub const fn dht_protocol(network: Network) -> StreamProtocol {
    match network {
        Network::Tethys => StreamProtocol::new("/subsquid/dht/tethys/1.0.0"),
        Network::Mainnet => StreamProtocol::new("/subsquid/dht/mainnet/1.0.0"),
    }
}
