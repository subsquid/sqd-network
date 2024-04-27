use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

use derivative::Derivative;
use libp2p::{
    gossipsub,
    gossipsub::{MessageAcceptance, MessageAuthenticity, Sha256Topic, TopicHash},
    identity::Keypair,
    swarm::{NetworkBehaviour, ToSwarm},
};

use crate::{
    behaviour::wrapped::{BehaviourWrapper, TToSwarm},
    PeerId,
};

#[cfg(feature = "metrics")]
use libp2p::metrics::{Metrics, Recorder};
#[cfg(feature = "metrics")]
use prometheus_client::registry::Registry;

struct TopicState {
    name: &'static str,
    topic: Sha256Topic,
    msg_ordering: MsgOrdering,
}

impl TopicState {
    pub fn new(name: &'static str, allow_unordered: bool) -> Self {
        Self {
            name,
            topic: Sha256Topic::new(name),
            msg_ordering: match allow_unordered {
                true => MsgOrdering::Unordered,
                false => MsgOrdering::Ordered {
                    sequence_numbers: Default::default(),
                },
            },
        }
    }
}

enum MsgOrdering {
    Unordered,
    Ordered {
        sequence_numbers: HashMap<PeerId, u64>, // FIXME: Potential memory leak
    },
}

#[derive(Derivative, Clone)]
#[derivative(Debug)]
pub struct PubsubMsg {
    pub peer_id: PeerId,
    pub topic: &'static str,
    #[derivative(Debug = "ignore")]
    pub data: Box<[u8]>,
}

pub struct PubsubBehaviour {
    inner: gossipsub::Behaviour,
    topics: HashMap<TopicHash, TopicState>,
    #[cfg(feature = "metrics")]
    metrics: Metrics,
}

impl PubsubBehaviour {
    pub fn new(keypair: Keypair, #[cfg(feature = "metrics")] registry: &mut Registry) -> Self {
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .validate_messages()
            .message_id_fn(msg_id)
            .build()
            .expect("config should be valid");
        let inner =
            gossipsub::Behaviour::new(MessageAuthenticity::Signed(keypair), gossipsub_config)
                .expect("config should be valid");
        Self {
            inner,
            topics: Default::default(),
            #[cfg(feature = "metrics")]
            metrics: Metrics::new(registry),
        }
    }

    pub fn subscribe(&mut self, topic_name: &'static str, allow_unordered: bool) {
        log::info!("Subscribing to topic {topic_name}");
        let topic = TopicState::new(topic_name, allow_unordered);
        let topic_hash = topic.topic.hash();
        if self.topics.contains_key(&topic_hash) {
            log::warn!("Topic {topic_name} already subscribed");
            return;
        }
        if let Err(e) = self.inner.subscribe(&topic.topic) {
            log::error!("Cannot subscribe to {topic_name}: {e:}");
            return;
        }
        self.topics.insert(topic_hash, topic);
        log::info!("Topic {topic_name} subscribed");
    }

    pub fn publish(&mut self, topic_name: &'static str, msg: impl Into<Vec<u8>>) {
        log::debug!("Publishing message to topic {topic_name}");
        let topic_hash = Sha256Topic::new(topic_name).hash();
        if let Err(e) = self.inner.publish(topic_hash, msg) {
            log::error!("Error publishing message to {topic_name}: {e:?}");
        }
    }

    /// Validate gossipsub message
    ///   1) Check if message is not anonymous,
    ///   2) Check if topic is known (subscribed),
    ///   3) Enforce message ordering (if configured for topic).
    fn validate_gossipsub_msg(
        &mut self,
        msg: gossipsub::Message,
    ) -> Result<PubsubMsg, &'static str> {
        let peer_id = match msg.source {
            Some(peer_id) => peer_id,
            None => return Err("anonymous message"),
        };
        let topic_state = match self.topics.get_mut(&msg.topic) {
            Some(x) => x,
            None => return Err("message with unknown topic"),
        };
        if let MsgOrdering::Ordered { sequence_numbers } = &mut topic_state.msg_ordering {
            let last_seq_no = sequence_numbers.get(&peer_id).copied().unwrap_or_default();
            match msg.sequence_number {
                None => return Err("message without sequence number"),
                // Sequence numbers should be timestamp-based, can't be from the future
                Some(seq_no) if seq_no > timestamp_now() => return Err("invalid sequence number"),
                Some(seq_no) if seq_no <= last_seq_no => return Err("old message"),
                Some(seq_no) => sequence_numbers.insert(peer_id, seq_no),
            };
        }

        Ok(PubsubMsg {
            peer_id,
            topic: topic_state.name,
            data: msg.data.into_boxed_slice(),
        })
    }
}

impl BehaviourWrapper for PubsubBehaviour {
    type Inner = gossipsub::Behaviour;
    type Event = PubsubMsg;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_inner_event(
        &mut self,
        ev: <Self::Inner as NetworkBehaviour>::ToSwarm,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        log::debug!("Gossipsub event received: {ev:?}");
        #[cfg(feature = "metrics")]
        self.metrics.record(&ev);
        let (msg, propagation_source, message_id) = match ev {
            gossipsub::Event::Message {
                message,
                propagation_source,
                message_id,
            } => (message, propagation_source, message_id),
            _ => return None,
        };

        match self.validate_gossipsub_msg(msg) {
            Ok(msg) => {
                let _ = self.inner.report_message_validation_result(
                    &message_id,
                    &propagation_source,
                    MessageAcceptance::Accept,
                );
                Some(ToSwarm::GenerateEvent(msg))
            }
            Err(e) => {
                log::debug!("Discarding gossipsub message from {propagation_source}: {e}");
                let _ = self.inner.report_message_validation_result(
                    &message_id,
                    &propagation_source,
                    MessageAcceptance::Reject,
                );
                None
            }
        }
    }
}

// Default gossipsub msg ID function, copied from libp2p
fn msg_id(msg: &gossipsub::Message) -> gossipsub::MessageId {
    let mut source_string = if let Some(peer_id) = msg.source.as_ref() {
        peer_id.to_base58()
    } else {
        PeerId::from_bytes(&[0, 1, 0]).expect("Valid peer id").to_base58()
    };
    source_string.push_str(&msg.sequence_number.unwrap_or_default().to_string());
    gossipsub::MessageId::from(source_string)
}

#[inline(always)]
fn timestamp_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("we're after 1970")
        .as_nanos() as u64
}
