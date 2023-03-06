use bimap::BiHashMap;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fmt::Debug,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures::{
    future,
    future::BoxFuture,
    stream,
    stream::{Fuse, FusedStream, Stream, StreamExt},
    FutureExt, TryFutureExt,
};

use libp2p::{
    autonat,
    core::{connection::ConnectionId, transport::OrTransport, upgrade, upgrade::ReadyUpgrade},
    dcutr,
    dns::TokioDnsConfig,
    identify,
    identity::Keypair,
    kad::{
        store::MemoryStore, BootstrapResult, GetClosestPeersResult, Kademlia, KademliaConfig,
        KademliaEvent, QueryId, QueryResult,
    },
    multiaddr::Protocol,
    noise,
    relay::v2::client::Client as RelayClient,
    swarm::{
        behaviour::{ConnectionClosed, ConnectionEstablished, DialFailure, FromSwarm},
        dial_opts::{DialOpts, PeerCondition},
        handler::ConnectionEvent,
        ConnectionHandler, ConnectionHandlerEvent, KeepAlive, NegotiatedSubstream,
        NetworkBehaviour, NetworkBehaviourAction, NotifyHandler, PollParameters, SubstreamProtocol,
        SwarmEvent,
    },
    tcp,
    yamux::YamuxConfig,
    Multiaddr, PeerId, Swarm, Transport,
};
use libp2p_swarm_derive::NetworkBehaviour;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt, ReadBuf},
    sync::{mpsc, oneshot},
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};

use tonic::transport::{server::Connected, Uri};

use tower::Service;

use crate::{ffi, Error, Message, MsgContent};

pub const GRPC_PROTOCOL: &[u8] = b"/grpc/0.0.1";
pub const SUBSQUID_PROTOCOL: &[u8] = b"/subsquid/0.0.1";

#[derive(NetworkBehaviour)]
struct WorkerBehaviour {
    grpc: GrpcBehaviour,
    identify: identify::Behaviour,
    kademlia: Kademlia<MemoryStore>,
    autonat: autonat::Behaviour,
    relay: RelayClient,
    dcutr: dcutr::behaviour::Behaviour,
}

#[derive(Default)]
struct GrpcBehaviour {
    inbound_streams: VecDeque<P2PConnection>,
    outbound_streams: VecDeque<P2PConnection>,
    stream_requests: VecDeque<PeerId>,
    connected_peers: HashSet<PeerId>,
    request_failures: VecDeque<RequestFailure>,
}

impl GrpcBehaviour {
    pub fn request_stream(&mut self, peer_id: PeerId) {
        self.stream_requests.push_front(peer_id);
    }
}

#[derive(Debug)]
enum GrpcBehaviourEvent {
    InboundStream(P2PConnection),
    OutboundStream(P2PConnection),
    RequestFailed(RequestFailure),
}

#[derive(Debug)]
struct RequestFailure {
    peer_id: PeerId,
    error: Error,
}

impl NetworkBehaviour for GrpcBehaviour {
    type ConnectionHandler = GrpcConnectionHandler;
    type OutEvent = GrpcBehaviourEvent;

    fn new_handler(&mut self) -> Self::ConnectionHandler {
        log::trace!("GrpcBehaviour: new connection handler");
        Default::default()
    }

