#![allow(dead_code)]

pub fn get_test_keypair() -> libp2p_identity::Keypair {
    libp2p_identity::Keypair::ed25519_from_bytes([
        19, 199, 234, 213, 79, 151, 179, 242, 187, 43, 210, 20, 250, 252, 12, 246, 223, 244, 119,
        225, 81, 225, 146, 40, 65, 35, 81, 91, 121, 13, 204, 37,
    ])
    .unwrap()
}
