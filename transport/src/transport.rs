use std::collections::VecDeque;
use std::sync::Arc;
use std::{
    collections::{hash_map::Entry, HashMap},
    marker::PhantomData,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use async_trait::async_trait;
use bimap::BiHashMap;
use futures::{
    stream::{Fuse, StreamExt},
    AsyncReadExt as FutAsyncRead, AsyncWriteExt,
};
#[cfg(feature = "metrics")]
use libp2p::metrics::{Metrics, Recorder, Registry};
use libp2p::{
    autonat, dcutr,
    gossipsub::{
        self, MessageAcceptance, MessageAuthenticity, PublishError, Sha256Topic, TopicHash,
    },
    identify,
    identity::Keypair,
    kad::{
        self, store::MemoryStore, GetClosestPeersError, GetClosestPeersOk, ProgressStep, QueryId,
        QueryResult,
    },
    multiaddr::Protocol,
    noise, ping,
    relay::client::Behaviour as RelayClient,
    request_response,
    request_response::ProtocolSupport,
    swarm::{
        dial_opts::{DialOpts, PeerCondition},
        ConnectionId, DialError, SwarmEvent,
    },
    yamux, Multiaddr, PeerId, Swarm, SwarmBuilder,
};
use libp2p_swarm_derive::NetworkBehaviour;
use rand::prelude::SliceRandom;
use tokio::{
    sync::{mpsc, oneshot},
    time::interval,
};
use tokio_stream::wrappers::{IntervalStream, ReceiverStream};
use tokio_util::sync::CancellationToken;

use crate::task_manager::TaskManager;
use crate::{
    cli::{BootNode, TransportArgs},
    util::{addr_is_reachable, get_keypair},
    Error, Message, MsgContent, Subscription,
};

type InboundMsgReceiver<T> = mpsc::Receiver<Message<T>>;
type OutboundMsgSender<T> = mpsc::Sender<Message<T>>;
type SubscriptionSender = mpsc::Sender<Subscription>;

pub const SUBSQUID_PROTOCOL: &str = "/subsquid/0.0.1";
const WORKER_PROTOCOL: &str = "/subsquid-worker/0.0.1";
const BOOTSTRAP_INTERVAL: Duration = Duration::from_secs(300);
const MAX_CONNS_PER_PEER: u32 = 2;

#[derive(NetworkBehaviour)]
struct Behaviour<T>
where
    T: MsgContent,
{
    identify: identify::Behaviour,
    kademlia: kad::Behaviour<MemoryStore>,
    relay: RelayClient,
    dcutr: dcutr::Behaviour,
    request: request_response::Behaviour<MessageCodec<T>>,
    gossipsub: gossipsub::Behaviour,
    ping: ping::Behaviour,
    autonat: autonat::Behaviour,
}

struct MessageCodec<T: MsgContent> {
    _phantom: PhantomData<T>,
}

impl<T: MsgContent> Default for MessageCodec<T> {
    fn default() -> Self {
        Self {
            _phantom: Default::default(),
        }
    }
}

impl<T: MsgContent> Clone for MessageCodec<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: MsgContent> Copy for MessageCodec<T> {}

#[async_trait]
impl<M: MsgContent> request_response::Codec for MessageCodec<M> {
    type Protocol = &'static str;
    type Request = M;
    type Response = ();

    async fn read_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
    ) -> std::io::Result<Self::Request>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        let mut buf = [0u8; std::mem::size_of::<usize>()];
        io.read_exact(&mut buf).await?;
        let msg_len = usize::from_be_bytes(buf);

        let mut msg = M::new(msg_len);
        io.read_exact(msg.as_mut_slice()).await?;
        log::trace!("New message decoded: {}", String::from_utf8_lossy(msg.as_slice()));
        Ok(msg)
    }

    async fn read_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        _io: &mut T,
    ) -> std::io::Result<Self::Response>
    where
        T: futures::AsyncRead + Unpin + Send,
    {
        Ok(())
    }

    async fn write_request<T>(
        &mut self,
        _protocol: &Self::Protocol,
        io: &mut T,
        req: Self::Request,
    ) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        let req = req.as_slice();
        log::trace!("New message to encode: {}", String::from_utf8_lossy(req));
        let msg_len = req.len().to_be_bytes();
        io.write_all(&msg_len).await?;
        io.write_all(req).await
    }

    async fn write_response<T>(
        &mut self,
        _protocol: &Self::Protocol,
        _io: &mut T,
        _res: Self::Response,
    ) -> std::io::Result<()>
    where
        T: futures::AsyncWrite + Unpin + Send,
    {
        Ok(())
    }
}