    fn on_swarm_event(&mut self, event: FromSwarm<Self::ConnectionHandler>) {
        match event {
            FromSwarm::ConnectionEstablished(ConnectionEstablished { peer_id, .. }) => {
                log::debug!("GrpcBehaviour: connection established with {peer_id}");
                self.connected_peers.insert(peer_id);
            }
            FromSwarm::ConnectionClosed(ConnectionClosed { peer_id, .. }) => {
                log::debug!("GrpcBehaviour: connection with {peer_id} closed");
                self.connected_peers.remove(&peer_id);
            }
            FromSwarm::DialFailure(DialFailure {
                peer_id: Some(peer_id),
                error,
                ..
            }) => {
                log::error!("GrpcBehaviour: dialing peer {peer_id} failed: {error:?}");
                let failure = RequestFailure {
                    peer_id,
                    error: error.into(),
                };
                self.request_failures.push_front(failure);
            }
            _ => {}
        }
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        _connection_id: ConnectionId,
        event: GrpcHandlerEvent,
    ) {
        match event {
            GrpcHandlerEvent::InboundStream(stream) => {
                self.inbound_streams.push_front(P2PConnection::new(peer_id, stream))
            }
            GrpcHandlerEvent::OutboundStream(stream) => {
                self.outbound_streams.push_front(P2PConnection::new(peer_id, stream))
            }
        }
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
        _params: &mut impl PollParameters,
    ) -> Poll<NetworkBehaviourAction<Self::OutEvent, Self::ConnectionHandler>> {
        log::trace!("GrpcBehaviour: poll");
        if let Some(stream) = self.inbound_streams.pop_back() {
            log::trace!("GrpcBehaviour: yielding inbound substream");
            return Poll::Ready(NetworkBehaviourAction::GenerateEvent(
                GrpcBehaviourEvent::InboundStream(stream),
            ));
        }
        if let Some(stream) = self.outbound_streams.pop_back() {
            log::trace!("GrpcBehaviour: yielding outbound substream");
            return Poll::Ready(NetworkBehaviourAction::GenerateEvent(
                GrpcBehaviourEvent::OutboundStream(stream),
            ));
        }
        if let Some(failure) = self.request_failures.pop_back() {
            log::trace!("GrpcBehaviour: yielding request failure {failure:?}");
            return Poll::Ready(NetworkBehaviourAction::GenerateEvent(
                GrpcBehaviourEvent::RequestFailed(failure),
            ));
        }
        if let Some(peer_id) = self.stream_requests.pop_back() {
            let action = if self.connected_peers.contains(&peer_id) {
                log::trace!("GrpcBehaviour: requesting substream for {peer_id}");
                NetworkBehaviourAction::NotifyHandler {
                    peer_id,
                    handler: NotifyHandler::Any,
                    event: RequestStream {},
                }
            } else {
                log::trace!("GrpcBehaviour: peer not connected: {peer_id}");
                let mut handler = GrpcConnectionHandler::default();
                // The handler needs to be created already with a pending stream request,
                // so that it opens a new stream as soon as the connection is established.
                handler.on_behaviour_event(RequestStream {});
                NetworkBehaviourAction::Dial {
                    opts: DialOpts::peer_id(peer_id).condition(PeerCondition::Disconnected).build(),
                    handler,
                }
            };
            return Poll::Ready(action);
        }
        Poll::Pending
    }
}

#[derive(Default)]
struct GrpcConnectionHandler {
    inbound_streams: VecDeque<NegotiatedSubstream>,
    outbound_streams: VecDeque<NegotiatedSubstream>,
    requested_streams: usize,
}

#[derive(Debug)]
struct RequestStream {}

type GrpcProtocol = ReadyUpgrade<&'static [u8]>;

#[derive(Debug)]
enum GrpcHandlerEvent {
    InboundStream(NegotiatedSubstream),
    OutboundStream(NegotiatedSubstream),
}

impl ConnectionHandler for GrpcConnectionHandler {
    type InEvent = RequestStream;
    type OutEvent = GrpcHandlerEvent;
    type Error = Error;
    type InboundProtocol = GrpcProtocol;
    type OutboundProtocol = GrpcProtocol;
    type InboundOpenInfo = ();
    type OutboundOpenInfo = ();

    fn listen_protocol(&self) -> SubstreamProtocol<Self::InboundProtocol, Self::InboundOpenInfo> {
        SubstreamProtocol::new(ReadyUpgrade::new(GRPC_PROTOCOL), ())
    }

    fn connection_keep_alive(&self) -> KeepAlive {
        KeepAlive::Yes
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
    ) -> Poll<
        ConnectionHandlerEvent<
            Self::OutboundProtocol,
            Self::OutboundOpenInfo,
            Self::OutEvent,
            Self::Error,
        >,
    > {
        if let Some(stream) = self.inbound_streams.pop_back() {
            log::trace!("GrpcConnectionHandler: yielding inbound substream");
            return Poll::Ready(ConnectionHandlerEvent::Custom(GrpcHandlerEvent::InboundStream(
                stream,
            )));
        }
        if let Some(stream) = self.outbound_streams.pop_back() {
            log::trace!("GrpcConnectionHandler: yielding outbound substream");
            return Poll::Ready(ConnectionHandlerEvent::Custom(GrpcHandlerEvent::OutboundStream(
                stream,
            )));
        }
        if self.requested_streams > 0 {
            log::trace!("GrpcConnectionHandler: requesting substream");
            self.requested_streams -= 1;
            let protocol = SubstreamProtocol::new(ReadyUpgrade::new(GRPC_PROTOCOL), ());
            return Poll::Ready(ConnectionHandlerEvent::OutboundSubstreamRequest { protocol });
        }
        Poll::Pending
    }

