use sha3::{Digest, Sha3_256};

use libp2p::{
    identity::{Keypair, PublicKey},
    PeerId,
};

use crate::{
    query_error, query_finished, query_result, Query, QueryError, QueryExecuted, QueryFinished,
    QueryResult,
};

const SHA3_256_SIZE: usize = 32;
const UUID_V4_SIZE: usize = 36;

pub fn sha3_256(msg: &[u8]) -> [u8; 32] {
    let mut hasher = Sha3_256::default();
    hasher.update(msg);
    hasher.finalize().into()
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
    pub fn sign(&mut self, keypair: &Keypair, worker_id: PeerId) -> Result<(), &'static str> {
        let msg = self.signed_msg(worker_id).ok_or("couldn't sign invalid Query")?;
        self.signature = sign(&msg, keypair);
        Ok(())
    }

    pub fn verify_signature(&self, signer_id: PeerId, worker_id: PeerId) -> bool {
        if let Some(msg) = self.signed_msg(worker_id) {
            verify_signature(signer_id, &msg, &self.signature)
        } else {
            false
        }
    }

    fn signed_msg(&self, worker_id: PeerId) -> Option<Vec<u8>> {
        // No two queries should have the same encoding. The easiest way to guarantee that
        // is to use invertible encoding. That's why variable length fields are prefixed with their length.
        if self.query_id.len() != UUID_V4_SIZE {
            return None;
        }
        let worker_id_bytes = worker_id.to_bytes();
        let mut msg = Vec::with_capacity(
            UUID_V4_SIZE
                + worker_id_bytes.len()
                + size_of_val(&self.timestamp_ms)
                + size_of::<u32>()
                + self.dataset.as_bytes().len()
                + size_of::<u32>()
                + self.query.as_bytes().len()
                + size_of::<u64>() * 2,
        );
        msg.extend_from_slice(self.query_id.as_bytes());
        msg.extend_from_slice(&worker_id_bytes);
        msg.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        msg.extend_from_slice(&(self.dataset.len() as u32).to_le_bytes());
        msg.extend_from_slice(self.dataset.as_bytes());
        msg.extend_from_slice(&(self.query.len() as u32).to_le_bytes());
        msg.extend_from_slice(self.query.as_bytes());
        if let Some(range) = self.block_range {
            msg.extend_from_slice(&range.begin.to_le_bytes());
            msg.extend_from_slice(&range.end.to_le_bytes());
        }
        Some(msg)
    }
}

impl QueryResult {
    pub fn sign(&mut self, keypair: &Keypair) -> Result<(), &'static str> {
        let msg = self.signed_msg().ok_or("couldn't sign invalid QueryResult")?;
        self.signature = sign(&msg, keypair);
        Ok(())
    }

    pub fn verify_signature(&self, signer_id: PeerId) -> bool {
        if let Some(msg) = self.signed_msg() {
            verify_signature(signer_id, &msg, &self.signature)
        } else {
            false
        }
    }

    fn signed_msg(&self) -> Option<Vec<u8>> {
        match &self.result {
            Some(query_result::Result::Ok(result)) => {
                signed_msg_query_ok(&self.query_id, &sha3_256(&result.data), result.last_block)
            }
            Some(query_result::Result::Err(QueryError { err: Some(err) })) => {
                signed_msg_query_err(&self.query_id, err)
            }
            _ => None,
        }
    }
}

impl QueryFinished {
    pub fn verify_signature(&self) -> bool {
        let msg = match &self.result {
            Some(query_finished::Result::Ok(result)) => {
                signed_msg_query_ok(&self.query_id, &result.data_hash, result.last_block)
            }
            Some(query_finished::Result::Err(QueryError { err: Some(err) })) => {
                signed_msg_query_err(&self.query_id, err)
            }
            _ => return false,
        };
        let Some(msg) = msg else {
            return false;
        };
        let Ok(worker_id) = self.worker_id.parse() else {
            return false;
        };
        verify_signature(worker_id, &msg, &self.worker_signature)
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

fn signed_msg_query_ok(query_id: &str, data_hash: &[u8], last_block: u64) -> Option<Vec<u8>> {
    if data_hash.len() != SHA3_256_SIZE {
        return None;
    }
    if query_id.len() != UUID_V4_SIZE {
        return None;
    }
    let mut msg = Vec::with_capacity(UUID_V4_SIZE + SHA3_256_SIZE + size_of_val(&last_block));
    msg.extend_from_slice(query_id.as_bytes());
    msg.extend_from_slice(data_hash);
    msg.extend_from_slice(&last_block.to_le_bytes());
    Some(msg)
}

fn signed_msg_query_err(query_id: &str, err: &query_error::Err) -> Option<Vec<u8>> {
    if query_id.len() != UUID_V4_SIZE {
        return None;
    }
    let code = match err {
        query_error::Err::BadRequest(_) => 1,
        query_error::Err::NotFound(_) => 2,
        query_error::Err::ServerError(_) => 3,
        query_error::Err::TooManyRequests(_) => 4,
        query_error::Err::ServerOverloaded(_) => 5,
        query_error::Err::Timeout(_) => 6,
    };
    let mut msg = Vec::with_capacity(UUID_V4_SIZE + 1);
    msg.extend_from_slice(query_id.as_bytes());
    msg.push(code);
    Some(msg)
}