pub struct P2PTransportBuilder {
    keypair: Keypair,
    listen_addrs: Vec<Multiaddr>,
    public_addrs: Vec<Multiaddr>,
    boot_nodes: Vec<BootNode>,
    relay_addr: Option<Multiaddr>,
    relay: bool,
    bootstrap: bool,
    #[cfg(feature = "metrics")]
    metrics: Metrics,
}

impl Default for P2PTransportBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl P2PTransportBuilder {
    pub fn new() -> Self {
        let keypair = Keypair::generate_ed25519();
        Self::from_keypair(keypair)
    }

    pub fn from_keypair(keypair: Keypair) -> Self {
        Self {
            keypair,
            listen_addrs: vec![],
            public_addrs: vec![],
            boot_nodes: vec![],
            relay_addr: None,
            relay: false,
            bootstrap: true,
            #[cfg(feature = "metrics")]
            metrics: Metrics::new(&mut Default::default()),
        }
    }

    pub async fn from_cli(args: TransportArgs) -> anyhow::Result<Self> {
        let keypair = get_keypair(args.key).await?;
        Ok(Self {
            keypair,
            listen_addrs: vec![args.p2p_listen_addr],
            public_addrs: args.p2p_public_addrs,
            boot_nodes: args.boot_nodes,
            relay_addr: None,
            relay: false,
            bootstrap: args.bootstrap,
            #[cfg(feature = "metrics")]
            metrics: Metrics::new(&mut Default::default()),
        })
    }

    pub fn listen_on<I: IntoIterator<Item = Multiaddr>>(&mut self, addrs: I) {
        self.listen_addrs.extend(addrs)
    }

    pub fn public_addrs<I: IntoIterator<Item = Multiaddr>>(&mut self, addrs: I) {
        self.public_addrs.extend(addrs)
    }

    pub fn boot_nodes<I: IntoIterator<Item = BootNode>>(&mut self, nodes: I) {
        self.boot_nodes.extend(nodes)
    }

    pub fn relay_addr(&mut self, addr: Multiaddr) {
        self.relay_addr = Some(addr);
        self.relay = true;
    }

    pub fn relay(&mut self, relay: bool) {
        self.relay = relay;
    }

    pub fn bootstrap(&mut self, bootstrap: bool) {
        self.bootstrap = bootstrap;
    }

    #[cfg(feature = "metrics")]
    pub fn with_registry(&mut self, registry: &mut Registry) {
        self.metrics = Metrics::new(registry);
    }

    pub fn local_peer_id(&self) -> PeerId {
        self.keypair.public().to_peer_id()
    }

    pub fn keypair(&self) -> Keypair {
        self.keypair.clone()
    }

    fn build_swarm<T: MsgContent>(keypair: Keypair) -> Result<Swarm<Behaviour<T>>, Error> {
        let local_peer_id = PeerId::from(keypair.public());
        let protocol = SUBSQUID_PROTOCOL.to_string();

        let gossipsub_config = gossipsub::ConfigBuilder::default()
            .validate_messages()
            .message_id_fn(gossipsub_msg_id)
            .build()
            .expect("config should be valid");
        let behaviour = |keypair: &Keypair, relay| Behaviour {
            relay,
            identify: identify::Behaviour::new(
                identify::Config::new(protocol, keypair.public())
                    .with_interval(Duration::from_secs(60))
                    .with_push_listen_addr_updates(true),
            ),
            kademlia: kad::Behaviour::with_config(
                local_peer_id,
                MemoryStore::new(local_peer_id),
                Default::default(),
            ),
            dcutr: dcutr::Behaviour::new(local_peer_id),
            request: request_response::Behaviour::new(
                vec![(WORKER_PROTOCOL, ProtocolSupport::Full)],
                request_response::Config::default().with_request_timeout(Duration::from_secs(60)),
            ),
            gossipsub: gossipsub::Behaviour::new(
                MessageAuthenticity::Signed(keypair.clone()),
                gossipsub_config,
            )
            .unwrap(),
            ping: ping::Behaviour::new(Default::default()),
            autonat: autonat::Behaviour::new(local_peer_id, Default::default()),
        };

        // SwarmBuilder::with_tokio(transport, behaviour, local_peer_id).build()
        Ok(SwarmBuilder::with_existing_identity(keypair)
            .with_tokio()
            .with_tcp(Default::default(), noise::Config::new, yamux::Config::default)?
            .with_dns()?
            .with_relay_client(noise::Config::new, yamux::Config::default)?
            .with_behaviour(behaviour)
            .expect("infallible")
            .with_swarm_config(|c| c.with_idle_connection_timeout(Duration::from_secs(120)))
            .build())
    }

