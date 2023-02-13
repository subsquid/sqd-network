use std::collections::{HashMap, HashSet, VecDeque};
use std::pin::Pin;
use std::task::{Context, Poll};

use libp2p::core::connection::ConnectionId;
use libp2p::core::upgrade::ReadyUpgrade;
use libp2p::futures::future::BoxFuture;
use libp2p::futures::{Stream, StreamExt};
use libp2p::identity::Keypair;
use libp2p::swarm::behaviour::{ConnectionClosed, ConnectionEstablished, FromSwarm};
use libp2p::swarm::handler::ConnectionEvent;
use libp2p::swarm::{
    ConnectionHandler, ConnectionHandlerEvent, KeepAlive, NegotiatedSubstream, NetworkBehaviour,
    NetworkBehaviourAction, NotifyHandler, PollParameters, SubstreamProtocol, SwarmEvent,
};
use libp2p::{Multiaddr, PeerId, Swarm};

use tokio::io::ReadBuf;
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::compat::FuturesAsyncReadCompatExt;

use tonic::transport::server::Connected;
use tonic::transport::Uri;

use tower::Service;

pub mod worker_rpc {
    tonic::include_proto!("worker_rpc"); // The string specified here must match the proto package name
}

pub const PROTOCOL_NAME: &[u8] = b"/grpc/0.0.1";

#[derive(Default)]
pub struct GrpcBehaviour {
    inbound_streams: VecDeque<P2PConnection>,
    outbound_streams: VecDeque<P2PConnection>,
    conn_requests: VecDeque<PeerId>,
    connected_peers: HashSet<PeerId>,
}

impl GrpcBehaviour {
    pub fn request_stream(&mut self, peer_id: PeerId) {
        self.conn_requests.push_front(peer_id);
    }
}

pub enum GrpcBehaviourEvent {
    InboundStream(P2PConnection),
    OutboundStream(P2PConnection),
}

impl NetworkBehaviour for GrpcBehaviour {
    type ConnectionHandler = GrpcConnectionHandler;
    type OutEvent = GrpcBehaviourEvent;

    fn new_handler(&mut self) -> Self::ConnectionHandler {
        log::debug!("New connection handler");
        Default::default()
    }

