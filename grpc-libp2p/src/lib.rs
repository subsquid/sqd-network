use libp2p::{swarm::DialError, TransportError};

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
    #[error("Dialing failed: {0:?}")]
    Dial(#[from] DialError),
    #[error("Invalid peer ID: {0}")]
    PeerId(String),
    #[error("Unexpected error: {0}")]
    Unexpected(&'static str),
}
