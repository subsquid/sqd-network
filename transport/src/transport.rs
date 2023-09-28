use std::{
    collections::{hash_map::Entry, HashMap},
    fmt::Debug,
    marker::PhantomData,
    time::Duration,
};

use async_trait::async_trait;
use bimap::BiHashMap;
use futures::{
    stream::{Fuse, StreamExt},
    AsyncReadExt as FutAsyncRead, AsyncWriteExt,
};
use libp2p::{
    autonat,
    autonat::NatStatus,
    core::{transport::OrTransport, upgrade},
    dcutr,
    dns::TokioDnsConfig,
    gossipsub::{self, MessageAuthenticity, Sha256Topic, TopicHash},
    identify,
    identity::Keypair,
    kad::{
        store::MemoryStore, GetClosestPeersError, GetClosestPeersOk, Kademlia, KademliaEvent,
        QueryId, QueryResult,
    },
    multiaddr::Protocol,
    noise, ping, relay,
    relay::client::Behaviour as RelayClient,
    request_response,
    request_response::ProtocolSupport,
    swarm::{behaviour::toggle::Toggle, dial_opts::DialOpts, SwarmBuilder, SwarmEvent},
    tcp, yamux, Multiaddr, PeerId, Swarm, Transport,
};
use libp2p_swarm_derive::NetworkBehaviour;
use rand::prelude::SliceRandom;
use tokio::{sync::mpsc, time::interval};
use tokio_stream::wrappers::{IntervalStream, ReceiverStream};

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

#[derive(NetworkBehaviour)]
struct Behaviour<T>
where
    T: MsgContent,
{
    identify: identify::Behaviour,
    kademlia: Kademlia<MemoryStore>,
    autonat: Toggle<autonat::Behaviour>,
    relay: RelayClient,
    dcutr: dcutr::Behaviour,
    request: request_response::Behaviour<MessageCodec<T>>,
    gossipsub: gossipsub::Behaviour,
    ping: ping::Behaviour,
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
        log::debug!("New message decoded: {}", String::from_utf8_lossy(msg.as_slice()));
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
        log::debug!("New message to encode: {}", String::from_utf8_lossy(req));
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
    private_node: bool,
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
            private_node: true,
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
            private_node: args.private_node,
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

    pub fn allow_private(&mut self, allow: bool) {
        self.private_node = allow;
    }

    pub fn local_peer_id(&self) -> PeerId {
        PeerId::from(self.keypair.public())
    }

    fn build_swarm<T: MsgContent>(keypair: Keypair, private_node: bool) -> Swarm<Behaviour<T>> {
        let local_peer_id = PeerId::from(keypair.public());

        let protocol = SUBSQUID_PROTOCOL.to_string();
        let (relay_transport, relay) = relay::client::new(local_peer_id);

        let tcp_transport =
            TokioDnsConfig::system(tcp::tokio::Transport::new(tcp::Config::new().nodelay(true)))
                .unwrap();
        let transport = OrTransport::new(relay_transport, tcp_transport)
            .upgrade(upgrade::Version::V1)
            .authenticate(noise::Config::new(&keypair).unwrap())
            .multiplex(yamux::Config::default())
            .boxed();

        let mut request_config = request_response::Config::default();
        request_config.set_request_timeout(Duration::from_secs(60));
        let behaviour = Behaviour {
            relay,
            identify: identify::Behaviour::new(
                identify::Config::new(protocol, keypair.public())
                    .with_interval(Duration::from_secs(60))
                    .with_push_listen_addr_updates(true),
            ),
            kademlia: Kademlia::with_config(
                local_peer_id,
                MemoryStore::new(local_peer_id),
                Default::default(),
            ),
            autonat: Toggle::from(
                (!private_node).then(|| autonat::Behaviour::new(local_peer_id, Default::default())),
            ),
            dcutr: dcutr::Behaviour::new(local_peer_id),
            request: request_response::Behaviour::new(
                vec![(WORKER_PROTOCOL, ProtocolSupport::Full)],
                request_config,
            ),
            gossipsub: gossipsub::Behaviour::new(
                MessageAuthenticity::Signed(keypair),
                Default::default(),
            )
            .unwrap(),
            ping: ping::Behaviour::new(Default::default()),
        };

        SwarmBuilder::with_tokio_executor(transport, behaviour, local_peer_id).build()
    }

    #[allow(clippy::needless_pass_by_ref_mut)]
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
                    e => log::warn!("Unexpected swarm event: {e:?}"),
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
                e => log::warn!("Unexpected swarm event: {e:?}"),
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
    ) -> Result<(InboundMsgReceiver<T>, OutboundMsgSender<T>, SubscriptionSender), Error> {
        log::info!("Local peer ID: {}", self.keypair.public().to_peer_id());
        let mut swarm = Self::build_swarm(self.keypair, self.private_node);

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
        let transport =
            P2PTransport::new(inbound_tx, outbound_rx, subscription_rx, swarm, self.bootstrap);

        tokio::task::spawn(transport.run());
        Ok((inbound_rx, outbound_tx, subscription_tx))
    }
}

