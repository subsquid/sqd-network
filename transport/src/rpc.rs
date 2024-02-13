use crate::transport::P2PTransportHandle;
use futures::{stream::BoxStream, Stream, StreamExt};
use libp2p::{
    identity::{Keypair, ParseError, PublicKey},
    PeerId,
};
use std::{
    fmt::Display,
    net::ToSocketAddrs,
    ops::DerefMut,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::{mpsc, Mutex, OwnedMutexGuard};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{async_trait, transport::Server, Request, Response, Status};

pub mod api {
    tonic::include_proto!("p2p_transport"); // The string specified here must match the proto package name
}

// This is the maximum *decompressed* size of the message
const MAX_MESSAGE_SIZE: usize = 100 * 1024 * 1024; // 100 MiB

type MsgContent = Vec<u8>;
type Message = crate::Message<MsgContent>;

pub struct P2PTransportServer {
    keypair: Keypair,
    msg_receiver: Arc<Mutex<BoxStream<'static, Result<api::Message, Status>>>>,
    transport_handle: P2PTransportHandle<MsgContent>,
}

impl P2PTransportServer {
    pub fn new(
        keypair: Keypair,
        msg_receiver: mpsc::Receiver<Message>,
        transport_handle: P2PTransportHandle<MsgContent>,
    ) -> Self {
        let msg_receiver = Arc::new(Mutex::new(
            ReceiverStream::new(msg_receiver).map(|msg| Ok(msg.into())).boxed(),
        ));
        Self {
            keypair,
            msg_receiver,
            transport_handle,
        }
    }
}

pub struct MsgStream {
    inner: OwnedMutexGuard<BoxStream<'static, Result<api::Message, Status>>>,
}

impl Stream for MsgStream {
    type Item = Result<api::Message, Status>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(self.inner.deref_mut()).poll_next(cx)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl From<Message> for api::Message {
    fn from(msg: Message) -> Self {
        api::Message {
            peer_id: msg.peer_id.map(|x| x.to_string()),
            content: msg.content,
            topic: msg.topic,
        }
    }
}

impl TryFrom<api::Message> for Message {
    type Error = ParseError;

    fn try_from(msg: api::Message) -> Result<Self, Self::Error> {
        let peer_id = match msg.peer_id {
            Some(peer_id) => Some(peer_id.parse()?),
            None => None,
        };
        Ok(Message {
            peer_id,
            topic: msg.topic,
            content: msg.content,
        })
    }
}

#[async_trait]
impl api::p2p_transport_server::P2pTransport for P2PTransportServer {
    type GetMessagesStream = MsgStream;

    async fn local_peer_id(
        &self,
        _request: Request<api::Empty>,
    ) -> Result<Response<api::PeerId>, Status> {
        Ok(Response::new(api::PeerId {
            peer_id: self.keypair.public().to_peer_id().to_string(),
        }))
    }

    async fn get_messages(
        &self,
        _request: Request<api::Empty>,
    ) -> Result<Response<Self::GetMessagesStream>, Status> {
        let guard = self.msg_receiver.clone().try_lock_owned().map_err(|_| {
            Status::failed_precondition("Only one inbound message stream can be open at once")
        })?;
        Ok(Response::new(MsgStream { inner: guard }))
    }

    async fn send_message(
        &self,
        request: Request<api::Message>,
    ) -> Result<Response<api::Empty>, Status> {
        let msg = match request.into_inner().try_into() {
            Ok(msg) => msg,
            Err(_) => return Err(Status::invalid_argument("Invalid peer ID")),
        };
        match self.transport_handle.send_message(msg).await {
            Ok(_) => Ok(Response::new(api::Empty {})),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    async fn toggle_subscription(
        &self,
        request: Request<api::Subscription>,
    ) -> Result<Response<api::Empty>, Status> {
        let subscription = request.into_inner();
        match self.transport_handle.toggle_subscription(subscription).await {
            Ok(_) => Ok(Response::new(api::Empty {})),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    async fn sign(&self, request: Request<api::Bytes>) -> Result<Response<api::Bytes>, Status> {
        let api::Bytes { bytes } = request.into_inner();
        match self.keypair.sign(&bytes) {
            Ok(bytes) => Ok(Response::new(api::Bytes { bytes })),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    async fn verify_signature(
        &self,
        request: Request<api::SignedData>,
    ) -> Result<Response<api::VerificationResult>, Status> {
        let api::SignedData {
            data,
            signature,
            peer_id,
        } = request.into_inner();
        let peer_id: PeerId = match peer_id.parse() {
            Ok(peer_id) => peer_id,
            Err(e) => return Err(Status::invalid_argument(e.to_string())),
        };
        let signature_ok = match PublicKey::try_decode_protobuf(&peer_id.to_bytes()[2..]) {
            Ok(pubkey) => pubkey.verify(&data, &signature),
            Err(e) => return Err(Status::invalid_argument(e.to_string())),
        };
        Ok(Response::new(api::VerificationResult { signature_ok }))
    }
}

pub async fn run_server<T: ToSocketAddrs + Display>(
    keypair: Keypair,
    listen_addr: T,
    msg_receiver: mpsc::Receiver<Message>,
    transport_handle: P2PTransportHandle<MsgContent>,
) -> anyhow::Result<()> {
    log::info!("Running gRPC server on address(es): {listen_addr}");
    let server = P2PTransportServer::new(keypair, msg_receiver, transport_handle.clone());
    let server = api::p2p_transport_server::P2pTransportServer::new(server)
        .max_decoding_message_size(MAX_MESSAGE_SIZE)
        .max_encoding_message_size(MAX_MESSAGE_SIZE);
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sigterm = signal(SignalKind::terminate())?;
    let shutdown = async move {
        tokio::select! {
            _ = sigint.recv() => (),
            _ = sigterm.recv() =>(),
        }
    };
    Server::builder()
        .add_service(server)
        .serve_with_shutdown(listen_addr.to_socket_addrs().unwrap().next().unwrap(), shutdown)
        .await?;
    transport_handle.stop().await?;
    Ok(())
}
