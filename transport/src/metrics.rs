use lazy_static::lazy_static;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;
use std::sync::atomic::AtomicU32;

lazy_static! {
    pub static ref INBOUND_MSG_QUEUE_SIZE: Gauge<u32, AtomicU32> = Default::default();
    pub static ref OUTBOUND_MSG_QUEUE_SIZE: Gauge<u32, AtomicU32> = Default::default();
    pub static ref DIAL_QUEUE_SIZE: Gauge<u32, AtomicU32> = Default::default();
    pub static ref PENDING_DIALS: Gauge<u32, AtomicU32> = Default::default();
    pub static ref ONGOING_DIALS: Gauge<u32, AtomicU32> = Default::default();
    pub static ref ONGOING_QUERIES: Gauge<u32, AtomicU32> = Default::default();
    pub static ref PENDING_MESSAGES: Gauge<u32, AtomicU32> = Default::default();
    pub static ref SUBSCRIBED_TOPICS: Gauge<u32, AtomicU32> = Default::default();
    pub static ref ACTIVE_CONNECTIONS: Gauge<u32, AtomicU32> = Default::default();
}

pub(crate) fn register_metrics(registry: &mut Registry) {
    registry.register(
        "inbound_msg_queue_size",
        "The number of inbound messages waiting in queue to be processed",
        INBOUND_MSG_QUEUE_SIZE.clone(),
    );
    registry.register(
        "outbound_msg_queue_size",
        "The number of outbound messages waiting in queue to be sent",
        OUTBOUND_MSG_QUEUE_SIZE.clone(),
    );
    registry.register(
        "dial_queue_size",
        "The number of dial requests waiting in queue to be dispatched",
        DIAL_QUEUE_SIZE.clone(),
    );
    registry.register(
        "pending_dials",
        "The number of dials which are pending because peer address is unknown",
        PENDING_DIALS.clone(),
    );
    registry.register(
        "ongoing_dials",
        "The number of ongoing dial attempts",
        ONGOING_DIALS.clone(),
    );
    registry.register(
        "pending_messages",
        "The number of outbound messages which are pending because peer address is unknown",
        PENDING_MESSAGES.clone(),
    );
    registry.register(
        "subscribed_topic",
        "The number of gossipsub topics the node is subscribed to",
        SUBSCRIBED_TOPICS.clone(),
    );
    registry.register(
        "active_connections",
        "The number of active p2p connections (both incoming and outgoing)",
        ACTIVE_CONNECTIONS.clone(),
    );
}
