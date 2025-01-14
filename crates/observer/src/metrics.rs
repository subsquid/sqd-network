use std::{sync::atomic::AtomicU64, time::Duration};

use lazy_static::lazy_static;
use libp2p::metrics::Metrics;
use prometheus_client::{
    metrics::{counter::Counter, family::Family, gauge::Gauge},
    registry::Registry,
};
use tokio::sync::OnceCell;

type Labels = Vec<(&'static str, String)>;

lazy_static! {
    pub static ref GOSSIPSUB_RECEIVED: Family::<Labels, Counter> = Family::default();
    pub static ref LAST_SEEN: Family::<Labels, Gauge> = Family::default();
    pub static ref MISSING_CHUNKS: Family::<Labels, Gauge> = Family::default();
    pub static ref PINGS_TOTAL: Family<Labels, Counter> = Family::default();
    pub static ref LAST_PING_TIME: Family<Labels, Gauge<f64, AtomicU64>> = Family::default();
}

pub static LIBP2P_METRICS: OnceCell<Metrics> = OnceCell::const_new();

pub fn record_message(topic: &str, peer_id: &str) {
    GOSSIPSUB_RECEIVED
        .get_or_create(&vec![("topic", topic.to_owned()), ("peer_id", peer_id.to_owned())])
        .inc();
}

pub fn peer_seen(peer_id: &str, addr: &str) {
    LAST_SEEN
        .get_or_create(&vec![("peer_id", peer_id.to_owned()), ("addr", addr.to_owned())])
        .set(now());
}

pub fn worker_heartbeat(peer_id: &str, missing_chunks: u64) {
    MISSING_CHUNKS
        .get_or_create(&vec![("peer_id", peer_id.to_owned())])
        .set(missing_chunks as i64);
}

pub fn ping(peer_id: &str, duration: Duration) {
    let labels = vec![("peer_id", peer_id.to_owned())];
    PINGS_TOTAL.get_or_create(&labels).inc();
    LAST_PING_TIME.get_or_create(&labels).set(duration.as_secs_f64());
}

pub fn ping_failed(peer_id: &str) {
    LAST_PING_TIME.remove(&vec![("peer_id", peer_id.to_owned())]);
}

pub fn register_metrics(registry: &mut Registry) {
    assert!(
        LIBP2P_METRICS.set(Metrics::new(registry)).is_ok(),
        "Metrics already initialized"
    );
    registry.register(
        "gossipsub_received",
        "The counter for the received gossipsub messages",
        GOSSIPSUB_RECEIVED.clone(),
    );
    registry.register(
        "last_seen",
        "The timestamp of the last message from the given peer",
        LAST_SEEN.clone(),
    );
    registry.register(
        "worker_missing_chunks",
        "The number of chunks missing from the worker",
        MISSING_CHUNKS.clone(),
    );
    registry.register("pings", "The number of pings sent to the worker", PINGS_TOTAL.clone());
    registry.register_with_unit(
        "last_ping",
        "The duration of the last ping",
        prometheus_client::registry::Unit::Seconds,
        LAST_PING_TIME.clone(),
    );
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}
