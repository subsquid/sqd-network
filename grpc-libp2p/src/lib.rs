use std::{
    collections::{HashMap, HashSet, VecDeque},
    pin::Pin,
    task::{Context, Poll},
};

use futures::{
    future::BoxFuture,
    stream::{Fuse, Stream, StreamExt},
};

use libp2p::{
    core::{connection::ConnectionId, upgrade::ReadyUpgrade},
    identity::Keypair,
    swarm::{
        behaviour::{ConnectionClosed, ConnectionEstablished, FromSwarm},
        handler::ConnectionEvent,
        ConnectionHandler, ConnectionHandlerEvent, DialError, KeepAlive, NegotiatedSubstream,
        NetworkBehaviour, NetworkBehaviourAction, NotifyHandler, PollParameters, SubstreamProtocol,
        SwarmEvent,
    },
    Multiaddr, PeerId, Swarm, TransportError,
};

use tokio::{
    io::ReadBuf,
    sync::{mpsc, oneshot},
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};

use tonic::transport::{server::Connected, Uri};

use tower::Service;

pub mod worker_rpc {
    tonic::include_proto!("worker_rpc"); // The string specified here must match the proto package name
}

pub const PROTOCOL_NAME: &[u8] = b"/grpc/0.0.1";

#[derive(Default)]
pub struct GrpcBehaviour {
    inbound_streams: VecDeque<P2PConnection>,
    outbound_streams: VecDeque<P2PConnection>,
    stream_requests: VecDeque<PeerId>,
    connected_peers: HashSet<PeerId>,
}

impl GrpcBehaviour {
    pub fn request_stream(&mut self, peer_id: PeerId) {
        self.stream_requests.push_front(peer_id);
    }
}

#[derive(Debug)]
pub enum GrpcBehaviourEvent {
    InboundStream(P2PConnection),
    OutboundStream(P2PConnection),
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
            GrpcHandlerEvent::InboundStream(stream) => self
                .inbound_streams
                .push_front(P2PConnection::new(peer_id, stream)),
            GrpcHandlerEvent::OutboundStream(stream) => self
                .outbound_streams
                .push_front(P2PConnection::new(peer_id, stream)),
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
        if let Some(peer_id) = self.stream_requests.pop_back() {
            if self.connected_peers.contains(&peer_id) {
                log::trace!("GrpcBehaviour: requesting substream for {peer_id}");
                return Poll::Ready(NetworkBehaviourAction::NotifyHandler {
                    peer_id,
                    handler: NotifyHandler::Any,
                    event: RequestStream {},
                });
            } else {
                log::trace!("GrpcBehaviour: peer not connected: {peer_id}");
                self.stream_requests.push_front(peer_id)
            }
        }
        Poll::Pending
    }
}

#[derive(Default)]
pub struct GrpcConnectionHandler {
    inbound_streams: VecDeque<NegotiatedSubstream>,
    outbound_streams: VecDeque<NegotiatedSubstream>,
    requested_streams: usize,
}

#[derive(Debug)]
pub struct RequestStream {}

type GrpcProtocol = ReadyUpgrade<&'static [u8]>;

