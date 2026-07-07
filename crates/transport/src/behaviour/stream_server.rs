use std::{task::Poll, time::Duration};

use derivative::Derivative;
use futures::{AsyncReadExt, AsyncWriteExt, StreamExt};
use libp2p::{swarm::ToSwarm, PeerId, Stream, StreamProtocol};
use prost::Message;
use tokio::sync::mpsc;

use crate::behaviour::wrapped::{BehaviourWrapper, TToSwarm};

#[derive(Debug, Clone, Copy)]
pub struct ServerConfig {
    /// The maximum length of the request message read from the stream.
    pub max_request_size: u64,
    /// The maximum length of the response message written to the stream.
    pub max_response_size: u64,
    /// Timeout applied on reading the request off the stream.
    pub read_timeout: Duration,
    /// Timeout applied on writing the response back (see [`ResponseSender::send`]).
    pub write_timeout: Duration,
    /// The number of inbound streams buffered while the behaviour is not polled (default: 128).
    ///
    /// Streams arriving while the buffer is full are dropped, mirroring the previous
    /// `request_response` behaviour of dropping requests once at capacity.
    pub incoming_capacity: usize,
    /// The number of parsed requests buffered between the reading tasks and the swarm poll.
    pub requests_queue_size: usize,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            max_request_size: 1024,
            max_response_size: 1024,
            read_timeout: Duration::from_secs(10),
            write_timeout: Duration::from_secs(60),
            incoming_capacity: 128,
            requests_queue_size: 128,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResponseError {
    #[error("response timeout")]
    Timeout,
    #[error("response too large")]
    ResponseTooLarge,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// A handle for sending the response to a single inbound request.
///
/// The handle owns the stream directly, so the response is written without any interaction with
/// the swarm. Dropping it without calling [`ResponseSender::send`] ends the stream without writing
/// a response: depending on the transport the client observes either a stream reset (an I/O error,
/// e.g. RESET_STREAM on QUIC) or an empty response. Either way the request does not succeed,
/// matching an omitted `request_response` response; callers treat both as an error.
pub struct ResponseSender {
    stream: Stream,
    max_response_size: u64,
    write_timeout: Duration,
}

impl std::fmt::Debug for ResponseSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResponseSender")
            .field("max_response_size", &self.max_response_size)
            .field("write_timeout", &self.write_timeout)
            .finish_non_exhaustive()
    }
}

impl ResponseSender {
    /// Write `data` as the response and close the stream.
    pub async fn send(mut self, data: &[u8]) -> Result<(), ResponseError> {
        if data.len() as u64 > self.max_response_size {
            return Err(ResponseError::ResponseTooLarge);
        }
        let fut = async {
            self.stream.write_all(data).await?;
            self.stream.close().await?;
            Ok(())
        };
        tokio::time::timeout(self.write_timeout, fut)
            .await
            .unwrap_or(Err(ResponseError::Timeout))
    }
}

#[derive(Derivative)]
#[derivative(Debug)]
pub struct Request<T> {
    pub peer_id: PeerId,
    #[derivative(Debug = "ignore")]
    pub request: T,
    #[derivative(Debug = "ignore")]
    pub response_sender: ResponseSender,
}

pub struct ServerBehaviour<T> {
    inner: libp2p_stream::Behaviour,
    incoming: libp2p_stream::IncomingStreams,
    protocol: &'static str,
    config: ServerConfig,
    requests_tx: mpsc::Sender<Request<T>>,
    requests_rx: mpsc::Receiver<Request<T>>,
}

impl<T: Message + Default + 'static> ServerBehaviour<T> {
    pub fn new(protocol: &'static str, config: ServerConfig) -> Self {
        let inner = libp2p_stream::Behaviour::new();
        let mut control = inner.new_control();
        let incoming = control
            .accept_with_capacity(StreamProtocol::new(protocol), config.incoming_capacity)
            .expect("stream server listener should not already exist");
        let (requests_tx, requests_rx) = mpsc::channel(config.requests_queue_size);
        Self {
            inner,
            incoming,
            protocol,
            config,
            requests_tx,
            requests_rx,
        }
    }

    fn spawn_read_task(&self, peer: PeerId, stream: Stream) {
        let protocol = self.protocol;
        let config = self.config;
        let requests_tx = self.requests_tx.clone();
        tokio::spawn(read_request(peer, stream, protocol, config, requests_tx));
    }
}

