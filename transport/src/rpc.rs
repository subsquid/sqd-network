use futures::{stream::BoxStream, Stream, StreamExt};
use libp2p::{identity::ParseError, PeerId};
use std::{
    fmt::Display,
    net::ToSocketAddrs,
    ops::DerefMut,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};
use tokio::sync::{mpsc, Mutex, OwnedMutexGuard};
use tokio_stream::wrappers::ReceiverStream;
use tonic::{async_trait, transport::Server, Request, Response, Status};

pub mod api {
    tonic::include_proto!("p2p_transport"); // The string specified here must match the proto package name
}

// This is the maximum *decompressed* size of the message
const MAX_MESSAGE_SIZE: usize = 100 * 1024 * 1024; // 100 MiB

type Message = crate::Message<Vec<u8>>;

pub struct P2PTransportServer {
    peer_id: String,
    msg_receiver: Arc<Mutex<BoxStream<'static, Result<api::Message, Status>>>>,
    msg_sender: mpsc::Sender<Message>,
    subscription_sender: mpsc::Sender<(String, bool)>,
}

impl P2PTransportServer {
    pub fn new(
        peer_id: PeerId,
        msg_receiver: mpsc::Receiver<Message>,
        msg_sender: mpsc::Sender<Message>,
        subscription_sender: mpsc::Sender<(String, bool)>,
    ) -> Self {
        let msg_receiver = Arc::new(Mutex::new(
            ReceiverStream::new(msg_receiver).map(|msg| Ok(msg.into())).boxed(),
        ));
        Self {
            peer_id: peer_id.to_string(),
            msg_receiver,
            msg_sender,
            subscription_sender,
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
            peer_id: self.peer_id.clone(),
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
        match self.msg_sender.send(msg).await {
            Ok(_) => Ok(Response::new(api::Empty {})),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }

    async fn toggle_subscription(
        &self,
        request: Request<api::Subscription>,
    ) -> Result<Response<api::Empty>, Status> {
        let api::Subscription { topic, subscribed } = request.into_inner();
        match self.subscription_sender.send((topic, subscribed)).await {
            Ok(_) => Ok(Response::new(api::Empty {})),
            Err(e) => Err(Status::internal(e.to_string())),
        }
    }
}

pub async fn run_server<T: ToSocketAddrs + Display>(
    local_peer_id: PeerId,
    listen_addr: T,
    msg_receiver: mpsc::Receiver<Message>,
    msg_sender: mpsc::Sender<Message>,
    subscription_sender: mpsc::Sender<(String, bool)>,
) -> anyhow::Result<()> {
    log::info!("Running gRPC server on address(es): {listen_addr}");
    let server =
        P2PTransportServer::new(local_peer_id, msg_receiver, msg_sender, subscription_sender);
    let server = api::p2p_transport_server::P2pTransportServer::new(server)
        .max_decoding_message_size(MAX_MESSAGE_SIZE)
        .max_encoding_message_size(MAX_MESSAGE_SIZE);
    Server::builder()
        .add_service(server)
        .serve(listen_addr.to_socket_addrs().unwrap().next().unwrap())
        .await?;
    Ok(())
}
