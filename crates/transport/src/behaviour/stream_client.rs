use std::{
    collections::HashMap,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{
    core::{transport::PortUse, Endpoint},
    swarm::{
        dial_opts::DialOpts, ConnectionDenied, ConnectionId, FromSwarm, NetworkBehaviour,
        THandlerInEvent, ToSwarm,
    },
    Multiaddr, PeerId, StreamProtocol,
};
use libp2p_stream::OpenStreamError;

use crate::util::StreamWithPayload;

#[derive(Debug, Clone, Copy)]
pub struct ClientConfig {
    /// The maximum number of open substreams per peer (default: 3)
    pub max_concurrent_streams: usize,
    /// Timeout applied on dialing the remote and opening a substream (default: 10 sec)
    pub connect_timeout: Duration,
    /// Timeout applied on the entire request-response cycle (default: 60 sec)
    pub request_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_concurrent_streams: 3,
            connect_timeout: Duration::from_secs(10),
            request_timeout: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RequestError {
    #[error(transparent)]
    Timeout(Timeout),
    #[error("the remote supports none of the requested protocols")]
    UnsupportedProtocol,
    #[error("response too large")]
    ResponseTooLarge,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Timeout {
    /// Peer lookup or connection establishing timed out
    #[error("connect timeout")]
    Connect,
    /// The response has not been received in time
    #[error("request timeout")]
    Request,
}

pub struct StreamClientHandle {
    protocol: &'static str,
    config: ClientConfig,
    control: libp2p_stream::Control,
    semaphores: parking_lot::Mutex<HashMap<PeerId, Arc<tokio::sync::Semaphore>>>,
}

impl StreamClientHandle {
    pub async fn get_raw_stream(
        &self,
        peer: PeerId,
    ) -> Result<impl AsyncRead + AsyncWrite, RequestError> {
        let fut = async {
            let permit = self
                .semaphores
                .lock()
                .entry(peer)
                .or_insert_with(|| {
                    Arc::new(tokio::sync::Semaphore::new(self.config.max_concurrent_streams))
                })
                .clone()
                .acquire_owned()
                .await
                .expect("Semaphone should never be closed");

            log::debug!("Opening stream to {}", peer);
            let stream = self
                .control
                .clone()
                .open_stream(peer, StreamProtocol::new(&self.protocol))
                .await?;
            log::debug!("Opened stream to {}", peer);

            Ok(StreamWithPayload::new(stream, permit))
        };
        tokio::time::timeout(self.config.connect_timeout, fut)
            .await
            .unwrap_or_else(|_| {
                log::debug!("Connection to {peer} timed out");
                Err(RequestError::Timeout(Timeout::Connect))
            })
    }

    /// Send request and get a response in a way compatible with the request-server behaviour
    pub async fn request_response(
        &self,
        peer: PeerId,
        request: &[u8],
        max_response_size: u64,
    ) -> Result<Vec<u8>, RequestError> {
        let fut = async {
            let mut stream = self.get_raw_stream(peer).await?;
            stream.write_all(request).await?;
            stream.close().await?;
            log::debug!("Sent {} bytes to {peer}", request.len());

            let mut buf = Vec::new();
            stream.take(max_response_size + 1).read_to_end(&mut buf).await?;
            if buf.len() as u64 > max_response_size {
                return Err(RequestError::ResponseTooLarge);
            }
            log::debug!("Read {} bytes from {peer}", buf.len());
            Ok(buf)
        };
        tokio::time::timeout(self.config.request_timeout, fut)
            .await
            .unwrap_or_else(|_| {
                log::debug!("Request to {peer} timed out");
                Err(RequestError::Timeout(Timeout::Request))
            })
    }
}

pub struct ClientBehaviour {
    inner: libp2p_stream::Behaviour,
}

impl Default for ClientBehaviour {
    fn default() -> Self {
        Self {
            inner: libp2p_stream::Behaviour::new(),
        }
    }
}

impl ClientBehaviour {
    pub fn new_handle(
        &self,
        protocol: &'static str,
        config: ClientConfig,
    ) -> StreamClientHandle {
        let control = self.inner.new_control();
        StreamClientHandle {
            protocol,
            config,
            control,
            semaphores: Default::default(),
        }
    }
}

#[derive(Debug)]
pub enum Event {
    Dial(DialOpts),
}

impl NetworkBehaviour for ClientBehaviour {
    type ConnectionHandler = <libp2p_stream::Behaviour as NetworkBehaviour>::ConnectionHandler;
    type ToSwarm = Event;

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        local_addr: &libp2p::Multiaddr,
        remote_addr: &libp2p::Multiaddr,
    ) -> Result<Self::ConnectionHandler, ConnectionDenied> {
        self.inner.handle_established_inbound_connection(
            connection_id,
            peer,
            local_addr,
            remote_addr,
        )
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: Endpoint,
        port_use: PortUse,
    ) -> Result<Self::ConnectionHandler, ConnectionDenied> {
        self.inner.handle_established_outbound_connection(
            connection_id,
            peer,
            addr,
            role_override,
            port_use,
        )
    }

    fn on_swarm_event(&mut self, event: FromSwarm) {
        self.inner.on_swarm_event(event)
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: ConnectionId,
        event: libp2p::swarm::THandlerOutEvent<Self>,
    ) {
        self.inner.on_connection_handler_event(peer_id, connection_id, event);
    }

    fn poll(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<ToSwarm<Self::ToSwarm, THandlerInEvent<Self>>> {
        match futures::ready!(self.inner.poll(cx)) {
            // Capture an attempt to dial a peer to be handled by the BaseBehaviour
            ToSwarm::Dial { opts } => Poll::Ready(ToSwarm::GenerateEvent(Event::Dial(opts))),
            e => Poll::Ready(e.map_out(|_| unreachable!())),
        }
    }
}

impl From<OpenStreamError> for RequestError {
    fn from(e: OpenStreamError) -> Self {
        match e {
            OpenStreamError::UnsupportedProtocol(_) => RequestError::UnsupportedProtocol,
            OpenStreamError::Io(e) => RequestError::Io(e),
            _ => unreachable!(),
        }
    }
}