    fn on_behaviour_event(&mut self, _event: Self::InEvent) {
        log::trace!("GrpcConnectionHandler: new substream requested");
        self.requested_streams += 1;
    }

    fn on_connection_event(
        &mut self,
        event: ConnectionEvent<
            Self::InboundProtocol,
            Self::OutboundProtocol,
            Self::InboundOpenInfo,
            Self::OutboundOpenInfo,
        >,
    ) {
        match event {
            ConnectionEvent::FullyNegotiatedInbound(inbound) => {
                log::trace!("Connection event: FullyNegotiatedInbound");
                self.inbound_streams.push_front(inbound.protocol)
            }
            ConnectionEvent::FullyNegotiatedOutbound(outbound) => {
                log::trace!("Connection event: FullyNegotiatedOutbound");
                self.outbound_streams.push_front(outbound.protocol)
            }
            ConnectionEvent::AddressChange(change) => {
                log::trace!("Connection event: address change {}", change.new_address)
            }
            // NOTE: These cases should never occur, because we're using ReadyUpgrade
            ConnectionEvent::DialUpgradeError(_) => log::error!("Dial upgrade error"),
            ConnectionEvent::ListenUpgradeError(_) => log::error!("Listen upgrade error"),
        }
    }
}

pub struct P2PTransportBuilder {
    swarm: Swarm<WorkerBehaviour>,
    bootstrap: bool,
}

impl P2PTransportBuilder {
    pub fn new() -> Result<Self, Error> {
        let keypair = Keypair::generate_ed25519();
        Self::from_keypair(keypair)
    }

    pub fn from_keypair(keypair: Keypair) -> Result<Self, Error> {
        let local_peer_id = PeerId::from(keypair.public());
        log::info!("Local peer ID: {local_peer_id}");

        let protocol = std::str::from_utf8(SUBSQUID_PROTOCOL).unwrap().to_string();
        let identify_cfg = identify::Config::new(protocol, keypair.public())
            .with_interval(Duration::from_secs(60))
            .with_push_listen_addr_updates(true);
        let mut kademlia_cfg = KademliaConfig::default();
        kademlia_cfg.set_protocol_names(vec![SUBSQUID_PROTOCOL.into()]);
        let store = MemoryStore::new(local_peer_id);
        let (relay_transport, relay) = RelayClient::new_transport_and_behaviour(local_peer_id);
        let behaviour = WorkerBehaviour {
            grpc: Default::default(),
            identify: identify::Behaviour::new(identify_cfg),
            kademlia: Kademlia::with_config(local_peer_id, store, kademlia_cfg),
            autonat: autonat::Behaviour::new(local_peer_id, Default::default()),
            relay,
            dcutr: Default::default(),
        };

        let tokio_dns_transport =
            TokioDnsConfig::system(tcp::tokio::Transport::new(tcp::Config::new().nodelay(true)))
                .unwrap();
        let transport = OrTransport::new(relay_transport, tokio_dns_transport)
            .upgrade(upgrade::Version::V1)
            .authenticate(noise::NoiseAuthenticated::xx(&keypair).unwrap())
            .multiplex(YamuxConfig::default())
            .boxed();

        let swarm = Swarm::with_tokio_executor(transport, behaviour, local_peer_id);
        Ok(Self {
            swarm,
            bootstrap: false,
        })
    }

    pub fn listen_on(&mut self, addr: Multiaddr) -> Result<(), Error> {
        log::info!("Listening on {addr}");
        self.swarm.listen_on(addr)?;
        Ok(())
    }

    pub fn dial(&mut self, addr: Multiaddr) -> Result<(), Error> {
        log::info!("Dialing {addr}");
        self.swarm.dial(addr)?;
        Ok(())
    }

    pub fn bootstrap(&mut self) {
        log::info!("Bootstrapping kademlia");
        self.bootstrap = true;
    }

    pub fn add_relay(&mut self, relay_addr: Multiaddr) -> Result<(), Error> {
        log::info!("Adding relay {relay_addr}");
        self.swarm.dial(relay_addr.clone())?;
        self.swarm.listen_on(relay_addr.with(Protocol::P2pCircuit))?;
        Ok(())
    }

