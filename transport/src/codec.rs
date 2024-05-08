mod legacy;
pub use legacy::LegacyCodec;

#[cfg(feature = "proto")]
mod proto;
#[cfg(feature = "proto")]
pub use proto::{ProtoCodec, ACK_SIZE};
