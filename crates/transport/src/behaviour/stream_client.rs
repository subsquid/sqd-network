use std::{collections::HashMap, sync::Arc, time::Duration};

use futures::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use libp2p::{
    swarm::{dial_opts::DialOpts, NetworkBehaviour, THandlerInEvent, ToSwarm},
    PeerId, StreamProtocol,
};
use libp2p_stream::OpenStreamError;

use crate::{util::StreamWithPayload, BehaviourWrapper};

use super::wrapped::TToSwarm;

#[derive(Debug, Clone, Copy)]
pub struct ClientConfig {
    /// The maximum number of open substreams per peer (default: 3)
    pub max_concurrent_streams: Option<usize>,
    /// The maximum length of the response message read in the [`request_response`] call (default: 1 KiB)
    pub max_response_size: u64,
    /// Timeout applied on dialing the remote and opening a substream (default: 10 sec)
    pub connect_timeout: Duration,
    /// Timeout applied on the entire request-response cycle (default: 60 sec)
    pub request_timeout: Duration,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            max_concurrent_streams: Some(3),
            max_response_size: 1024,
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
    pub(crate) config: ClientConfig,
    control: libp2p_stream::Control,
    semaphores: parking_lot::Mutex<HashMap<PeerId, Arc<tokio::sync::Semaphore>>>,
}

impl StreamClientHandle {
    pub async fn get_raw_stream(
        &self,
        peer: PeerId,
    ) -> Result<impl AsyncRead + AsyncWrite, RequestError> {
        let semaphore = self.config.max_concurrent_streams.map(|limit| {
            self.semaphores
                .lock()
                .entry(peer)
                .or_insert_with(|| Arc::new(tokio::sync::Semaphore::new(limit)))
                .clone()
        });

        let fut = async {
            let permit = if let Some(s) = semaphore {
                Some(s.acquire_owned().await.expect("Semaphone should never be closed"))
            } else {
                None
            };

            log::debug!("Opening stream to {}", peer);
            let stream = self
                .control
                .clone()
                .open_stream(peer, StreamProtocol::new(self.protocol))
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
    ) -> Result<Vec<u8>, RequestError> {
        let fut = async {
            let mut stream = self.get_raw_stream(peer).await?;
            stream.write_all(request).await?;
            stream.close().await?;
            log::debug!("Sent {} bytes to {peer}", request.len());

            let mut buf = Vec::new();
            stream.take(self.config.max_response_size + 1).read_to_end(&mut buf).await?;
            if buf.len() as u64 > self.config.max_response_size {
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

impl Clone for StreamClientHandle {
    fn clone(&self) -> Self {
        Self {
            config: self.config,
            control: self.control.clone(),
            protocol: self.protocol,
            semaphores: Default::default(),
        }
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
    pub fn new_handle(&self, protocol: &'static str, config: ClientConfig) -> StreamClientHandle {
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

impl BehaviourWrapper for ClientBehaviour {
    type Event = Event;
    type Inner = libp2p_stream::Behaviour;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn on_inner_command(
        &mut self,
        ev: ToSwarm<<Self::Inner as NetworkBehaviour>::ToSwarm, THandlerInEvent<Self::Inner>>,
    ) -> impl IntoIterator<Item = TToSwarm<Self>> {
        match ev {
            // Capture an attempt to dial a peer to be handled by the BaseBehaviour
            ToSwarm::Dial { opts } => Some(ToSwarm::GenerateEvent(Event::Dial(opts))),
            e => Some(e.map_out(|()| unreachable!("Stream behaviour doesn't produce events"))),
        }
    }
}

impl From<OpenStreamError> for RequestError {
    fn from(e: OpenStreamError) -> Self {
        match e {
            OpenStreamError::UnsupportedProtocol(_) => Self::UnsupportedProtocol,
            OpenStreamError::Io(e) => Self::Io(e),
            _ => unreachable!(),
        }
    }
}