struct P2PTransport<T: MsgContent> {
    inbound_msg_sender: mpsc::Sender<Message<T>>,
    outbound_msg_receiver: Fuse<ReceiverStream<Message<T>>>,
    subscription_receiver: Fuse<ReceiverStream<Subscription>>,
    pending_queries: BiHashMap<PeerId, QueryId>,
    pending_messages: HashMap<PeerId, Vec<T>>,
    subscribed_topics: HashMap<TopicHash, (String, bool)>, // hash -> (topic, allow_unordered)
    sequence_numbers: HashMap<(TopicHash, PeerId), u64>,
    swarm: Swarm<Behaviour<T>>,
    bootstrap: bool,
    running: bool,
}

impl<T: MsgContent> P2PTransport<T> {
    pub fn new(
        inbound_msg_sender: mpsc::Sender<Message<T>>,
        outbound_msg_receiver: mpsc::Receiver<Message<T>>,
        subscription_receiver: mpsc::Receiver<Subscription>,
        swarm: Swarm<Behaviour<T>>,
        bootstrap: bool,
    ) -> Self {
        Self {
            inbound_msg_sender,
            outbound_msg_receiver: ReceiverStream::new(outbound_msg_receiver).fuse(),
            subscription_receiver: ReceiverStream::new(subscription_receiver).fuse(),
            pending_queries: BiHashMap::new(),
            pending_messages: HashMap::new(),
            subscribed_topics: HashMap::new(),
            sequence_numbers: HashMap::new(),
            swarm,
            bootstrap,
            running: true,
        }
    }

    pub async fn run(mut self) {
        log::info!("P2PTransport starting");
        let mut bootstrap_timer = IntervalStream::new(interval(BOOTSTRAP_INTERVAL)).fuse();
        while self.running {
            futures::select! {
                _ = bootstrap_timer.select_next_some() => self.bootstrap(),
                event = self.swarm.select_next_some() => self.handle_swarm_event(event).await.unwrap_or_else(|e| {
                    log::error!("Error handling swarm event: {e}")
                }),
                msg = self.outbound_msg_receiver.select_next_some() =>
                    self.handle_outbound_msg(msg),
                subscription = self.subscription_receiver.select_next_some() =>
                    self.handle_subscription(subscription),
            }
        }
        log::info!("Shutting down P2P transport");
    }

    fn bootstrap(&mut self) {
        if self.bootstrap {
            log::info!("Bootstrapping kademlia");
            if let Err(e) = self.swarm.behaviour_mut().kademlia.bootstrap() {
                log::error!("Cannot bootstrap kademlia: {e:?}");
                self.running = false;
            }
        }
    }

    fn can_send_msg(&mut self, peer_id: &PeerId) -> bool {
        self.swarm.is_connected(peer_id)
            || self
                .swarm
                .behaviour_mut()
                .kademlia
                .kbucket(*peer_id)
                .is_some_and(|x| !x.is_empty())
    }

    fn send_msg(&mut self, peer_id: &PeerId, content: T) {
        log::debug!("Sending message to peer {peer_id}");
        self.swarm.behaviour_mut().request.send_request(peer_id, content);
    }

