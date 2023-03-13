#![feature(is_some_and)]

use cxx::{CxxString, CxxVector, UniquePtr};

use libp2p::{
    kad::{BootstrapError, NoKnownPeers},
    request_response::{InboundFailure, OutboundFailure},
    swarm::DialError,
    PeerId, TransportError,
};

use tokio::sync::mpsc;

pub mod rpc;
pub mod transport;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Libp2p transport creation failed")]
    Transport,
    #[error("Listening failed: {0:?}")]
    Listen(#[from] TransportError<std::io::Error>),
    #[error("Dialing failed: {0}")]
    Dial(String),
    #[error("Kademlia bootstrap error: {0:?}")]
    Bootstrap(#[from] BootstrapError),
    #[error("{0}")]
    NoPeers(#[from] NoKnownPeers),
    #[error("Invalid peer ID: {0}")]
    PeerId(String),
    #[error("Peer not found: {0}")]
    PeerNotFound(PeerId),
    #[error("Null pointer")]
    NullPointer,
    #[error("Message write error: {0}")]
    MessageWrite(std::io::Error),
    #[error("Message read error: {0}")]
    MessageRead(std::io::Error),
    #[error("Inbound failure:  {0}")]
    Inbound(#[from] InboundFailure),
    #[error("Outbound failure:  {0}")]
    Outbound(#[from] OutboundFailure),
    #[error("Query timed out. Could not find peer {0}")]
    QueryTimeout(PeerId),
    #[error("Unexpected error: {0}")]
    Unexpected(&'static str),
}

impl From<DialError> for Error {
    fn from(err: DialError) -> Self {
        Self::Dial(format!("{err:?}"))
    }
}

impl From<&DialError> for Error {
    fn from(err: &DialError) -> Self {
        Self::Dial(format!("{err:?}"))
    }
}

pub type MsgContent = UniquePtr<CxxVector<u8>>;

#[derive(Debug)]
pub struct Message {
    pub peer_id: PeerId,
    pub content: MsgContent,
}

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
        ) -> UniquePtr<MessageReceiver>;
        fn stop(self: Pin<&mut Worker>);

    }
}
