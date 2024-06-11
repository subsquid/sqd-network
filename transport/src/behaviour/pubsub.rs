use std::{
    collections::HashMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use derivative::Derivative;
use libp2p::gossipsub::PublishError;
use libp2p::{
    gossipsub,
    gossipsub::{MessageAcceptance, MessageAuthenticity, Sha256Topic, TopicHash},
    identity::Keypair,
    swarm::{NetworkBehaviour, ToSwarm},
};
use tokio::time::Instant;

use crate::{
    behaviour::wrapped::{BehaviourWrapper, TToSwarm},
    record_event, PeerId,
};

const SUBSCRIPTION_TIMEOUT: Duration = Duration::from_secs(60);

struct TopicState {
    name: &'static str,
    topic: Sha256Topic,
    sequence_numbers: HashMap<PeerId, u64>, // FIXME: Potential memory leak
    keep_last: u64,
    subscribed_at: Instant,
}

impl TopicState {
    pub fn new(name: &'static str, keep_last: u64) -> Self {
        Self {
            name,
            topic: Sha256Topic::new(name),
            sequence_numbers: Default::default(),
            keep_last,
            subscribed_at: Instant::now(),
        }
    }
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
}

impl PubsubBehaviour {
    pub fn new(keypair: Keypair, max_msg_size: usize) -> Self {
        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .validate_messages()
            .message_id_fn(msg_id)
            .max_transmit_size(max_msg_size)
            .build()
            .expect("config should be valid");
        let inner =
            gossipsub::Behaviour::new(MessageAuthenticity::Signed(keypair), gossipsub_config)
                .expect("config should be valid");
        Self {
            inner,
            topics: Default::default(),
        }
    }

    pub fn subscribe(&mut self, topic_name: &'static str, keep_last: u64) {
        log::info!("Subscribing to topic {topic_name}");
        let topic = TopicState::new(topic_name, keep_last);
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
        let Some(topic) = self.topics.get(&topic_hash) else {
            return log::error!("Cannot publish to unsubscribed topic: {topic_name}");
        };

        match self.inner.publish(topic_hash, msg) {
            Err(PublishError::InsufficientPeers)
                if topic.subscribed_at.elapsed() <= SUBSCRIPTION_TIMEOUT =>
            {
                log::info!("Waiting for peers to publish to {topic_name}")
            }
            Err(e) => log::error!("Error publishing message to {topic_name}: {e:?}"),
            Ok(_) => log::debug!("Message published to {topic_name}"),
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
        let Some(peer_id) = msg.source else {
            return Err("anonymous message");
        };
        let Some(topic_state) = self.topics.get_mut(&msg.topic) else {
            return Err("message with unknown topic");
        };
        let last_seq_no = topic_state.sequence_numbers.entry(peer_id).or_default();
        match msg.sequence_number {
            None => return Err("message without sequence number"),
            // Sequence numbers should be timestamp-based, can't be from the future
            Some(seq_no) if seq_no > timestamp_now() => return Err("invalid sequence number"),
            Some(seq_no) if seq_no + topic_state.keep_last <= *last_seq_no => {
                return Err("old message")
            }
            Some(seq_no) if seq_no > *last_seq_no => *last_seq_no = seq_no,
            _ => {}
        };

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
        record_event(&ev);
        let gossipsub::Event::Message {
            message,
            propagation_source,
            message_id,
        } = ev
        else {
            return None;
        };

        match self.validate_gossipsub_msg(message) {
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
        .as_nanos()
        .try_into()
        .expect("not that far in the future")
}