    pub fn run(self) -> (impl Stream<Item = Result<P2PConnection, Error>>, P2PConnector) {
        let (inbound_tx, inbound_rx) = mpsc::channel(1024);
        let (requests_tx, requests_rx) = mpsc::channel(1024);
        let transport = P2PTransport::new(inbound_tx, requests_rx, self.swarm, self.bootstrap);

        tokio::task::spawn(transport.run().map_err(|e| log::error!("Transport error: {e:?}")));
        (ReceiverStream::new(inbound_rx), P2PConnector::new(requests_tx))
    }
}

struct P2PTransport {
    inbound_streams_sink: mpsc::Sender<Result<P2PConnection, Error>>,
    request_receiver: Fuse<ReceiverStream<P2PConnectionRequest>>,
    request_callbacks: HashMap<PeerId, VecDeque<P2PConnectionCallback>>,
    pending_queries: BiHashMap<PeerId, QueryId>,
    swarm: Swarm<WorkerBehaviour>,
    bootstrap: bool,
}

impl P2PTransport {
    pub fn new(
        inbound_streams_sink: mpsc::Sender<Result<P2PConnection, Error>>,
        requests_receiver: mpsc::Receiver<P2PConnectionRequest>,
        swarm: Swarm<WorkerBehaviour>,
        bootstrap: bool,
    ) -> Self {
        Self {
            inbound_streams_sink,
            request_receiver: ReceiverStream::new(requests_receiver).fuse(),
            request_callbacks: HashMap::new(),
            pending_queries: BiHashMap::new(),
            swarm,
            bootstrap,
        }
    }

    pub async fn run(mut self) -> Result<(), Error> {
        log::debug!("P2PTransport starting");
        if self.bootstrap {
            self.bootstrap_kademlia().await?;
        }
        loop {
            futures::select! {
                event = self.swarm.select_next_some() => self.handle_swarm_event(event).await?,
                request = self.request_receiver.select_next_some() =>
                    self.handle_connection_request(request)
            }
        }
    }

    async fn bootstrap_kademlia(&mut self) -> Result<(), Error> {
        let mut bootstrap_initiated = false;
        while self.bootstrap {
            // Bootstrap cannot be initiated until there are some peers connected
            if !bootstrap_initiated && self.swarm.behaviour_mut().kademlia.bootstrap().is_ok() {
                log::debug!("Kademlia bootstrap initiated");
                bootstrap_initiated = true;
            }
            let event = self.swarm.select_next_some().await;
            self.handle_swarm_event(event).await?;
        }
        Ok(())
    }

    async fn handle_swarm_event<E: Debug>(
        &mut self,
        event: SwarmEvent<WorkerBehaviourEvent, E>,
    ) -> Result<(), Error> {
        log::debug!("P2PTransport handling swarm event: {event:?}");
        match event {
            SwarmEvent::Behaviour(WorkerBehaviourEvent::Grpc(event)) => {
                self.handle_grpc_event(event).await
            }
            SwarmEvent::Behaviour(WorkerBehaviourEvent::Identify(event)) => {
                self.handle_identify_event(event)
            }
            SwarmEvent::Behaviour(WorkerBehaviourEvent::Kademlia(event)) => {
                self.handle_kademlia_event(event).await
            }
            _ => Ok(()),
        }
    }

    fn handle_identify_event(&mut self, event: identify::Event) -> Result<(), Error> {
        log::debug!("Identify event received: {event:?}");
        let (peer_id, listen_addrs) = match event {
            identify::Event::Received { peer_id, info } => (peer_id, info.listen_addrs),
            _ => return Ok(()),
        };
        let kademlia = &mut self.swarm.behaviour_mut().kademlia;
        for address in listen_addrs {
            kademlia.add_address(&peer_id, address);
        }
        Ok(())
    }

    async fn handle_kademlia_event(&mut self, event: KademliaEvent) -> Result<(), Error> {
        log::debug!("Kademlia event received: {event:?}");
        match event {
            KademliaEvent::OutboundQueryProgressed {
                id,
                result: QueryResult::GetClosestPeers(result),
                ..
            } => self.handle_peer_query(id, result).await,
            KademliaEvent::OutboundQueryProgressed {
                result: QueryResult::Bootstrap(result),
                ..
            } => self.handle_bootstrap_result(result),
            _ => Ok(()),
        }
    }