    async fn wait_for_listening<T: MsgContent>(swarm: &mut Swarm<Behaviour<T>>) {
        // There is no easy way to wait for *all* listen addresses to be ready (e.g. counting
        // events doesn't work, because 0.0.0.0 addr will generate as many events, as there are
        // available network interfaces). Assuming 1 second should be enough in most cases.
        let _ = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                match swarm.next().await.unwrap() {
                    SwarmEvent::NewListenAddr { address, .. } => {
                        log::info!("Listening on {:?}", address);
                    }
                    e => log::debug!("Unexpected swarm event: {e:?}"),
                }
            }
        })
        .await;
    }

    async fn wait_for_first_connection<T: MsgContent>(swarm: &mut Swarm<Behaviour<T>>) {
        loop {
            match swarm.next().await.unwrap() {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    log::info!("Connection established with {peer_id}");
                    break;
                }
                SwarmEvent::Behaviour(BehaviourEvent::Kademlia(_)) => {}
                e => log::debug!("Unexpected swarm event: {e:?}"),
            }
        }
    }

    async fn wait_for_identify<T: MsgContent>(swarm: &mut Swarm<Behaviour<T>>) {
        let mut received = false;
        let mut sent = false;
        while !(received && sent) {
            match swarm.next().await.unwrap() {
                SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Sent {
                    ..
                })) => {
                    sent = true;
                }
                SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received {
                    ..
                })) => {
                    received = true;
                }
                event => log::debug!("{:?}", event),
            }
        }
    }

    pub async fn run<T: MsgContent>(
        self,
    ) -> Result<(InboundMsgReceiver<T>, P2PTransportHandle<T>), Error> {
        log::info!("Local peer ID: {}", self.keypair.public().to_peer_id());
        let mut swarm = Self::build_swarm(self.keypair)?;

        // If relay node not specified explicitly, use random boot node
        let relay_addr = self.relay_addr.or_else(|| {
            self.boot_nodes
                .choose(&mut rand::thread_rng())
                .map(|node| node.address.clone().with(Protocol::P2p(node.peer_id)))
        });

        // Listen on provided addresses
        for addr in self.listen_addrs {
            swarm.listen_on(addr)?;
        }
        Self::wait_for_listening(&mut swarm).await;

        // Register public addresses
        for addr in self.public_addrs {
            swarm.add_external_address(addr);
        }

        // Connect to boot nodes
        if !self.boot_nodes.is_empty() {
            for BootNode { peer_id, address } in self.boot_nodes {
                log::info!("Connecting to boot node {peer_id} at {address}");
                swarm.behaviour_mut().kademlia.add_address(&peer_id, address.clone());
                swarm.dial(DialOpts::peer_id(peer_id).addresses(vec![address]).build())?;
            }
            Self::wait_for_first_connection(&mut swarm).await;
        }

        // Connect to relay and listen for relayed connections
        if self.relay {
            let addr = relay_addr.ok_or(Error::NoRelay)?;
            log::info!("Connecting to relay {addr}");
            swarm.dial(addr.clone())?;
            Self::wait_for_identify(&mut swarm).await;
            swarm.listen_on(addr.with(Protocol::P2pCircuit))?;
        }

        let (inbound_tx, inbound_rx) = mpsc::channel(100);
        let (outbound_tx, outbound_rx) = mpsc::channel(100);
        let (subscription_tx, subscription_rx) = mpsc::channel(100);
        let (dial_tx, dial_rx) = mpsc::channel(100);
        let transport = P2PTransport::new(
            inbound_tx,
            outbound_rx,
            subscription_rx,
            dial_rx,
            swarm,
            self.bootstrap,
            #[cfg(feature = "metrics")]
            self.metrics,
        );

        let handle = P2PTransportHandle::new(outbound_tx, subscription_tx, dial_tx, transport);
        Ok((inbound_rx, handle))
    }
}

