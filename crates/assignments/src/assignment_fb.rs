#![allow(dead_code, unused_imports, unsafe_op_in_unsafe_fn, clippy::all)]

use libp2p_identity::PeerId;

include!("../schema/gen/assignment_generated.rs");

impl Eq for WorkerId {}

impl PartialOrd for WorkerId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for WorkerId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl TryInto<PeerId> for WorkerId {
    type Error = libp2p_identity::ParseError;

    fn try_into(self) -> Result<PeerId, Self::Error> {
        PeerId::from_bytes(&self.0)
    }
}

impl From<PeerId> for WorkerId {
    fn from(peer_id: PeerId) -> Self {
        let buf = peer_id.to_bytes();
        let (bytes, _) = buf
            .split_first_chunk()
            .expect("PeerId should always have a valid length");
        WorkerId(*bytes)
    }
}
