use libp2p::core::upgrade::ReadyUpgrade;
use std::collections::VecDeque;
use std::pin::Pin;
use std::task::{Context, Poll};

use libp2p::core::connection::ConnectionId;
use libp2p::futures::{Stream, StreamExt};
use libp2p::{Multiaddr, PeerId, Swarm};
use libp2p::futures::future::BoxFuture;

use libp2p::identity::Keypair;
use libp2p::swarm::handler::ConnectionEvent;
use libp2p::swarm::{ConnectionHandler, ConnectionHandlerEvent, KeepAlive, NegotiatedSubstream, NetworkBehaviour, NetworkBehaviourAction, NotifyHandler, PollParameters, SubstreamProtocol, SwarmEvent};
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

pub struct GrpcServerBehaviour {
    streams: VecDeque<P2PConnection>,
    requests: VecDeque<PeerId>,
}

impl GrpcServerBehaviour {
    pub fn new() -> Self {
        Self {
            streams: VecDeque::new(),
            requests: VecDeque::new(),
        }
    }

    pub fn request_stream(&mut self, peer_id: PeerId) {
        self.requests.push_front(peer_id);
    }
}

impl NetworkBehaviour for GrpcServerBehaviour {
    type ConnectionHandler = GrpcServerHandler;
    type OutEvent = P2PConnection;

    fn new_handler(&mut self) -> Self::ConnectionHandler {
        log::debug!("New connection handler");
        Self::ConnectionHandler::new()
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        _connection_id: ConnectionId,
        stream: NegotiatedSubstream,
    ) {
        log::debug!("Connection handler event {peer_id:?}");
        self.streams
            .push_front(P2PConnection::new(peer_id, stream))
    }

    fn poll(
        &mut self,
        _cx: &mut Context<'_>,
        _params: &mut impl PollParameters,
    ) -> Poll<NetworkBehaviourAction<Self::OutEvent, Self::ConnectionHandler>> {
        log::debug!("Poll network behaviour");
        if let Some(stream) = self.streams.pop_back() {
            log::debug!("Behaviour yielding substream");
            return Poll::Ready(NetworkBehaviourAction::GenerateEvent(stream))
        }
        if let Some(peer_id) = self.requests.pop_back() {
            if self.addresses_of_peer(&peer_id).is_empty() {
                log::debug!("Peer not connected: {peer_id}");
                self.requests.push_front(peer_id)
            }
            else {
                log::debug!("Behaviour requesting substream for {peer_id}");
                return Poll::Ready(NetworkBehaviourAction::NotifyHandler {
                    peer_id,
                    handler: NotifyHandler::Any,
                    event: (),
                })
            }
        }
        Poll::Pending
    }
}

pub struct GrpcServerHandler {
    streams: VecDeque<NegotiatedSubstream>,
    requested: usize,
}

impl GrpcServerHandler {
    pub fn new() -> Self {
        Self {
            streams: VecDeque::new(),
            requested: 0,
        }
    }
}

impl ConnectionHandler for GrpcServerHandler {
    type InEvent = ();
    type OutEvent = NegotiatedSubstream;
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
        if let Some(stream) = self.streams.pop_back() {
            log::debug!("Connection handler yielding substream");
            return Poll::Ready(ConnectionHandlerEvent::Custom(stream))
        }
        if self.requested > 0 {
            log::debug!("Connection handler requesting substream");
            self.requested -= 1;
            let protocol = SubstreamProtocol::new(ReadyUpgrade::new(PROTOCOL_NAME), ());
            return Poll::Ready(ConnectionHandlerEvent::OutboundSubstreamRequest { protocol })
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
                self.streams.push_front(inbound.protocol)
            }
            ConnectionEvent::FullyNegotiatedOutbound(outbound) => {
                log::debug!("Connection event: FullyNegotiatedOutbound");
                self.streams.push_front(outbound.protocol)
            }
            ConnectionEvent::AddressChange(change) => log::debug!("Address change: {}", change.new_address),
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
    swarm: Swarm<GrpcServerBehaviour>,
}

impl P2PTransport {
    pub fn new(keypair: Keypair) -> Self {
        let local_peer_id = PeerId::from(keypair.public());
        log::info!("Local peer ID: {local_peer_id}");
        let transport = libp2p::tokio_development_transport(keypair).unwrap();
        let behaviour = GrpcServerBehaviour::new();
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

    pub fn run(self) -> impl Stream<Item=Result<P2PConnection, Error>> {
        let (sender, receiver) = tokio::sync::mpsc::channel(1024);
        let mut swarm = self.swarm;
        tokio::task::spawn(async move {
            log::debug!("P2P Transport run");
            loop {
                let swarm_event = swarm.select_next_some().await;
                match swarm_event {
                    SwarmEvent::Behaviour(conn) => {
                        log::debug!("New connection ready");
                        if let Err(_) = sender.send(Ok(conn)).await {
                            break
                        }
                    },
                    SwarmEvent::ConnectionEstablished { peer_id, .. } => log::debug!("Connection established with {peer_id}"),
                    SwarmEvent::ConnectionClosed { peer_id, .. } => log::debug!("Connection closed with {peer_id}"),
                    SwarmEvent::IncomingConnection { .. } => log::debug!("Connection incoming"),
                    SwarmEvent::IncomingConnectionError { error, .. } => log::error!("Incoming connection error: {error}"),
                    SwarmEvent::OutgoingConnectionError { error, .. } => log::error!("Outgoing connection error: {error}"),
                    SwarmEvent::BannedPeer { peer_id, .. } => log::debug!("Banned peer: {peer_id}"),
                    SwarmEvent::NewListenAddr { address, .. } => log::info!("Listening on {address}"),
                    SwarmEvent::ExpiredListenAddr { address, .. } => log::info!("Expired listen address: {address}"),
                    SwarmEvent::ListenerClosed { .. } => log::debug!("Listener closed"),
                    SwarmEvent::ListenerError { error, .. } => log::debug!("Listener error: {error}"),
                    SwarmEvent::Dialing(peer_id) => log::info!("Dialing {peer_id}"),
                }
            }
        });
        ReceiverStream::new(receiver)
    }
}

pub struct P2PConnector {
    transport: Option<P2PTransport>
}

impl P2PConnector {
    pub fn new(transport: P2PTransport) -> Self
    {
        Self {
            transport: Some(transport)
        }
    }
}

impl Service<Uri> for P2PConnector {
    type Response = P2PConnection;
    type Error = Error;
    type Future = BoxFuture<'static, Result<P2PConnection, Error>>;

    fn poll_ready(&mut self, _cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        log::debug!("P2PConnector poll ready");
        match self.transport {
            Some(_) => Poll::Ready(Ok(())),
            None => Poll::Pending
        }
    }

    fn call(&mut self, req: Uri) -> Self::Future {
        // FIXME: This is totally broken and will work only once

        let peer_id: PeerId = req.host().unwrap().parse().unwrap();
        log::debug!("P2PConnector call: {}", peer_id);
        let mut transport = self.transport.take().unwrap();
        transport.request_stream(peer_id);
        let mut stream = transport.run();
        Box::pin(async move {
            stream.next().await.unwrap()
        })
    }
}


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