struct DialResultSender(oneshot::Sender<bool>);

impl DialResultSender {
    pub fn send_result(self, result: bool) {
        let _ = self.0.send(result).map_err(|_| log::error!("Error sending dial result"));
    }
}

type DialSender = mpsc::Sender<(PeerId, DialResultSender)>;
type DialReceiver = mpsc::Receiver<(PeerId, DialResultSender)>;

#[derive(Clone)]
pub struct P2PTransportHandle<T: MsgContent> {
    msg_sender: OutboundMsgSender<T>,
    subscription_sender: SubscriptionSender,
    dial_sender: DialSender,
    _task_manager: Arc<TaskManager>, // This ensures that transport is stopped when the last handle is dropped
}

#[derive(thiserror::Error, Debug)]
pub enum P2PTransportError {
    #[error("{0}")]
    Send(String),
    #[error(transparent)]
    Recv(#[from] oneshot::error::RecvError),
    #[error("Operation timed out")]
    Timeout(#[from] tokio::time::error::Elapsed),
}

impl<T> From<mpsc::error::SendError<T>> for P2PTransportError {
    fn from(error: mpsc::error::SendError<T>) -> Self {
        Self::Send(error.to_string())
    }
}

impl<T: MsgContent> P2PTransportHandle<T> {
    fn new(
        msg_sender: OutboundMsgSender<T>,
        subscription_sender: SubscriptionSender,
        dial_sender: DialSender,
        transport: P2PTransport<T>,
    ) -> Self {
        let mut task_manager = TaskManager::default();
        task_manager.spawn(|c| transport.run(c));
        Self {
            msg_sender,
            subscription_sender,
            dial_sender,
            _task_manager: Arc::new(task_manager),
        }
    }

    pub async fn send_message(&self, msg: Message<T>) -> Result<(), P2PTransportError> {
        self.msg_sender.send(msg).await?;
        Ok(())
    }

    pub async fn send_direct_msg(
        &self,
        msg_content: T,
        peer_id: PeerId,
    ) -> Result<(), P2PTransportError> {
        let msg = Message {
            peer_id: Some(peer_id),
            topic: None,
            content: msg_content,
        };
        self.msg_sender.send(msg).await?;
        Ok(())
    }

    pub async fn broadcast_msg(
        &self,
        msg_content: T,
        topic: impl ToString,
    ) -> Result<(), P2PTransportError> {
        let msg = Message {
            peer_id: None,
            topic: Some(topic.to_string()),
            content: msg_content,
        };
        self.msg_sender.send(msg).await?;
        Ok(())
    }

    pub async fn subscribe(&self, topic: impl ToString) -> Result<(), P2PTransportError> {
        let subscription = Subscription {
            topic: topic.to_string(),
            subscribed: true,
            allow_unordered: false,
        };
        self.toggle_subscription(subscription).await
    }

    pub async fn toggle_subscription(
        &self,
        subscription: Subscription,
    ) -> Result<(), P2PTransportError> {
        self.subscription_sender.send(subscription).await?;
        Ok(())
    }

    pub async fn dial_peer(&self, peer_id: PeerId) -> Result<bool, P2PTransportError> {
        let (tx, rx) = oneshot::channel();
        let sender = DialResultSender(tx);
        self.dial_sender.send((peer_id, sender)).await?;
        Ok(tokio::time::timeout(Duration::from_secs(60), rx).await??)
    }
}

struct P2PTransport<T: MsgContent> {
    inbound_msg_sender: mpsc::Sender<Message<T>>,
    outbound_msg_receiver: Fuse<ReceiverStream<Message<T>>>,
    subscription_receiver: Fuse<ReceiverStream<Subscription>>,
    dial_receiver: Fuse<ReceiverStream<(PeerId, DialResultSender)>>,
    pending_dials: HashMap<PeerId, Vec<DialResultSender>>,
    ongoing_dials: HashMap<ConnectionId, DialResultSender>,
    ongoing_queries: BiHashMap<PeerId, QueryId>,
    pending_messages: HashMap<PeerId, Vec<T>>,
    subscribed_topics: HashMap<TopicHash, (String, bool)>, // hash -> (topic, allow_unordered)
    sequence_numbers: HashMap<(TopicHash, PeerId), u64>,
    active_connections: HashMap<PeerId, VecDeque<ConnectionId>>,
    swarm: Swarm<Behaviour<T>>,
    bootstrap: bool,
    #[cfg(feature = "metrics")]
    metrics: Metrics,
}

impl<T: MsgContent> P2PTransport<T> {
    pub fn new(
        inbound_msg_sender: mpsc::Sender<Message<T>>,
        outbound_msg_receiver: mpsc::Receiver<Message<T>>,
        subscription_receiver: mpsc::Receiver<Subscription>,
        dial_receiver: DialReceiver,
        swarm: Swarm<Behaviour<T>>,
        bootstrap: bool,
        #[cfg(feature = "metrics")] metrics: Metrics,
    ) -> Self {
        Self {
            inbound_msg_sender,
            outbound_msg_receiver: ReceiverStream::new(outbound_msg_receiver).fuse(),
            subscription_receiver: ReceiverStream::new(subscription_receiver).fuse(),
            dial_receiver: ReceiverStream::new(dial_receiver).fuse(),
            pending_dials: Default::default(),
            ongoing_dials: Default::default(),
            ongoing_queries: Default::default(),
            pending_messages: Default::default(),
            subscribed_topics: Default::default(),
            sequence_numbers: Default::default(),
            active_connections: Default::default(),
            swarm,
            bootstrap,
            #[cfg(feature = "metrics")]
            metrics,
        }
    }

    pub async fn run(mut self, cancel_token: CancellationToken) {
        log::info!("P2PTransport starting");
        let mut bootstrap_timer = IntervalStream::new(interval(BOOTSTRAP_INTERVAL)).fuse();
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => break,
                _ = bootstrap_timer.select_next_some() => {
                    if !self.bootstrap() {
                        break
                    }
                },
                event = self.swarm.select_next_some() => self.handle_swarm_event(event).await.unwrap_or_else(|e| {
                    log::error!("Error handling swarm event: {e}")
                }),
                msg = self.outbound_msg_receiver.select_next_some() =>
                    self.handle_outbound_msg(msg),
                subscription = self.subscription_receiver.select_next_some() =>
                    self.handle_subscription(subscription),
                (peer_id, result_sender) = self.dial_receiver.select_next_some() =>
                    self.dial_peer(peer_id, result_sender)
            }
        }
        log::info!("Shutting down P2P transport");
    }

