use futures::{stream::BoxStream, Stream, StreamExt};
use libp2p::{core::ParseError, PeerId};
use std::{
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

type Message = crate::Message<Vec<u8>>;

pub struct P2PTransportServer {
    peer_id: String,
    msg_receiver: Arc<Mutex<BoxStream<'static, Result<api::Message, Status>>>>,
    msg_sender: mpsc::Sender<Message>,
}

impl P2PTransportServer {
    pub fn new(
        peer_id: PeerId,
        msg_receiver: mpsc::Receiver<Message>,
        msg_sender: mpsc::Sender<Message>,
    ) -> Self {
        let msg_receiver = Arc::new(Mutex::new(
            ReceiverStream::new(msg_receiver).map(|msg| Ok(msg.into())).boxed(),
        ));
        Self {
            peer_id: peer_id.to_string(),
            msg_receiver,
            msg_sender,
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
        let peer_id = msg.peer_id.to_string();
        let content = msg.content;
        api::Message { peer_id, content }
    }
}

impl TryFrom<api::Message> for Message {
    type Error = ParseError;

    fn try_from(msg: api::Message) -> Result<Self, Self::Error> {
        let peer_id = msg.peer_id.parse()?;
        let content = msg.content;
        Ok(Message { peer_id, content })
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
}

pub async fn run_server(
    local_peer_id: PeerId,
    msg_receiver: mpsc::Receiver<Message>,
    msg_sender: mpsc::Sender<Message>,
) -> anyhow::Result<()> {
    log::info!("Running gRPC server");
    let server = P2PTransportServer::new(local_peer_id, msg_receiver, msg_sender);
    Server::builder()
        .add_service(api::p2p_transport_server::P2pTransportServer::new(server))
        .serve("0.0.0.0:50051".to_socket_addrs().unwrap().next().unwrap())
        .await?;
    Ok(())
}
