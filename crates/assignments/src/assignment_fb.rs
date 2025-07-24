#![allow(dead_code, unused_imports, unsafe_op_in_unsafe_fn, clippy::all)]

use libp2p_identity::PeerId;

include!("../schema/gen/assignment_generated.rs");

impl Eq for WorkerId {}

impl TryInto<PeerId> for WorkerId {
    type Error = libp2p_identity::ParseError;

    fn try_into(self) -> Result<PeerId, Self::Error> {
        PeerId::from_bytes(&self.0)
    }
}

impl From<PeerId> for WorkerId {
    fn from(peer_id: PeerId) -> Self {
        let buf = peer_id.to_bytes();
        let (bytes, rest) =
            buf.split_first_chunk().expect("PeerId should always have a valid length");
        debug_assert_eq!(rest, &[] as &[u8], "PeerId should not have extra bytes");
        WorkerId(*bytes)
    }
}

#[test]
fn test_worker_id_conversion() {
    let peer_id = libp2p_identity::Keypair::generate_ed25519().public().to_peer_id();
    let worker_id: WorkerId = peer_id.clone().into();
    let converted_peer_id: PeerId = worker_id.try_into().expect("Conversion should succeed");
    assert_eq!(peer_id, converted_peer_id, "PeerId conversion failed");
}
