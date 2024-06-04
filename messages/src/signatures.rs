use sha3::{Digest, Sha3_256};

use libp2p::{
    identity::{Keypair, PublicKey},
    PeerId,
};

use crate::{Ping, ProstMsg, Query, QueryExecuted};

pub fn msg_hash<M: ProstMsg>(msg: &M) -> Vec<u8> {
    let mut result = [0u8; 32];
    let mut hasher = Sha3_256::default();
    hasher.update(msg.encode_to_vec().as_slice());
    Digest::finalize_into(hasher, result.as_mut_slice().into());
    result.to_vec()
}

fn verify_signature<T: SignedMessage>(peer_id: &PeerId, msg: &mut T) -> bool {
    let sig = msg.detach_signature();
    let encoded = msg.encode_to_vec();
    let result = match PublicKey::try_decode_protobuf(&peer_id.to_bytes()[2..]) {
        Ok(pubkey) => pubkey.verify(&encoded, &sig),
        Err(_) => false,
    };
    msg.attach_signature(sig);
    result
}

pub trait SignedMessage: ProstMsg + Sized {
    fn detach_signature(&mut self) -> Vec<u8>;
    fn attach_signature(&mut self, signature: Vec<u8>);

    fn sign(&mut self, keypair: &Keypair) {
        _ = self.detach_signature(); // To make signing idempotent
        let bytes = self.encode_to_vec();
        let signature = keypair.sign(&bytes).expect("infallible for Ed25519");
        self.attach_signature(signature);
    }

    fn verify_signature(&mut self, peer_id: &PeerId) -> bool {
        verify_signature(peer_id, self)
    }
}

impl SignedMessage for Ping {
    fn detach_signature(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.signature)
    }

    fn attach_signature(&mut self, signature: Vec<u8>) {
        self.signature = signature;
    }
}

impl SignedMessage for Query {
    fn detach_signature(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.signature)
    }

    fn attach_signature(&mut self, signature: Vec<u8>) {
        self.signature = signature;
    }
}

impl SignedMessage for QueryExecuted {
    fn detach_signature(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.signature)
    }

    fn attach_signature(&mut self, signature: Vec<u8>) {
        self.signature = signature;
    }

    fn verify_signature(&mut self, peer_id: &PeerId) -> bool {
        if !verify_signature(peer_id, self) {
            return false;
        }
        let Ok(client_id) = self.client_id.parse() else {
            return false;
        };
        self.query.as_mut().is_some_and(|q| verify_signature(&client_id, q))
    }
}
