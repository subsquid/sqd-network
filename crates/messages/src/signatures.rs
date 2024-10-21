use sha3::{Digest, Sha3_256};

use libp2p::{
    identity::{Keypair, PublicKey},
    PeerId,
};

use crate::{
    query_finished, query_result, ProstMsg, Query, QueryExecuted, QueryFinished, QueryOk, QueryResult, QueryResultSummary
};

fn sha3_256(msg: &[u8]) -> [u8; 32] {
    let mut hasher = Sha3_256::default();
    hasher.update(msg);
    hasher.finalize().into()
}

pub fn msg_hash<M: ProstMsg>(msg: &M) -> [u8; 32] {
    sha3_256(&msg.encode_to_vec())
}

fn sign(msg: &[u8], keypair: &Keypair) -> Vec<u8> {
    keypair.sign(msg).expect("signing should succeed for Ed25519")
}

fn verify_signature(peer_id: PeerId, msg: &[u8], signature: &[u8]) -> bool {
    match PublicKey::try_decode_protobuf(&peer_id.to_bytes()[2..]) {
        Ok(pubkey) => pubkey.verify(msg, signature),
        Err(_) => false,
    }
}

impl Query {
    pub fn sign(&mut self, keypair: &Keypair, worker_id: PeerId) {
        let msg = self.signed_msg(worker_id);
        self.signature = sign(&msg, keypair);
    }

    pub fn verify_signature(&self, signer_id: PeerId, worker_id: PeerId) -> bool {
        verify_signature(signer_id, &self.signed_msg(worker_id), &self.signature)
    }

    fn signed_msg(&self, worker_id: PeerId) -> Vec<u8> {
        let mut msg = Vec::with_capacity(525000);
        msg.extend_from_slice(self.query_id.as_bytes());
        msg.extend_from_slice(&worker_id.to_bytes());
        msg.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        msg.extend_from_slice(self.dataset.as_bytes());
        msg.extend_from_slice(self.query.as_bytes());
        if let Some(range) = self.block_range {
            msg.extend_from_slice(&range.begin.to_le_bytes());
            msg.extend_from_slice(&range.end.to_le_bytes());
        }
        msg
    }
}

impl QueryResultSummary {
    pub fn new(result: &[u8]) -> Self {
        let hash = sha3_256(result).to_vec();
        Self {
            hash,
            size: result.len() as u64,
            signature: None,
        }
    }

    pub fn sign(&mut self, keypair: &Keypair, query_id: &str) {
        let msg = Self::signed_msg(&self.hash, query_id);
        self.signature = Some(sign(&msg, keypair));
    }

    pub fn verify_signature(&self, signer_id: PeerId, query_id: &str) -> bool {
        if let Some(signature) = &self.signature {
            verify_signature(signer_id, &Self::signed_msg(&self.hash, query_id), signature)
        } else {
            false
        }
    }

    fn signed_msg(hash: &[u8], query_id: &str) -> Vec<u8> {
        let mut msg = Vec::with_capacity(32 + query_id.len());
        msg.extend_from_slice(hash);
        msg.extend_from_slice(query_id.as_bytes());
        msg
    }
}

impl QueryOk {
    pub fn new(result: Vec<u8>, last_block: u64) -> Self {
        Self {
            summary: Some(QueryResultSummary::new(&result)),
            data: result,
            last_block: Some(last_block),
        }
    }

    pub fn sign(&mut self, keypair: &Keypair, query_id: &str) {
        if let Some(summary) = &mut self.summary {
            summary.sign(keypair, query_id);
        }
    }

    pub fn verify_signature(&self, signer_id: PeerId, query_id: &str) -> bool {
        if let Some(summary) = &self.summary {
            summary.verify_signature(signer_id, query_id)
        } else {
            false
        }
    }
}

impl QueryResult {
    pub fn sign(&mut self, keypair: &Keypair) {
        if let Some(query_result::Result::Ok(result)) = &mut self.result {
            result.sign(keypair, &self.query_id);
        }
    }

    pub fn verify_signature(&self, signer_id: PeerId) -> bool {
        if let Some(query_result::Result::Ok(result)) = &self.result {
            result.verify_signature(signer_id, &self.query_id)
        } else {
            true
        }
    }
}

impl QueryFinished {
    pub fn verify_signature(&self) -> bool {
        if let Some(query_finished::Result::Ok(result)) = &self.result {
            let Ok(worker_id) = self.worker_id.parse() else {
                return false;
            };
            result.verify_signature(worker_id, &self.query_id)
        } else {
            true
        }
    }
}

impl QueryExecuted {
    pub fn verify_client_signature(&self, worker_id: PeerId) -> bool {
        let Some(query) = &self.query else {
            return false;
        };
        let Ok(client_id) = self.client_id.parse() else {
            return false;
        };
        query.verify_signature(client_id, worker_id)
    }
}
