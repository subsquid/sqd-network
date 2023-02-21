#![feature(is_some_and)]

use cxx::{CxxString, CxxVector, UniquePtr};
use libp2p::{
    kad::{BootstrapError, NoKnownPeers},
    swarm::DialError,
    PeerId, TransportError,
};

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

pub struct P2PSender {}

impl P2PSender {
    pub fn send_message(&mut self, peer_id: &CxxString, msg: UniquePtr<CxxVector<u8>>) {
        let msg = String::from_utf8_lossy(msg.as_ref().unwrap().as_slice());
        println!("Sending message '{msg}' to peer {peer_id:?}")
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