    fn on_swarm_event(&mut self, event: FromSwarm<Self::ConnectionHandler>) {
        match event {
            FromSwarm::ConnectionEstablished(ConnectionEstablished { peer_id, .. }) => {
                log::debug!("Connection established with {peer_id}");
                self.connected_peers.insert(peer_id);
            }
            FromSwarm::ConnectionClosed(ConnectionClosed { peer_id, .. }) => {
                log::debug!("Connection with {peer_id} closed");
                self.connected_peers.remove(&peer_id);
            }
            FromSwarm::AddressChange(_) => {}
            FromSwarm::DialFailure(_) => {}
            FromSwarm::ListenFailure(_) => {}
            FromSwarm::NewListener(_) => {}
            FromSwarm::NewListenAddr(_) => {}
            FromSwarm::ExpiredListenAddr(_) => {}
            FromSwarm::ListenerError(_) => {}
            FromSwarm::ListenerClosed(_) => {}
            FromSwarm::NewExternalAddr(_) => {}
            FromSwarm::ExpiredExternalAddr(_) => {}
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
        log::debug!("Poll network behaviour");
        if let Some(stream) = self.inbound_streams.pop_back() {
            log::debug!("Behaviour yielding inbound substream");
            return Poll::Ready(NetworkBehaviourAction::GenerateEvent(
                GrpcBehaviourEvent::InboundStream(stream),
            ));
        }
        if let Some(stream) = self.outbound_streams.pop_back() {
            log::debug!("Behaviour yielding outbound substream");
            return Poll::Ready(NetworkBehaviourAction::GenerateEvent(
                GrpcBehaviourEvent::OutboundStream(stream),
            ));
        }
        if let Some(peer_id) = self.conn_requests.pop_back() {
            if self.connected_peers.contains(&peer_id) {
                log::debug!("Behaviour requesting substream for {peer_id}");
                return Poll::Ready(NetworkBehaviourAction::NotifyHandler {
                    peer_id,
                    handler: NotifyHandler::Any,
                    event: RequestStream {},
                });
            } else {
                log::debug!("Peer not connected: {peer_id}");
                self.conn_requests.push_front(peer_id)
            }
        }
        Poll::Pending
    }
}

#[derive(Default)]
pub struct GrpcConnectionHandler {
    inbound_streams: VecDeque<NegotiatedSubstream>,
    outbound_streams: VecDeque<NegotiatedSubstream>,
    requested: usize,
}

#[derive(Debug)]
pub struct RequestStream {}

#[derive(Debug)]
pub enum GrpcHandlerEvent {
    InboundStream(NegotiatedSubstream),
    OutboundStream(NegotiatedSubstream),
}

impl ConnectionHandler for GrpcConnectionHandler {
    type InEvent = RequestStream;
    type OutEvent = GrpcHandlerEvent;
    type Error = Error;
    type InboundProtocol = ReadyUpgrade<&'static [u8]>;
    type OutboundProtocol = ReadyUpgrade<&'static [u8]>;
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
        log::debug!("Poll connection handler");
        if let Some(stream) = self.inbound_streams.pop_back() {
            log::debug!("Connection handler yielding inbound substream");
            return Poll::Ready(ConnectionHandlerEvent::Custom(
                GrpcHandlerEvent::InboundStream(stream),
            ));
        }
        if let Some(stream) = self.outbound_streams.pop_back() {
            log::debug!("Connection handler yielding outbound substream");
            return Poll::Ready(ConnectionHandlerEvent::Custom(
                GrpcHandlerEvent::OutboundStream(stream),
            ));
        }
        if self.requested > 0 {
            log::debug!("Connection handler requesting substream");
            self.requested -= 1;
            let protocol = SubstreamProtocol::new(ReadyUpgrade::new(PROTOCOL_NAME), ());
            return Poll::Ready(ConnectionHandlerEvent::OutboundSubstreamRequest { protocol });
        }
        Poll::Pending
    }

    fn on_behaviour_event(&mut self, _event: Self::InEvent) {
        log::debug!("New substream requested");
        self.requested += 1;
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
                log::debug!("Connection event: FullyNegotiatedInbound");
                self.inbound_streams.push_front(inbound.protocol)
            }
            ConnectionEvent::FullyNegotiatedOutbound(outbound) => {
                log::debug!("Connection event: FullyNegotiatedOutbound");
                self.outbound_streams.push_front(outbound.protocol)
            }
            ConnectionEvent::AddressChange(change) => {
                log::debug!("Address change: {}", change.new_address)
            }
            ConnectionEvent::DialUpgradeError(_) => log::error!("Dial upgrade error"),
            ConnectionEvent::ListenUpgradeError(_) => log::error!("Listen upgrade error"),
        }
    }
}

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("unexpected error happened")]
    Unexpected,
}

pub struct P2PTransport {
    swarm: Swarm<GrpcBehaviour>,
}

impl P2PTransport {
    pub fn new(keypair: Keypair) -> Self {
        let local_peer_id = PeerId::from(keypair.public());
        log::info!("Local peer ID: {local_peer_id}");
        let transport = libp2p::tokio_development_transport(keypair).unwrap();
        let behaviour = GrpcBehaviour::default();
        let swarm = Swarm::with_tokio_executor(transport, behaviour, local_peer_id);
        Self { swarm }
    }

    pub fn listen_on(&mut self, addr: Multiaddr) {
        self.swarm.listen_on(addr).unwrap();
    }

    pub fn dial(&mut self, addr: Multiaddr) {
        self.swarm.dial(addr).unwrap();
    }

    pub fn request_stream(&mut self, peer_id: PeerId) {
        self.swarm.behaviour_mut().request_stream(peer_id)
    }