    async fn handle_peer_query(
        &mut self,
        query_id: QueryId,
        result: GetClosestPeersResult,
    ) -> Result<(), Error> {
        log::debug!("Peer query {query_id:?} result: {result:?}");
        let (peer_id, _) = self
            .pending_queries
            .remove_by_right(&query_id)
            .ok_or(Error::Unexpected("Unknown query"))?;

        if result.is_ok_and(|ok| !ok.peers.is_empty()) {
            log::debug!("Peer query successful: {peer_id}");
            let num_requests = self
                .request_callbacks
                .get(&peer_id)
                .map(|queue| queue.len())
                .unwrap_or_default();
            // Request as many streams as there are pending requests
            for _ in 0..num_requests {
                self.swarm.behaviour_mut().grpc.request_stream(peer_id);
            }
        } else {
            log::debug!("Peer query failed: {peer_id}");
            // Send error to *all* request callbacks - no stream will be opened if peer cannot
            // be found in the network.
            let callbacks = self.request_callbacks.get_mut(&peer_id).unwrap().drain(..);
            for callback in callbacks {
                let _ = callback
                    .call(Err(Error::PeerNotFound(peer_id)))
                    .map_err(|e| log::warn!("Request callback failed: {e:?}"));
            }
        }
        Ok(())
    }

    fn handle_bootstrap_result(&mut self, result: BootstrapResult) -> Result<(), Error> {
        log::debug!("Kademlia bootstrap result: {result:?}");
        result?;
        self.bootstrap = false;
        Ok(())
    }

    async fn handle_grpc_event(&mut self, event: GrpcBehaviourEvent) -> Result<(), Error> {
        log::debug!("GRPC event received: {event:?}");
        match event {
            GrpcBehaviourEvent::OutboundStream(conn) => {
                // We don't want to raise errors if the callback fails.
                // Only missing callback means something is wrong (this should never happen).
                let _ = self
                    .request_callbacks
                    .get_mut(&conn.peer_id)
                    .and_then(|queue| queue.pop_back())
                    .ok_or(Error::Unexpected("No callback for connection"))?
                    .call(Ok(conn))
                    .map_err(|e| log::warn!("Request callback failed: {e:?}"));
            }
            GrpcBehaviourEvent::InboundStream(conn) => {
                // We don't want to raise errors if the receiver end is closed or dropped.
                let _ = self
                    .inbound_streams_sink
                    .send(Ok(conn))
                    .await
                    .map_err(|_| log::warn!("Unhandled inbound stream"));
            }
            GrpcBehaviourEvent::RequestFailed(failure) => {
                let _ = self
                    .request_callbacks
                    .get_mut(&failure.peer_id)
                    .and_then(|queue| queue.pop_back())
                    .ok_or(Error::Unexpected("No callback for connection"))?
                    .call(Err(failure.error))
                    .map_err(|e| log::warn!("Request callback failed: {e:?}"));
            }
        }
        Ok(())
    }

    fn handle_connection_request(&mut self, request: P2PConnectionRequest) {
        log::debug!("P2PTransport handling connection request: {request:?}");
        let P2PConnectionRequest { peer_id, callback } = request;
        self.request_callbacks.entry(peer_id).or_default().push_front(callback);

        // If the peer is reachable, we can request a stream. Otherwise, it needs to be found
        // using Kademlia. However, there's no need to start a new query if one is already
        // in progress. Peers without listen addresses are also reachable, as long as they're
        // connected. Hence the is_connected() check first.
        if self.swarm.is_connected(&peer_id)
            || !self.swarm.behaviour_mut().addresses_of_peer(&peer_id).is_empty()
        {
            self.swarm.behaviour_mut().grpc.request_stream(peer_id);
        } else if !self.pending_queries.contains_left(&peer_id) {
            let query_id = self.swarm.behaviour_mut().kademlia.get_closest_peers(peer_id);
            self.pending_queries.insert(peer_id, query_id);
        }
    }
}

#[derive(Debug)]
struct P2PConnectionCallback {
    sender: oneshot::Sender<Result<P2PConnection, Error>>,
}

impl P2PConnectionCallback {
    pub fn new(sender: oneshot::Sender<Result<P2PConnection, Error>>) -> Self {
        Self { sender }
    }