    fn broadcast_msg(&mut self, topic: String, content: T) {
        log::debug!("Broadcasting message with topic '{topic}'");
        let topic = Sha256Topic::new(topic).hash();
        let data = content.to_vec();
        if let Err(e) = self.swarm.behaviour_mut().gossipsub.publish(topic, data) {
            log::error!("Error broadcasting message: {e:?}");
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
        log::debug!("Handling outbound msg: {msg:?}");
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
        // If there is an ongoing query for the recipient peer,
        // put the message in queue – it will be sent once the query is complete.
        else if self.pending_queries.contains_left(&peer_id) {
            log::debug!("Appending message to queue");
            self.pending_messages
                .get_mut(&peer_id)
                .expect("There should always be at least one pending message")
                .push(content);
        }
        // Start a new query, if necessary.
        else {
            log::debug!("Starting query for peer {peer_id}");
            let query_id = self.swarm.behaviour_mut().kademlia.get_closest_peers(peer_id);
            self.pending_queries.insert(peer_id, query_id);
            self.pending_messages.insert(peer_id, vec![content]);
        }
    }

    async fn handle_swarm_event<E: Debug>(
        &mut self,
        event: SwarmEvent<BehaviourEvent<T>, E>,
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
            SwarmEvent::Behaviour(BehaviourEvent::Autonat(event)) => {
                self.handle_autonat_event(event);
                Ok(())
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                self.send_pending_messages(&peer_id);
                Ok(())
            }
            e => Ok(log::trace!("Swarm event: {e:?}")),
        }
    }

    async fn handle_gossipsub_event(&mut self, event: gossipsub::Event) -> Result<(), Error> {
        log::debug!("Gossipsub event received: {event:?}");
        let msg = match event {
            gossipsub::Event::Message { message, .. } => message,
            _ => return Ok(()),
        };
        let source = match msg.source {
            Some(peer_id) => peer_id,
            None => return Ok(log::warn!("Dropping anonymous message")),
        };
        let (topic, allow_unordered) = match self.subscribed_topics.get(&msg.topic) {
            Some(x) => x,
            None => return Ok(log::warn!("Dropping message with unknown topic")),
        };
        if !allow_unordered {
            let key = (msg.topic, source);
            let last_seq_no = self.sequence_numbers.get(&key).copied().unwrap_or_default();
            match msg.sequence_number {
                Some(seq_no) if seq_no > last_seq_no => {
                    self.sequence_numbers.insert(key, seq_no);
                }
                _ => return Ok(log::debug!("Dropping message with old sequence number")),
            }
        }

        let msg = Message {
            peer_id: Some(source),
            content: T::from_vec(msg.data),
            topic: Some(topic.to_owned()),
        };
        self.inbound_msg_sender
            .send(msg)
            .await
            .map_err(|_| Error::Unexpected("Inbound messages sink closed"))
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

    fn handle_kademlia_event(&mut self, event: KademliaEvent) -> Result<(), Error> {
        log::debug!("Kademlia event received: {event:?}");
        let (query_id, result) = match event {
            KademliaEvent::OutboundQueryProgressed {
                id,
                result: QueryResult::GetClosestPeers(result),
                ..
            } => (id, result),
            _ => return Ok(()),
        };

        let peer_id = self
            .pending_queries
            .get_by_right(&query_id)
            .ok_or(Error::Unexpected("Unknown query"))?
            .to_owned();
        let (timeout, peers) = match result {
            Ok(GetClosestPeersOk { peers, .. }) => (false, peers),
            Err(GetClosestPeersError::Timeout { peers, .. }) => (true, peers),
        };

        // Query reached the peer that was looked for. Send all pending messages.
        if peers.contains(&peer_id) {
            self.pending_queries.remove_by_right(&query_id);
            self.send_pending_messages(&peer_id);
        }
        // Query timed out and the peer wasn't found. Drop all pending messages.
        else if timeout {
            self.pending_queries.remove_by_right(&query_id);
            self.pending_messages.remove(&peer_id);
            return Err(Error::QueryTimeout(peer_id));
        }

        Ok(())
    }

    fn send_pending_messages(&mut self, peer_id: &PeerId) {
        self.pending_messages
            .remove(peer_id)
            .map(|messages| messages.into_iter().map(|msg| self.send_msg(peer_id, msg)));
    }

    fn handle_autonat_event(&mut self, event: autonat::Event) {
        log::debug!("AutoNAT event received: {event:?}");
        let autonat = self.swarm.behaviour().autonat.as_ref();
        if let (autonat::Event::OutboundProbe(_), Some(autonat)) = (event, autonat) {
            let status = autonat.nat_status();
            let confidence = autonat.confidence();
            if matches!(status, NatStatus::Private) && confidence > 0 {
                log::error!("Node is not publicly reachable!");
                self.running = false;
            }
        }
    }
}