    fn bootstrap(&mut self) -> bool {
        if self.bootstrap {
            log::debug!("Bootstrapping kademlia");
            if let Err(e) = self.swarm.behaviour_mut().kademlia.bootstrap() {
                log::error!("Cannot bootstrap kademlia: {e:?}");
                return false;
            }
        }
        true
    }

    fn can_send_msg(&mut self, peer_id: &PeerId) -> bool {
        if self.swarm.is_connected(peer_id) {
            return true;
        }
        let dial_opts = DialOpts::peer_id(*peer_id).condition(PeerCondition::Always).build();
        self.swarm.dial(dial_opts).is_ok()
    }

    fn send_msg(&mut self, peer_id: &PeerId, content: T) {
        log::debug!("Sending message to peer {peer_id}");
        self.swarm.behaviour_mut().request.send_request(peer_id, content);
    }

    fn broadcast_msg(&mut self, topic: String, content: T) {
        log::debug!("Broadcasting message with topic '{topic}'");
        let topic_hash = Sha256Topic::new(&topic).hash();
        let data = content.to_vec();
        let size = data.len();
        if let Err(e) = self.swarm.behaviour_mut().gossipsub.publish(topic_hash, data) {
            match e {
                PublishError::InsufficientPeers => (), // Nobody listening, not an actual error
                PublishError::MessageTooLarge => {
                    log::error!("Broadcast message too large. topic={topic} size={size} bytes")
                }
                e => log::error!("Error broadcasting message: {e:?} topic={topic}"),
            }
        }
    }

