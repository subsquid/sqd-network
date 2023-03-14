use async_trait::async_trait;
use bimap::BiHashMap;
use std::{collections::HashMap, fmt::Debug, time::Duration};

use futures::{
    stream::{Fuse, StreamExt},
    AsyncReadExt as FutAsyncRead, AsyncWriteExt,
};

use libp2p::{
    // autonat,
    core::{transport::OrTransport, upgrade, ProtocolName},
    dcutr,
    dns::TokioDnsConfig,
    identify,
    identity::Keypair,
    kad::{
        store::MemoryStore, GetClosestPeersError, GetClosestPeersOk, Kademlia, KademliaEvent,
        QueryId, QueryResult,
    },
    multiaddr::Protocol,
    noise,
    relay::v2::client::Client as RelayClient,
    request_response::{
        ProtocolSupport, RequestResponse, RequestResponseCodec, RequestResponseEvent,
        RequestResponseMessage,
    },
    swarm::{dial_opts::DialOpts, NetworkBehaviour, SwarmEvent},
    tcp,
    yamux::YamuxConfig,
    Multiaddr,
    PeerId,
    Swarm,
    Transport,
};
use libp2p_swarm_derive::NetworkBehaviour;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use crate::{ffi, Error, Message, MsgContent};

pub const GRPC_PROTOCOL: &[u8] = b"/grpc/0.0.1";
pub const SUBSQUID_PROTOCOL: &[u8] = b"/subsquid/0.0.1";

#[derive(NetworkBehaviour)]
struct WorkerBehaviour {
    identify: identify::Behaviour,
    kademlia: Kademlia<MemoryStore>,
    // autonat: autonat::Behaviour,
    relay: RelayClient,
    dcutr: dcutr::behaviour::Behaviour,
    request: RequestResponse<MessageCodec>,
}

#[derive(Debug, Clone)]
struct WorkerProtocol();

impl ProtocolName for WorkerProtocol {
    fn protocol_name(&self) -> &[u8] {
        "/subsquid-worker/0.0.1".as_bytes()
    }
}

#[derive(Debug, Clone)]
struct MessageCodec();

#[async_trait]
impl RequestResponseCodec for MessageCodec {
    type Protocol = WorkerProtocol;
    type Request = MsgContent;
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

        let mut buf = ffi::new_buffer(msg_len);
        let buf_slice = buf
            .as_mut()
            .expect("Newly created buffer should never be a null pointer")
            .as_mut_slice();
        io.read_exact(buf_slice).await?;
        log::debug!("New message decoded: {}", String::from_utf8_lossy(buf.as_slice()));
        Ok(buf)
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
        log::debug!("New message to encode: {}", String::from_utf8_lossy(req.as_slice()));
        let msg_len = req.len().to_be_bytes();
        io.write_all(&msg_len).await?;
        io.write_all(req.as_slice()).await
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
    boot_nodes: Vec<(PeerId, Multiaddr)>,
    relay: Option<Multiaddr>,
    bootstrap: bool,
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
            boot_nodes: vec![],
            relay: None,
            bootstrap: true,
        }
    }

    pub fn listen_on<I: IntoIterator<Item = Multiaddr>>(&mut self, addrs: I) {
        self.listen_addrs.extend(addrs.into_iter())
    }

    pub fn boot_nodes<I: IntoIterator<Item = (PeerId, Multiaddr)>>(&mut self, nodes: I) {
        self.boot_nodes.extend(nodes.into_iter())
    }

    pub fn relay(&mut self, addr: Multiaddr) {
        self.relay = Some(addr);
    }

    pub fn bootstrap(&mut self, bootstrap: bool) {
        self.bootstrap = bootstrap;
    }

    fn build_swarm(keypair: Keypair) -> Swarm<WorkerBehaviour> {
        let local_peer_id = PeerId::from(keypair.public());
        log::info!("Local peer ID: {local_peer_id}");

        let protocol = std::str::from_utf8(SUBSQUID_PROTOCOL).unwrap().to_string();
        let (relay_transport, relay) = RelayClient::new_transport_and_behaviour(local_peer_id);
        let behaviour = WorkerBehaviour {
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
            dcutr: Default::default(),
            request: RequestResponse::new(
                MessageCodec(),
                vec![(WorkerProtocol(), ProtocolSupport::Full)],
                Default::default(),
            ),
        };

        let tcp_transport =
            TokioDnsConfig::system(tcp::tokio::Transport::new(tcp::Config::new().nodelay(true)))
                .unwrap();
        let transport = OrTransport::new(relay_transport, tcp_transport)
            .upgrade(upgrade::Version::V1)
            .authenticate(noise::NoiseAuthenticated::xx(&keypair).unwrap())
            .multiplex(YamuxConfig::default())
            .boxed();

        Swarm::with_tokio_executor(transport, behaviour, local_peer_id)
    }

    async fn wait_for_listening(swarm: &mut Swarm<WorkerBehaviour>) {
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

    async fn wait_for_first_connection(swarm: &mut Swarm<WorkerBehaviour>) {
        loop {
            match swarm.next().await.unwrap() {
                SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                    log::info!("Connection established with {peer_id}");
                    break;
                }
                SwarmEvent::Behaviour(WorkerBehaviourEvent::Kademlia(_)) => {}
                e => log::warn!("Unexpected swarm event: {e:?}"),
            }
        }
    }

    async fn wait_for_identify(swarm: &mut Swarm<WorkerBehaviour>) {
        let mut received = false;
        let mut sent = false;
        while !(received && sent) {
            match swarm.next().await.unwrap() {
                SwarmEvent::Behaviour(WorkerBehaviourEvent::Identify(identify::Event::Sent {
                    ..
                })) => {
                    sent = true;
                }
                SwarmEvent::Behaviour(WorkerBehaviourEvent::Identify(
                    identify::Event::Received { .. },
                )) => {
                    received = true;
                }
                event => log::debug!("{:?}", event),
            }
        }
    }

    pub async fn run(self) -> Result<(mpsc::Receiver<Message>, mpsc::Sender<Message>), Error> {
        let mut swarm = Self::build_swarm(self.keypair);

        // If relay node not specified explicitly, use first boot node (TODO: random boot node?)
        let relay = self.relay.or_else(|| {
            self.boot_nodes
                .first()
                .map(|(peer_id, addr)| addr.clone().with(Protocol::P2p((*peer_id).into())))
        });

        // Listen on provided addresses
        for addr in self.listen_addrs {
            swarm.listen_on(addr)?;
        }
        Self::wait_for_listening(&mut swarm).await;

        // Connect to boot nodes
        if !self.boot_nodes.is_empty() {
            for (peer_id, addr) in self.boot_nodes {
                log::info!("Connecting to boot node {peer_id} at {addr}");
                swarm.behaviour_mut().kademlia.add_address(&peer_id, addr.clone());
                swarm.dial(DialOpts::peer_id(peer_id).addresses(vec![addr]).build())?;
            }
            Self::wait_for_first_connection(&mut swarm).await;
        }

        // Listen for relayed connections
        if let Some(addr) = relay {
            log::info!("Connecting to relay {addr}");
            swarm.dial(addr.clone())?;
            Self::wait_for_identify(&mut swarm).await; // TODO: Is this really needed?
            swarm.listen_on(addr.with(Protocol::P2pCircuit))?;
        }

        if self.bootstrap {
            log::info!("Bootstrapping kademlia");
            swarm.behaviour_mut().kademlia.bootstrap()?;
        }

        let (inbound_tx, inbound_rx) = mpsc::channel(1024);
        let (outbound_tx, outbound_rx) = mpsc::channel(1024);
        let transport = P2PTransport::new(inbound_tx, outbound_rx, swarm);

        tokio::task::spawn(transport.run());
        Ok((inbound_rx, outbound_tx))
    }
}

