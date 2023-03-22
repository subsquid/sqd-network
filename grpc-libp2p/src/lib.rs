use libp2p::{
    kad::{BootstrapError, NoKnownPeers},
    request_response::{InboundFailure, OutboundFailure},
    swarm::DialError,
    PeerId, TransportError,
};
use std::fmt::Debug;

#[cfg(feature = "rpc")]
pub mod rpc;
pub mod transport;
#[cfg(feature = "worker")]
pub mod worker;

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

pub trait MsgContent: Sized + Send + Debug + 'static {
    fn new(size: usize) -> Self;
    fn as_slice(&self) -> &[u8];
    fn as_mut_slice(&mut self) -> &mut [u8];
}

#[derive(Debug)]
pub struct Message<T: MsgContent> {
    pub peer_id: PeerId,
    pub content: T,
}
