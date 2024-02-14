use libp2p::{
    kad::{BootstrapError, NoKnownPeers},
    noise,
    request_response::{InboundFailure, OutboundFailure},
    swarm::DialError,
    TransportError,
};

pub use libp2p::{
    identity::{Keypair, PublicKey},
    Multiaddr, PeerId,
};
pub use message::{Message, MsgContent};
pub use rpc::api::Subscription;

pub mod cli;
mod message;
pub mod rpc;
pub mod task_manager;
pub mod transport;
pub mod util;

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
    #[error("Inbound failure: {error} Peer ID: {peer}")]
    Inbound { peer: PeerId, error: InboundFailure },
    #[error("Outbound failure: {error} Peer ID: {peer}")]
    Outbound {
        peer: PeerId,
        error: OutboundFailure,
    },
    #[error("Query timed out. Could not find peer {0}")]
    QueryTimeout(PeerId),
    #[error("No available relay")]
    NoRelay,
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

impl From<noise::Error> for Error {
    fn from(_: noise::Error) -> Self {
        Self::Transport
    }
}

impl From<std::io::Error> for Error {
    fn from(_: std::io::Error) -> Self {
        Self::Transport
    }
}