    pub fn call(self, result: Result<P2PConnection, Error>) -> Result<(), Error> {
        self.sender
            .send(result)
            .map_err(|_| Error::Unexpected("Connection receiver dropped"))
    }
}

#[derive(Debug)]
struct P2PConnectionRequest {
    peer_id: PeerId,
    callback: P2PConnectionCallback,
}

#[derive(Debug, Clone)]
pub struct P2PConnector {
    request_sender: mpsc::Sender<P2PConnectionRequest>,
}

impl P2PConnector {
    fn new(request_sender: mpsc::Sender<P2PConnectionRequest>) -> Self {
        Self { request_sender }
    }

    pub fn request_connection(
        &self,
        peer_id: PeerId,
    ) -> BoxFuture<'static, Result<P2PConnection, Error>> {
        let request_sender = self.request_sender.clone();
        Box::pin(async move {
            let (sender, receiver) = oneshot::channel();
            let callback = P2PConnectionCallback::new(sender);
            request_sender
                .send(P2PConnectionRequest { peer_id, callback })
                .await
                .map_err(|_| Error::Unexpected("Cannot send connection request"))?;
            receiver.await.map_err(|_| Error::Unexpected("Connection callback dropped"))?
        })
    }
}

impl Service<Uri> for P2PConnector {
    type Response = P2PConnection;
    type Error = Error;
    type Future = future::Either<
        future::Ready<Result<P2PConnection, Error>>,
        BoxFuture<'static, Result<P2PConnection, Error>>,
    >;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    /// Expected URI format: 'whatever://<peer_id>'
    fn call(&mut self, req: Uri) -> Self::Future {
        match peer_id_from_uri(req) {
            Err(e) => future::err(e).left_future(),
            Ok(peer_id) => self.request_connection(peer_id).right_future(),
        }
    }
}

fn peer_id_from_uri(uri: Uri) -> Result<PeerId, Error> {
    uri.host()
        .ok_or_else(|| Error::PeerId(uri.to_string()))?
        .parse()
        .map_err(|_| Error::PeerId(uri.to_string()))
}

#[derive(Debug)]
pub struct P2PConnection {
    pub(crate) peer_id: PeerId,
    pub(crate) stream: Compat<NegotiatedSubstream>,
}

impl P2PConnection {
    pub fn new(peer_id: PeerId, stream: NegotiatedSubstream) -> Self {
        let stream = stream.compat();
        Self { peer_id, stream }
    }
}

impl Connected for P2PConnection {
    type ConnectInfo = PeerId;

    fn connect_info(&self) -> Self::ConnectInfo {
        self.peer_id
    }
}

impl tokio::io::AsyncRead for P2PConnection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for P2PConnection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, std::io::Error>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), std::io::Error>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

// This struct wraps byte-based I/O (P2P connection) into a messaged-based one.
// It is responsible for encoding/decoding messages sent to/received from other peers.
struct MessageIO {
    connection: P2PConnection,
}

impl MessageIO {
    pub fn new(connection: P2PConnection) -> Self {
        Self { connection }
    }

    pub async fn write_msg(&mut self, msg: MsgContent) -> Result<(), Error> {
        log::debug!("New message to encode: {}", String::from_utf8_lossy(msg.as_slice()));
        let msg = msg.as_ref().ok_or(Error::NullPointer)?;
        let msg_len = msg.len() as u32;
        self.connection.write_u32(msg_len).await.map_err(Error::MessageWrite)?;
        self.connection.write_all(msg.as_slice()).await.map_err(Error::MessageWrite)
    }

    pub async fn read_msg(&mut self) -> Result<Option<Message>, Error> {
        let msg_len = match self.connection.read_u32().await {
            Ok(msg_len) => msg_len as usize,
            Err(_) => return Ok(None), // Connection closed
        };
        let mut buf = ffi::new_buffer(msg_len);
        self.connection
            .read_exact(buf.as_mut().ok_or(Error::NullPointer)?.as_mut_slice())
            .await
            .map_err(Error::MessageRead)?;
        log::debug!("New message decoded: {}", String::from_utf8_lossy(buf.as_slice()));
        Ok(Some(Message {
            peer_id: self.connection.peer_id,
            content: buf,
        }))
    }

    pub async fn shutdown(&mut self) {
        log::debug!("Shutting down connection");
        self.connection
            .shutdown()
            .await
            .unwrap_or_else(|e| log::error!("Connection shutdown failed: {e:?}"));
    }