/// Read and decode the request message, keeping the decoding work off the swarm event loop.
/// Returning early drops the stream, which resets it, so the client observes a failed request.
async fn read_request<T: Message + Default>(
    peer: PeerId,
    mut stream: Stream,
    protocol: &'static str,
    config: ServerConfig,
    requests_tx: mpsc::Sender<Request<T>>,
) {
    let read_fut = async {
        let mut buf = Vec::new();
        (&mut stream).take(config.max_request_size + 1).read_to_end(&mut buf).await?;
        Ok::<_, std::io::Error>(buf)
    };
    let buf = match tokio::time::timeout(config.read_timeout, read_fut).await {
        Ok(Ok(buf)) => buf,
        Ok(Err(e)) => {
            log::debug!("Failed to read request from {peer}: {e}");
            return;
        }
        Err(_) => {
            log::debug!("Reading request from {peer} timed out");
            return;
        }
    };
    if buf.len() as u64 > config.max_request_size {
        log::warn!("Request from {peer} is too large ({} bytes), dropping", buf.len());
        return;
    }
    let request = match T::decode(buf.as_slice()) {
        Ok(request) => request,
        Err(e) => {
            log::warn!("Failed to decode {protocol} request from {peer}: {e}");
            return;
        }
    };

    let request = Request {
        peer_id: peer,
        request,
        response_sender: ResponseSender {
            stream,
            max_response_size: config.max_response_size,
            write_timeout: config.write_timeout,
        },
    };
    // Dropping `request` here (on a full or closed channel) resets the stream.
    if let Err(e) = requests_tx.try_send(request) {
        match e {
            mpsc::error::TrySendError::Full(_) => {
                log::warn!("Requests buffer is full, dropping request from {peer}")
            }
            mpsc::error::TrySendError::Closed(_) => {
                log::debug!("Requests channel is closed, dropping request from {peer}")
            }
        }
    }
}

impl<T: Message + Default + 'static> BehaviourWrapper for ServerBehaviour<T> {
    type Inner = libp2p_stream::Behaviour;
    type Event = Request<T>;

    fn inner(&mut self) -> &mut Self::Inner {
        &mut self.inner
    }

    fn poll(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<impl IntoIterator<Item = TToSwarm<Self>>> {
        loop {
            match self.incoming.poll_next_unpin(cx) {
                Poll::Ready(Some((peer, stream))) => self.spawn_read_task(peer, stream),
                Poll::Ready(None) => {
                    log::warn!("Incoming streams ended unexpectedly");
                    break;
                }
                Poll::Pending => break,
            }
        }

        match self.requests_rx.poll_recv(cx) {
            Poll::Ready(Some(request)) => Poll::Ready(Some(ToSwarm::GenerateEvent(request))),
            _ => Poll::Pending,
        }
    }
}

#[cfg(all(test, feature = "actors"))]
mod tests {
    use std::{future::Future, sync::Arc};

    use futures::StreamExt;
    use libp2p::{swarm::SwarmEvent, Swarm};
    use libp2p_swarm_test::SwarmExt;

    use super::*;
    use crate::behaviour::{
        stream_client::{ClientBehaviour, ClientConfig, StreamClientHandle},
        wrapped::Wrapped,
    };

    const PROTO: &str = "/test/stream-server/1";

    /// The message type served by the test protocol.
    #[derive(Clone, PartialEq, ::prost::Message)]
    struct TestMsg {
        #[prost(bytes = "vec", tag = "1")]
        payload: Vec<u8>,
    }

    fn encode(payload: &[u8]) -> Vec<u8> {
        TestMsg {
            payload: payload.to_vec(),
        }
        .encode_to_vec()
    }

    fn server_config() -> ServerConfig {
        ServerConfig {
            max_request_size: 1024,
            max_response_size: 1024,
            read_timeout: Duration::from_secs(5),
            write_timeout: Duration::from_secs(5),
            incoming_capacity: 128,
            requests_queue_size: 128,
        }
    }