    fn subscribe(&mut self, topic: String, allow_unordered: bool) {
        log::debug!("Subscribing to topic {topic}");
        let topic = Sha256Topic::new(topic);
        let topic_hash = topic.hash();
        if let Entry::Vacant(e) = self.subscribed_topics.entry(topic_hash) {
            if let Err(e) = self.swarm.behaviour_mut().gossipsub.subscribe(&topic) {
                return log::error!("Subscribing failed: {e:?}");
            }
            e.insert((topic.to_string(), allow_unordered));
        }
    }

    fn unsubscribe(&mut self, topic: String) {
        log::debug!("Unsubscribing from topic {topic}");
        let topic = Sha256Topic::new(topic);
        let topic_hash = topic.hash();
        if self.subscribed_topics.remove(&topic_hash).is_some() {
            if let Err(e) = self.swarm.behaviour_mut().gossipsub.unsubscribe(&topic) {
                log::error!("Unsubscribing failed: {e:?}");
            }
        }
        self.sequence_numbers.retain(|(t, _), _| t != &topic_hash);
    }

    fn handle_subscription(&mut self, subscription: Subscription) {
        log::debug!("Handling subscription: {subscription:?}");
        if subscription.subscribed {
            self.subscribe(subscription.topic, subscription.allow_unordered)
        } else {
            self.unsubscribe(subscription.topic)
        }
    }

    fn handle_outbound_msg(&mut self, msg: Message<T>) {
        log::trace!("Handling outbound msg: {msg:?}");
        let Message {
            peer_id,
            content,
            topic,
        } = msg;
        if let Some(topic) = topic {
            return self.broadcast_msg(topic, content);
        }
        let peer_id = match peer_id {
            Some(peer_id) => peer_id,
            None => return log::error!("Cannot send message with neither peer_id nor topic"),
        };

        // Send the message right away if possible.
        if self.can_send_msg(&peer_id) {
            self.send_msg(&peer_id, content)
        }
        // Otherwise add message to queue and lookup peer on DHT.
        // All pending messages will be sent out once the peer is found.
        else {
            self.pending_messages.entry(peer_id).or_default().push(content);
            self.lookup_peer(peer_id);
        }
    }

    fn lookup_peer(&mut self, peer_id: PeerId) {
        if self.ongoing_queries.contains_left(&peer_id) {
            log::debug!("Query for peer {peer_id} already ongoing");
        } else {
            log::debug!("Starting query for peer {peer_id}");
            let query_id = self.swarm.behaviour_mut().kademlia.get_closest_peers(peer_id);
            self.ongoing_queries.insert(peer_id, query_id);
        }
    }