    pub fn run(
        self,
    ) -> (
        impl Stream<Item = Result<P2PConnection, Error>>,
        tokio::sync::mpsc::Sender<(
            PeerId,
            tokio::sync::oneshot::Sender<Result<P2PConnection, Error>>,
        )>,
    ) {
        let (inbound_tx, inbound_rx) = tokio::sync::mpsc::channel(1024);
        let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(1024);
        let mut swarm = self.swarm;

        tokio::task::spawn(async move {
            log::debug!("P2P Transport run");
            let mut outbound_callback_map: HashMap<
                PeerId,
                tokio::sync::oneshot::Sender<Result<P2PConnection, Error>>,
            > = HashMap::new();
            loop {
                let swarm_event = tokio::select! {
                    event = swarm.select_next_some() => {
                        log::debug!("Swarm event");
                        event
                    },
                    outbound_request = outbound_rx.recv() => {
                        log::debug!("Outbound request");
                        if let Some((peer_id, callback)) = outbound_request {
                            swarm.behaviour_mut().request_stream(peer_id);
                            outbound_callback_map.insert(peer_id, callback);
                        }
                        continue
                    }
                };
                match swarm_event {
                    SwarmEvent::Behaviour(event) => {
                        log::debug!("New connection ready");
                        match event {
                            GrpcBehaviourEvent::OutboundStream(conn) => {
                                let callback = outbound_callback_map.remove(&conn.peer_id).unwrap();
                                if let Err(_) = callback.send(Ok(conn)) {
                                    break;
                                }
                            }
                            GrpcBehaviourEvent::InboundStream(conn) => {
                                if let Err(_) = inbound_tx.send(Ok(conn)).await {
                                    break;
                                }
                            }
                        }
                    }
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                        log::debug!("Connection established with {peer_id}")
                    }
                    SwarmEvent::ConnectionClosed { peer_id, .. } => {
                        log::debug!("Connection closed with {peer_id}")
                    }
                    SwarmEvent::IncomingConnection { .. } => log::debug!("Connection incoming"),
                    SwarmEvent::IncomingConnectionError { error, .. } => {
                        log::error!("Incoming connection error: {error}")
                    }
                    SwarmEvent::OutgoingConnectionError { error, .. } => {
                        log::error!("Outgoing connection error: {error}")
                    }
                    SwarmEvent::BannedPeer { peer_id, .. } => log::debug!("Banned peer: {peer_id}"),
                    SwarmEvent::NewListenAddr { address, .. } => {
                        log::info!("Listening on {address}")
                    }
                    SwarmEvent::ExpiredListenAddr { address, .. } => {
                        log::debug!("Expired listen address: {address}")
                    }
                    SwarmEvent::ListenerClosed { .. } => log::debug!("Listener closed"),
                    SwarmEvent::ListenerError { error, .. } => {
                        log::debug!("Listener error: {error}")
                    }
                    SwarmEvent::Dialing(peer_id) => log::info!("Dialing {peer_id}"),
                }
            }
        });
        (ReceiverStream::new(inbound_rx), outbound_tx)
    }
}

pub struct P2PConnector {
    outbound_requests_sender: tokio::sync::mpsc::Sender<(
        PeerId,
        tokio::sync::oneshot::Sender<Result<P2PConnection, Error>>,
    )>,
}

impl P2PConnector {
    pub fn new(
        outbound_requests_sender: tokio::sync::mpsc::Sender<(
            PeerId,
            tokio::sync::oneshot::Sender<Result<P2PConnection, Error>>,
        )>,
    ) -> Self {
        Self {
            outbound_requests_sender,
        }
    }
}

impl Service<Uri> for P2PConnector {
    type Response = P2PConnection;
    type Error = Error;
    type Future = BoxFuture<'static, Result<P2PConnection, Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        log::debug!("P2PConnector poll ready");
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        let peer_id: PeerId = req.host().unwrap().parse().unwrap();
        log::debug!("P2PConnector call: {}", peer_id);
        let (tx, rx) = tokio::sync::oneshot::channel();
        let sender = self.outbound_requests_sender.clone();
        Box::pin(async move {
            sender.send((peer_id, tx)).await.unwrap();
            rx.await.unwrap()
        })
    }
}

#[derive(Debug)]
pub struct P2PConnection {
    peer_id: PeerId,
    stream: tokio_util::compat::Compat<NegotiatedSubstream>,
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