    fn client_config() -> ClientConfig {
        ClientConfig {
            max_concurrent_streams: None,
            max_response_size: 1024,
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(5),
        }
    }

    /// Echo the request payload back as the response.
    fn echo(req: Request<TestMsg>) -> impl Future<Output = ()> + Send {
        let Request {
            request,
            response_sender,
            ..
        } = req;
        async move {
            let _ = response_sender.send(&request.payload).await;
        }
    }

    /// Build a server and a client swarm, connect them, and spawn both event loops.
    /// The server runs `handle_request` for each inbound request. Returns the server's
    /// peer id and a client handle for the test protocol.
    async fn setup<F, Fut>(handle_request: F) -> (PeerId, StreamClientHandle)
    where
        F: Fn(Request<TestMsg>) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut server = Swarm::new_ephemeral_tokio(|_| {
            Wrapped::from(ServerBehaviour::<TestMsg>::new(PROTO, server_config()))
        });
        let mut client = Swarm::new_ephemeral_tokio(|_| Wrapped::from(ClientBehaviour::default()));
        let handle = client.behaviour().new_handle(PROTO, client_config());
        let server_peer = *server.local_peer_id();

        server.listen().with_memory_addr_external().await;
        client.connect(&mut server).await;

        let handle_request = Arc::new(handle_request);
        tokio::spawn(async move {
            loop {
                let ev = server.select_next_some().await;
                if let SwarmEvent::Behaviour(req) = ev {
                    let handle_request = handle_request.clone();
                    tokio::spawn(async move { handle_request(req).await });
                }
            }
        });
        tokio::spawn(client.loop_on_next());

        (server_peer, handle)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn round_trip() {
        let (peer, handle) = setup(echo).await;
        let response = handle.request_response(peer, &encode(b"hello world")).await.unwrap();
        assert_eq!(response, b"hello world");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn concurrent_requests() {
        // Fire many requests concurrently: the server must serve every one of them. This
        // exercises the buffered accept channel (Control::accept_with_capacity) under a burst.
        // Note: on the fast in-process transport the server drains the accept channel quickly,
        // so this does not deterministically reproduce the drop that capacity 0 is prone to
        // under real scheduling jitter; it is a load/correctness check, not a strict guard.
        let (peer, handle) = setup(echo).await;
        let handle = Arc::new(handle);
        let futures = (0..50u32).map(|i| {
            let handle = handle.clone();
            async move { (i, handle.request_response(peer, &encode(&i.to_le_bytes())).await) }
        });
        let results = futures::future::join_all(futures).await;
        for (i, result) in results {
            let response = result.unwrap_or_else(|e| panic!("request {i} failed: {e:?}"));
            assert_eq!(response, i.to_le_bytes());
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropped_response_yields_empty() {
        // The server drops the ResponseSender without replying. Depending on the transport the
        // client observes either a reset (I/O error) or an empty response; on this in-process
        // transport it is an empty response. Either way it is a non-success (equivalent to the
        // old `request_response` ResponseOmission), which the application layer treats as an error.
        let (peer, handle) = setup(|_req| async move {}).await;
        let result = handle.request_response(peer, &encode(b"hello")).await;
        assert!(
            result.as_ref().map_or(true, Vec::is_empty),
            "expected a failure or empty response when the server drops the response, got {result:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn oversized_request_rejected() {
        // The server reads at most max_request_size + 1 bytes, sees the request is too large, and
        // drops the stream without replying — the client must not get a successful echo back.
        let (peer, handle) = setup(echo).await;
        let oversized = vec![0u8; 2048]; // exceeds max_request_size (1024)
        let result = handle.request_response(peer, &oversized).await;
        assert!(
            result.as_ref().map_or(true, Vec::is_empty),
            "expected a failure or empty response for an oversized request, got {result:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn undecodable_request_rejected() {
        // The bytes pass the size check but fail to decode as TestMsg, so the read task drops
        // the stream without replying — the client must not observe a successful response.
        let (peer, handle) = setup(echo).await;
        let garbage = [0x08]; // field 1, varint wire type, missing value
        let result = handle.request_response(peer, &garbage).await;
        assert!(
            result.as_ref().map_or(true, Vec::is_empty),
            "expected a failure or empty response for an undecodable request, got {result:?}"
        );
    }
}