    async fn handle_swarm_event(
        &mut self,
        event: SwarmEvent<BehaviourEvent<T>>,
    ) -> Result<(), Error> {
        match event {
            SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(event)) => {
                self.handle_gossipsub_event(event).await
            }
            SwarmEvent::Behaviour(BehaviourEvent::Request(event)) => {
                self.handle_request_event(event).await
            }
            SwarmEvent::Behaviour(BehaviourEvent::Identify(event)) => {
                self.handle_identify_event(event)
            }
            SwarmEvent::Behaviour(BehaviourEvent::Kademlia(event)) => {
                self.handle_kademlia_event(event)
            }
            SwarmEvent::ConnectionEstablished {
                peer_id,
                connection_id,
                ..
            } => {
                #[cfg(feature = "metrics")]
                self.metrics.record(&event);
                self.handle_connection_established(peer_id, connection_id);
                Ok(())
            }
            SwarmEvent::ConnectionClosed {
                peer_id,
                connection_id,
                ..
            } => {
                #[cfg(feature = "metrics")]
                self.metrics.record(&event);
                self.handle_connection_closed(peer_id, connection_id);
                Ok(())
            }
            SwarmEvent::OutgoingConnectionError {
                peer_id,
                connection_id,
                ..
            } => {
                #[cfg(feature = "metrics")]
                self.metrics.record(&event);
                self.handle_connection_failed(peer_id, connection_id);
                Ok(())
            }
            e => {
                #[cfg(feature = "metrics")]
                self.metrics.record(&e);
                log::trace!("Swarm event: {e:?}");
                Ok(())
            }
        }
    }

    async fn handle_gossipsub_event(&mut self, event: gossipsub::Event) -> Result<(), Error> {
        log::debug!("Gossipsub event received: {event:?}");
        #[cfg(feature = "metrics")]
        self.metrics.record(&event);
        let (msg, propagation_source) = match event {
            gossipsub::Event::Message {
                message,
                propagation_source,
                ..
            } => (message, propagation_source),
            _ => return Ok(()),
        };
        let msg_id = gossipsub_msg_id(&msg);

        let (source, topic, data) = match self.validate_gossipsub_msg(msg) {
            Ok((source, topic, data)) => {
                let _ = self.swarm.behaviour_mut().gossipsub.report_message_validation_result(
                    &msg_id,
                    &propagation_source,
                    MessageAcceptance::Accept,
                );
                (source, topic, data)
            }
            Err(e) => {
                log::debug!("Discarding gossipsub message from {propagation_source}: {e}");
                let _ = self.swarm.behaviour_mut().gossipsub.report_message_validation_result(
                    &msg_id,
                    &propagation_source,
                    MessageAcceptance::Reject,
                );
                return Ok(());
            }
        };

        let msg = Message {
            peer_id: Some(source),
            content: T::from_vec(data),
            topic: Some(topic),
        };
        self.inbound_msg_sender
            .send(msg)
            .await
            .map_err(|_| Error::Unexpected("Inbound messages sink closed"))
    }

    /// Validate gossipsub message and return (source, topic, data)
    fn validate_gossipsub_msg(
        &mut self,
        msg: gossipsub::Message,
    ) -> Result<(PeerId, String, Vec<u8>), &'static str> {
        let source = match msg.source {
            Some(peer_id) => peer_id,
            None => return Err("anonymous message"),
        };
        let (topic, allow_unordered) = match self.subscribed_topics.get(&msg.topic) {
            Some(x) => x,
            None => return Err("message with unknown topic"),
        };
        if !allow_unordered {
            let key = (msg.topic, source);
            let last_seq_no = self.sequence_numbers.get(&key).copied().unwrap_or_default();
            match msg.sequence_number {
                None => return Err("message with out sequence number"),
                // Sequence numbers should be timestamp-based, can't be from the future
                Some(seq_no) if seq_no > timestamp_now() => return Err("invalid sequence number"),
                Some(seq_no) if seq_no <= last_seq_no => return Err("old message"),
                Some(seq_no) => self.sequence_numbers.insert(key, seq_no),
            };
        }

        Ok((source, topic.to_string(), msg.data))
    }

    async fn handle_request_event(
        &mut self,
        event: request_response::Event<T, ()>,
    ) -> Result<(), Error> {
        log::debug!("Request-Response event received: {event:?}");
        let (peer_id, content, channel) = match event {
            request_response::Event::Message {
                peer,
                message:
                    request_response::Message::Request {
                        request, channel, ..
                    },
            } => (peer, request, channel),
            request_response::Event::InboundFailure { error, peer, .. } => {
                return Err(Error::Inbound { error, peer })
            }
            request_response::Event::OutboundFailure { error, peer, .. } => {
                return Err(Error::Outbound { error, peer })
            }
            _ => return Ok(()),
        };
        // Send response just to prevent errors being emitted
        let _ = self.swarm.behaviour_mut().request.send_response(channel, ());
        let message = Message {
            peer_id: Some(peer_id),
            topic: None,
            content,
        };
        self.inbound_msg_sender
            .send(message)
            .await
            .map_err(|_| Error::Unexpected("Inbound messages sink closed"))
    }

    fn handle_identify_event(&mut self, event: identify::Event) -> Result<(), Error> {
        log::debug!("Identify event received: {event:?}");
        #[cfg(feature = "metrics")]
        self.metrics.record(&event);
        let (peer_id, listen_addrs) = match event {
            identify::Event::Received { peer_id, info } => (peer_id, info.listen_addrs),
            _ => return Ok(()),
        };
        let kademlia = &mut self.swarm.behaviour_mut().kademlia;
        listen_addrs.into_iter().filter(addr_is_reachable).for_each(|addr| {
            kademlia.add_address(&peer_id, addr);
        });
        Ok(())
    }

    fn handle_kademlia_event(&mut self, event: kad::Event) -> Result<(), Error> {
        log::debug!("Kademlia event received: {event:?}");
        #[cfg(feature = "metrics")]
        self.metrics.record(&event);
        let (query_id, result, finished) = match event {
            kad::Event::OutboundQueryProgressed {
                id,
                result: QueryResult::GetClosestPeers(result),
                step: ProgressStep { last, .. },
                ..
            } => (id, result, last),
            _ => return Ok(()),
        };

        let peer_id = self
            .ongoing_queries
            .get_by_right(&query_id)
            .ok_or(Error::Unexpected("Unknown query"))?
            .to_owned();
        let peers = match result {
            Ok(GetClosestPeersOk { peers, .. }) => peers,
            Err(GetClosestPeersError::Timeout { peers, .. }) => peers,
        };

        // Query reached the peer that was looked for. Send all pending messages.
        if peers.contains(&peer_id) {
            self.ongoing_queries.remove_by_right(&query_id);
            self.peer_found(peer_id);
        }
        // Query finished and the peer wasn't found. Drop all pending messages and dial requests.
        else if finished {
            self.ongoing_queries.remove_by_right(&query_id);
            self.peer_not_found(&peer_id);
        }

        Ok(())
    }

    fn send_pending_messages(&mut self, peer_id: &PeerId) {
        log::debug!("Sending pending messages to {peer_id}");
        self.pending_messages
            .remove(peer_id)
            .into_iter()
            .flatten()
            .for_each(|msg| self.send_msg(peer_id, msg));
    }

    fn peer_found(&mut self, peer_id: PeerId) {
        log::debug!("Peer found: {peer_id}");
        self.pending_dials
            .remove(&peer_id)
            .into_iter()
            .flatten()
            .for_each(|rs| self.dial_peer(peer_id, rs));
        self.send_pending_messages(&peer_id);
    }

    fn peer_not_found(&mut self, peer_id: &PeerId) {
        log::debug!("Peer not found: {peer_id}");
        self.pending_dials
            .remove(peer_id)
            .into_iter()
            .flatten()
            .for_each(|rs| rs.send_result(false));
        self.pending_messages.remove(peer_id);
    }

    fn dial_peer(&mut self, peer_id: PeerId, result_sender: DialResultSender) {
        log::debug!("Dialing peer {peer_id}");
        let dial_opts = DialOpts::peer_id(peer_id).condition(PeerCondition::Always).build();
        let conn_id = dial_opts.connection_id();
        match self.swarm.dial(dial_opts) {
            Err(DialError::NoAddresses) => {
                self.pending_dials.entry(peer_id).or_default().push(result_sender);
                self.lookup_peer(peer_id);
            }
            Err(e) => {
                log::warn!("Cannot dial peer {peer_id}: {e:?}");
                result_sender.send_result(false);
            }
            Ok(()) => {
                self.ongoing_dials.insert(conn_id, result_sender);
            }
        }
    }

    fn handle_connection_established(&mut self, peer_id: PeerId, connection_id: ConnectionId) {
        log::debug!("Connection established with {peer_id}");
        self.send_pending_messages(&peer_id);
        if let Some(result_sender) = self.ongoing_dials.remove(&connection_id) {
            result_sender.send_result(true);
        }

        let peer_conns = self.active_connections.entry(peer_id).or_default();
        peer_conns.push_front(connection_id);
        if peer_conns.len() > MAX_CONNS_PER_PEER as usize {
            log::debug!("Connection limit reached for {peer_id}");
            let conn_to_close = peer_conns.back().expect("not empty");
            self.swarm.close_connection(*conn_to_close);
        }
    }

    fn handle_connection_closed(&mut self, peer_id: PeerId, connection_id: ConnectionId) {
        log::debug!("Connection with {peer_id} closed");
        match self.active_connections.get_mut(&peer_id) {
            Some(conns) => conns.retain(|cid| *cid != connection_id),
            None => log::warn!("Unknown connection peer_id={peer_id} conn_id={connection_id}"),
        }
    }

    fn handle_connection_failed(&mut self, peer_id: Option<PeerId>, connection_id: ConnectionId) {
        let peer_id = peer_id.map(|id| id.to_string()).unwrap_or("<unknown>".to_string());
        log::debug!("Outgoing connection to {peer_id} failed");
        if let Some(result_sender) = self.ongoing_dials.remove(&connection_id) {
            result_sender.send_result(false)
        }
    }
}

// Default gossipsub msg ID function, copied from libp2p
fn gossipsub_msg_id(msg: &gossipsub::Message) -> gossipsub::MessageId {
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