#[derive(Debug)]
pub enum GrpcHandlerEvent {
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
        SubstreamProtocol::new(ReadyUpgrade::new(PROTOCOL_NAME), ())
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
            let protocol = SubstreamProtocol::new(ReadyUpgrade::new(PROTOCOL_NAME), ());
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

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Libp2p transport creation failed")]
    Transport,
    #[error("Listening failed: {0:?}")]
    Listen(#[from] TransportError<std::io::Error>),
    #[error("Dialing failed: {0:?}")]
    Dial(#[from] DialError),
    #[error("Connection receiver dropped")]
    ReceiverDropped,
    #[error("No callback for outbound connection")]
    NoCallback,
}

pub struct P2PTransportBuilder {
    swarm: Swarm<GrpcBehaviour>,
}

impl P2PTransportBuilder {
    pub fn new() -> Result<Self, Error> {
        let keypair = Keypair::generate_ed25519();
        Self::from_keypair(keypair)
    }

    pub fn from_keypair(keypair: Keypair) -> Result<Self, Error> {
        let local_peer_id = PeerId::from(keypair.public());
        log::info!("Local peer ID: {local_peer_id}");
        let transport =
            libp2p::tokio_development_transport(keypair).map_err(|_| Error::Transport)?;
        let behaviour = GrpcBehaviour::default();
        let swarm = Swarm::with_tokio_executor(transport, behaviour, local_peer_id);
        Ok(Self { swarm })
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

    pub fn run(self) -> (impl Stream<Item = Result<P2PConnection, Error>>, P2PConnector) {
        let (inbound_tx, inbound_rx) = mpsc::channel(1024);
        let (requests_tx, requests_rx) = mpsc::channel(1024);
        let transport = P2PTransport::new(inbound_tx, requests_rx, self.swarm);

        tokio::task::spawn(transport.run());
        (ReceiverStream::new(inbound_rx), P2PConnector::new(requests_tx))
    }
}

struct P2PTransport {
    inbound_streams_sink: mpsc::Sender<Result<P2PConnection, Error>>,
    request_receiver: Fuse<ReceiverStream<P2PConnectionRequest>>,
    request_callbacks: HashMap<PeerId, P2PConnectionCallback>,
    swarm: Swarm<GrpcBehaviour>,
}

impl P2PTransport {
    pub fn new(
        inbound_streams_sink: mpsc::Sender<Result<P2PConnection, Error>>,
        requests_receiver: mpsc::Receiver<P2PConnectionRequest>,
        swarm: Swarm<GrpcBehaviour>,
    ) -> Self {
        let request_callbacks = HashMap::new();
        let request_receiver = ReceiverStream::new(requests_receiver).fuse();
        Self {
            inbound_streams_sink,
            request_receiver,
            request_callbacks,
            swarm,
        }
    }

    pub async fn run(mut self) -> Result<(), Error> {
        log::debug!("P2PTransport starting");
        loop {
            futures::select! {
                event = self.swarm.select_next_some() => self.handle_swarm_event(event).await?,
                request = self.request_receiver.select_next_some() =>
                    self.handle_connection_request(request)
            }
        }
    }

    async fn handle_swarm_event(
        &mut self,
        event: SwarmEvent<GrpcBehaviourEvent, Error>,
    ) -> Result<(), Error> {
        log::trace!("P2PTransport handling swarm event: {event:?}");
        let event = match event {
            SwarmEvent::Behaviour(event) => event,
            _ => return Ok(()),
        };
        match event {
            GrpcBehaviourEvent::OutboundStream(conn) => {
                self.request_callbacks
                    .remove(&conn.peer_id)
                    .ok_or(Error::NoCallback)?
                    .call(Ok(conn))?;
            }
            GrpcBehaviourEvent::InboundStream(conn) => {
                // We don't want to raise errors if the receiver end is closed or dropped.
                let _ = self
                    .inbound_streams_sink
                    .send(Ok(conn))
                    .await
                    .map_err(|_| log::warn!("Unhandled inbound stream"));
            }
        }
        Ok(())
    }

    fn handle_connection_request(&mut self, request: P2PConnectionRequest) {
        log::debug!("P2PTransport handling connection request: {request:?}");
        let P2PConnectionRequest { peer_id, callback } = request;
        self.swarm.behaviour_mut().request_stream(peer_id);
        self.request_callbacks.insert(peer_id, callback);
    }
}

#[derive(Debug)]
pub struct P2PConnectionCallback {
    sender: oneshot::Sender<Result<P2PConnection, Error>>,
}

impl P2PConnectionCallback {
    pub fn new(sender: oneshot::Sender<Result<P2PConnection, Error>>) -> Self {
        Self { sender }
    }

    pub fn call(self, result: Result<P2PConnection, Error>) -> Result<(), Error> {
        self.sender.send(result).map_err(|_| Error::ReceiverDropped)
    }
}

#[derive(Debug)]
pub struct P2PConnectionRequest {
    pub peer_id: PeerId,
    pub callback: P2PConnectionCallback,
}

#[derive(Debug)]
pub struct P2PConnector {
    request_sender: mpsc::Sender<P2PConnectionRequest>,
}

impl P2PConnector {
    pub fn new(request_sender: mpsc::Sender<P2PConnectionRequest>) -> Self {
        Self { request_sender }
    }
}

impl Service<Uri> for P2PConnector {
    type Response = P2PConnection;
    type Error = Error;
    type Future = BoxFuture<'static, Result<P2PConnection, Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        Poll::Ready(Ok(()))
    }

    /// Expected URI format: 'whatever://<peer_id>'
    fn call(&mut self, req: Uri) -> Self::Future {
        let request_sender = self.request_sender.clone();
        Box::pin(async move {
            let peer_id: PeerId = req.host().unwrap().parse().unwrap();
            let (sender, receiver) = oneshot::channel();
            let callback = P2PConnectionCallback::new(sender);
            request_sender
                .send(P2PConnectionRequest { peer_id, callback })
                .await
                .unwrap();
            receiver.await.unwrap()
        })
    }
}

#[derive(Debug)]
pub struct P2PConnection {
    peer_id: PeerId,
    stream: Compat<NegotiatedSubstream>,
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