    pub fn into_msg_stream(self) -> impl Stream<Item = Message> + Unpin {
        Box::pin(stream::unfold(self, |mut codec| async move {
            match codec.read_msg().await {
                Ok(Some(msg)) => return Some((msg, codec)),
                Err(e) => log::error!("Error reading message: {e:?}"),
                _ => {}
            }
            codec.shutdown().await;
            None
        }))
    }
}

// This struct is responsible for handling outbound messages going to a single peer.
// It initiates new connections if needed and forwards received messages.
struct OutboundPeerHandler {
    msg_receiver: Fuse<ReceiverStream<MsgContent>>,
    connector: P2PConnector,
    peer_id: PeerId,
}

impl OutboundPeerHandler {
    // Spawn a new handler running in the background and return a channel for communication
    pub fn spawn(connector: P2PConnector, peer_id: PeerId) -> mpsc::Sender<MsgContent> {
        let (sender, receiver) = mpsc::channel(1024);
        let handler = Self::new(receiver, connector, peer_id);
        tokio::spawn(handler.run());
        sender
    }

    pub fn new(
        msg_receiver: mpsc::Receiver<MsgContent>,
        connector: P2PConnector,
        peer_id: PeerId,
    ) -> Self {
        Self {
            msg_receiver: ReceiverStream::new(msg_receiver).fuse(),
            connector,
            peer_id,
        }
    }

    pub async fn run(mut self) {
        let peer_id = self.peer_id;
        log::debug!("Handler for peer {peer_id} starting");
        let mut codec = match self.connector.request_connection(peer_id).await {
            Ok(conn) => MessageIO::new(conn),
            Err(e) => return log::error!("Failed to connect to peer {peer_id}: {e:?}"),
        }; // TODO: Retry connecting (?)

        while !self.msg_receiver.is_terminated() {
            let msg = self.msg_receiver.select_next_some().await;
            codec
                .write_msg(msg)
                .await
                .unwrap_or_else(|e| log::error!("Writing message failed: {e:?}"));
            // TODO: Re-connect on error (?)
        }
        codec.shutdown().await
    }
}

// This struct is responsible for routing outbound messages to the appropriate OutboundPeerHandler
pub struct Router {
    msg_receiver: Fuse<ReceiverStream<Message>>,
    peer_channels: HashMap<PeerId, mpsc::Sender<MsgContent>>, // TODO: Limit the number of these (?)
    connector: P2PConnector,
}

impl Router {
    // Spawn a new router running in the background and return a channel for communication
    pub fn spawn(connector: P2PConnector) -> mpsc::Sender<Message> {
        log::debug!("Spawning outbound message router");
        let (sender, receiver) = mpsc::channel(1024);
        let router = Self::new(receiver, connector);
        tokio::spawn(router.run());
        sender
    }

    pub fn new(msg_receiver: mpsc::Receiver<Message>, connector: P2PConnector) -> Self {
        Self {
            msg_receiver: ReceiverStream::new(msg_receiver).fuse(),
            peer_channels: Default::default(),
            connector,
        }
    }

    pub async fn run(mut self) {
        while !self.msg_receiver.is_terminated() {
            let msg = self.msg_receiver.select_next_some().await;
            self.handle_message(msg).await;
        }
    }

    async fn handle_message(&mut self, msg: Message) {
        let Message { peer_id, content } = msg;
        log::debug!("Routing message to peer {peer_id}");
        // Get existing peer handler or spawn a new one, and send the message to it
        self.peer_channels
            .entry(peer_id)
            .or_insert_with(|| OutboundPeerHandler::spawn(self.connector.clone(), peer_id))
            .send(content)
            .await
            .unwrap_or_else(|e| log::error!("Sending message error: {e:?}"));
    }
}

// Transform a stream of incoming connections into a stream of incoming messages.
pub fn read_messages<T>(incoming_conns: T) -> impl Stream<Item = Message>
where
    T: Stream<Item = Result<P2PConnection, Error>> + Send + 'static,
{
    // TODO: Introduce a limit of concurrently handled connections
    incoming_conns
        .filter_map(|res| async move {
            res.map_err(|e| log::error!("Incoming connection error: {e:?}")).ok()
        })
        .flat_map_unordered(None, |conn| MessageIO::new(conn).into_msg_stream())
}
