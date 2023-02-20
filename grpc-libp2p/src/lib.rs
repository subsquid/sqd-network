#![feature(is_some_and)]

use libp2p::{
    kad::{BootstrapError, NoKnownPeers},
    swarm::DialError,
    PeerId, TransportError,
};

pub mod transport;
pub mod worker;
pub mod worker_api {
    tonic::include_proto!("worker_rpc"); // The string specified here must match the proto package name
}

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