struct P2PTransport {
    inbound_msg_sender: mpsc::Sender<Message>,
    outbound_msg_receiver: Fuse<ReceiverStream<Message>>,
    pending_queries: BiHashMap<PeerId, QueryId>,
    pending_messages: HashMap<PeerId, Vec<MsgContent>>,
    swarm: Swarm<WorkerBehaviour>,
}

impl P2PTransport {
    pub fn new(
        inbound_msg_sender: mpsc::Sender<Message>,
        outbound_msg_receiver: mpsc::Receiver<Message>,
        swarm: Swarm<WorkerBehaviour>,
    ) -> Self {
        Self {
            inbound_msg_sender,
            outbound_msg_receiver: ReceiverStream::new(outbound_msg_receiver).fuse(),
            pending_queries: BiHashMap::new(),
            pending_messages: HashMap::new(),
            swarm,
        }
    }

    pub async fn run(mut self) {
        log::debug!("P2PTransport starting");
        loop {
            futures::select! {
                event = self.swarm.select_next_some() => self.handle_swarm_event(event).await.unwrap_or_else(|e| {
                    log::error!("Error handling swarm event: {e}")
                }),
                msg = self.outbound_msg_receiver.select_next_some() =>
                    self.handle_outbound_msg(msg)
            }
        }
    }

    fn can_send_msg(&mut self, peer_id: &PeerId) -> bool {
        self.swarm.is_connected(peer_id)
            || !self.swarm.behaviour_mut().addresses_of_peer(peer_id).is_empty()
    }

    fn send_msg(&mut self, peer_id: &PeerId, content: MsgContent) {
        log::debug!("Sending message to peer {peer_id}");
        self.swarm.behaviour_mut().request.send_request(peer_id, content);
    }

    fn handle_outbound_msg(&mut self, msg: Message) {
        log::debug!("Handling outbound msg: {msg:?}");
        let Message { peer_id, content } = msg;
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
        event: SwarmEvent<WorkerBehaviourEvent, E>,
    ) -> Result<(), Error> {
        log::debug!("P2PTransport handling swarm event: {event:?}");
        match event {
            SwarmEvent::Behaviour(WorkerBehaviourEvent::Request(event)) => {
                self.handle_request_event(event).await
            }
            SwarmEvent::Behaviour(WorkerBehaviourEvent::Identify(event)) => {
                self.handle_identify_event(event)
            }
            SwarmEvent::Behaviour(WorkerBehaviourEvent::Kademlia(event)) => {
                self.handle_kademlia_event(event)
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                self.send_pending_messages(&peer_id);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn handle_request_event(
        &mut self,
        event: RequestResponseEvent<MsgContent, ()>,
    ) -> Result<(), Error> {
        log::debug!("Request-Response event received: {event:?}");
        let (peer_id, content, channel) = match event {
            RequestResponseEvent::Message {
                peer,
                message:
                    RequestResponseMessage::Request {
                        request, channel, ..
                    },
            } => (peer, request, channel),
            RequestResponseEvent::InboundFailure { error, .. } => return Err(error)?,
            RequestResponseEvent::OutboundFailure { error, .. } => return Err(error)?,
            _ => return Ok(()),
        };
        // Sending response just to emitting errors
        let _ = self.swarm.behaviour_mut().request.send_response(channel, ());
        let message = Message { peer_id, content };
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
        for address in listen_addrs {
            // TODO: Filter out private network addresses
            kademlia.add_address(&peer_id, address);
        }
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
}
