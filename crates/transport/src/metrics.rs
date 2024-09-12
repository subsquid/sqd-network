use std::sync::atomic::{AtomicU32, AtomicU64};

use lazy_static::lazy_static;
use libp2p::metrics::Metrics;
use prometheus_client::{
    metrics::{counter::Counter, family::Family, gauge::Gauge},
    registry::Registry,
};
use tokio::sync::OnceCell;

lazy_static! {
    pub static ref ACTIVE_CONNECTIONS: Gauge<u32, AtomicU32> = Default::default();
    pub static ref ONGOING_PROBES: Gauge<u32, AtomicU32> = Default::default();
    pub static ref ONGOING_QUERIES: Gauge<u32, AtomicU32> = Default::default();
    pub static ref QUEUE_SIZE: Family<Vec<(&'static str, &'static str)>, Gauge<u32, AtomicU32>> =
        Default::default();
    pub static ref DROPPED: Family<Vec<(&'static str, &'static str)>, Counter<u64, AtomicU64>> =
        Default::default();
    pub static ref DISCARDED_MESSAGES: Counter = Default::default();
    pub static ref PINGS_PUBLISHED: Counter = Default::default();
    pub static ref PINGS_RECEIVED: Counter = Default::default();
    pub static ref PONGS_SENT: Counter = Default::default();
    pub static ref PONGS_RECEIVED: Counter = Default::default();
    pub static ref WORKER_LOGS_PUBLISHED: Counter = Default::default();
    pub static ref WORKER_LOGS_RECEIVED: Counter = Default::default();
    pub static ref LOGS_COLLECTED_PUBLISHED: Counter = Default::default();
    pub static ref LOGS_COLLECTED_RECEIVED: Counter = Default::default();
}

pub static LIBP2P_METRICS: OnceCell<Metrics> = OnceCell::const_new();

pub fn register_metrics(registry: &mut Registry) {
    assert!(
        LIBP2P_METRICS.set(Metrics::new(registry)).is_ok(),
        "Metrics already initialized"
    );
    registry.register(
        "active_connections",
        "The number of active p2p connections (both incoming and outgoing)",
        ACTIVE_CONNECTIONS.clone(),
    );
    registry.register(
        "ongoing_probes",
        "The number of ongoing peer reachability probes",
        ONGOING_PROBES.clone(),
    );
    registry.register(
        "ongoing_queries",
        "The number of ongoing kademlia DHT queries",
        ONGOING_QUERIES.clone(),
    );
    registry.register(
        "queue_size",
        "The number of messages/events waiting to be processed",
        QUEUE_SIZE.clone(),
    );
    registry.register(
        "discarded_messages",
        "Gossipsub messages discarded because they were deprecated",
        DISCARDED_MESSAGES.clone(),
    );
    registry.register("dropped", "The number of dropped messages/events", DROPPED.clone());
    registry.register(
        "pings_published",
        "The number of published ping messages",
        PINGS_PUBLISHED.clone(),
    );
    registry.register(
        "pings_received",
        "The number of received ping messages",
        PINGS_RECEIVED.clone(),
    );
    registry.register("pongs_sent", "The number of sent pong messages", PONGS_SENT.clone());
    registry.register(
        "pongs_received",
        "The number of received pong messages",
        PONGS_RECEIVED.clone(),
    );
    registry.register(
        "worker_logs_published",
        "The number of published worker logs messages",
        WORKER_LOGS_PUBLISHED.clone(),
    );
    registry.register(
        "worker_logs_received",
        "The number of received worker logs messages",
        WORKER_LOGS_RECEIVED.clone(),
    );
    registry.register(
        "logs_collected_published",
        "The number of published logs collected messages",
        LOGS_COLLECTED_PUBLISHED.clone(),
    );
    registry.register(
        "logs_collected_received",
        "The number of received logs collected messages",
        LOGS_COLLECTED_RECEIVED.clone(),
    );
}
