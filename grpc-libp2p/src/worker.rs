use cxx::{let_cxx_string, CxxString, CxxVector, UniquePtr};
use futures::{stream::FusedStream, StreamExt};
use libp2p::PeerId;
use std::ops::Deref;

use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

pub type MsgContent = UniquePtr<CxxVector<u8>>;

impl crate::MsgContent for MsgContent {
    fn new(size: usize) -> Self {
        ffi::new_buffer(size)
    }

    fn as_slice(&self) -> &[u8] {
        self.deref().as_slice()
    }

    fn as_mut_slice(&mut self) -> &mut [u8] {
        self.as_mut().expect("Null pointer").as_mut_slice()
    }
}

type Message = crate::Message<MsgContent>;

impl Message {
    pub fn new(peer_id: &CxxString, content: MsgContent) -> Self {
        let peer_id = peer_id.to_string().parse().unwrap();
        Self { peer_id, content }
    }
}

pub struct P2PSender(mpsc::Sender<Message>);

impl P2PSender {
    pub fn send_message(&mut self, peer_id: &CxxString, msg: MsgContent) {
        log::debug!("Sending message to peer {peer_id}");
        let message = Message::new(peer_id, msg);
        self.0.blocking_send(message).unwrap();
    }
}

impl From<mpsc::Sender<Message>> for Box<P2PSender> {
    fn from(sender: mpsc::Sender<Message>) -> Self {
        Box::new(P2PSender(sender))
    }
}

pub async fn run_worker(
    local_peer_id: PeerId,
    msg_receiver: mpsc::Receiver<Message>,
    msg_sender: mpsc::Sender<Message>,
    config: String,
) {
    let_cxx_string!(peer_id = local_peer_id.to_string());

    let sender = ffi::wrap_sender(msg_sender.into());

    let mut worker = ffi::new_worker();
    let_cxx_string!(config = config);
    worker.as_mut().unwrap().configure(&config);

    let mut handler = worker.as_mut().unwrap().start(sender, &peer_id);
    let mut inbound_messages = ReceiverStream::new(msg_receiver).fuse();
    while !inbound_messages.is_terminated() {
        let Message { peer_id, content } = inbound_messages.select_next_some().await;
        let_cxx_string!(peer_id = peer_id.to_base58());
        handler.as_mut().unwrap().on_message_received(&peer_id, content);
    }
}

#[cxx::bridge(namespace = "subsquid")]
pub mod ffi {
    extern "Rust" {
        type P2PSender;
        #[cxx_name = "sendMessage"]
        fn send_message(&mut self, peer_id: &CxxString, msg: UniquePtr<CxxVector<u8>>);
    }

    unsafe extern "C++" {
        include!("grpc-libp2p/sql-archives/worker/src/rust_binding.hpp");
        include!("grpc-libp2p/src/worker/worker.hpp");

        #[rust_name = "new_buffer"]
        fn newBuffer(size: usize) -> UniquePtr<CxxVector<u8>>;

        type MessageReceiver;
        #[rust_name = "on_message_received"]
        fn onMessageReceived(
            self: Pin<&mut MessageReceiver>,
            peer_id: &CxxString,
            msg: UniquePtr<CxxVector<u8>>,
        );

        type MessageSender;
        #[rust_name = "wrap_sender"]
        fn wrapSender(sender: Box<P2PSender>) -> UniquePtr<MessageSender>;

        type Worker;
        #[rust_name = "new_worker"]
        fn newWorker() -> UniquePtr<Worker>;
        fn configure(self: Pin<&mut Worker>, opaque: &CxxString);
        fn start(
            self: Pin<&mut Worker>,
            sender: UniquePtr<MessageSender>,
            peer_id: &CxxString,
        ) -> UniquePtr<MessageReceiver>;
        fn stop(self: Pin<&mut Worker>);

    }
}
