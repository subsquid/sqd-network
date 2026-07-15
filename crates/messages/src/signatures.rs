use std::mem::{size_of, size_of_val};

use sha3::{Digest, Sha3_256};

use libp2p_identity::{Keypair, PeerId, PublicKey};

use crate::{
    query_error, query_finished, query_result, ProstMsg, Query, QueryError, QueryExecuted,
    QueryFinished, QueryOk, QueryOkSummary, QueryResult,
};

const SHA3_256_SIZE: usize = 32;
const UUID_V4_SIZE: usize = 36;

pub fn sha3_256(msg: &[u8]) -> [u8; 32] {
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
    pub fn sign(&mut self, keypair: &Keypair, worker_id: PeerId) -> Result<(), &'static str> {
        let msg = self.msg_to_sign(worker_id).ok_or("couldn't sign invalid Query")?;
        self.signature = sign(&msg, keypair);
        Ok(())
    }

    pub fn verify_signature(&self, signer_id: PeerId, worker_id: PeerId) -> bool {
        if let Some(msg) = self.msg_to_sign(worker_id) {
            verify_signature(signer_id, &msg, &self.signature)
        } else {
            false
        }
    }

    fn msg_to_sign(&self, worker_id: PeerId) -> Option<Vec<u8>> {
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
                + self.dataset.len()
                + size_of::<u32>()
                + self.query.len()
                + size_of::<u32>()
                + self.chunk_id.len()
                + size_of::<u64>() * 2,
        );
        let dataset_len = u32::try_from(self.dataset.len()).expect("dataset length fits in u32");
        let query_len = u32::try_from(self.query.len()).expect("query length fits in u32");
        let chunk_id_len = u32::try_from(self.chunk_id.len()).expect("chunk_id length fits in u32");
        msg.extend_from_slice(self.query_id.as_bytes());
        msg.extend_from_slice(&worker_id_bytes);
        msg.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        msg.extend_from_slice(&dataset_len.to_le_bytes());
        msg.extend_from_slice(self.dataset.as_bytes());
        msg.extend_from_slice(&query_len.to_le_bytes());
        msg.extend_from_slice(self.query.as_bytes());
        msg.extend_from_slice(&chunk_id_len.to_le_bytes());
        msg.extend_from_slice(self.chunk_id.as_bytes());
        if let Some(range) = self.block_range {
            msg.extend_from_slice(&range.begin.to_le_bytes());
            msg.extend_from_slice(&range.end.to_le_bytes());
        }
        if self.query_engine != 0 || self.output_format != 0 {
            if self.block_range.is_none() {
                // Marks the absence of block_range to keep the encoding invertible.
                msg.push(0);
            }
            // Both fields are always emitted together so their values stay
            // unambiguous regardless of which one is non-default.
            msg.push(u8::try_from(self.query_engine).ok()?);
            msg.push(u8::try_from(self.output_format).ok()?);
        }
        Some(msg)
    }
}

impl QueryResult {
    pub fn sign(&mut self, keypair: &Keypair) -> Result<(), &'static str> {
        let msg = self.msg_to_sign().ok_or("couldn't sign invalid QueryResult")?;
        self.signature = sign(&msg, keypair);
        Ok(())
    }

    pub fn verify_signature(&self, signer_id: PeerId) -> bool {
        if let Some(msg) = self.msg_to_sign() {
            verify_signature(signer_id, &msg, &self.signature)
        } else {
            false
        }
    }

    fn msg_to_sign(&self) -> Option<Vec<u8>> {
        match &self.result {
            Some(query_result::Result::Ok(result)) => {
                msg_to_sign_query_ok(&self.query_id, &sha3_256(&result.data), result.last_block)
            }
            Some(query_result::Result::Err(QueryError { err: Some(err) })) => {
                msg_to_sign_query_err(&self.query_id, err)
            }
            _ => None,
        }
    }
}

impl QueryFinished {
    pub fn verify_signature(&self) -> bool {
        let msg = match &self.result {
            Some(query_finished::Result::Ok(result)) => {
                msg_to_sign_query_ok(&self.query_id, &result.data_hash, result.last_block)
            }
            Some(query_finished::Result::Err(QueryError { err: Some(err) })) => {
                msg_to_sign_query_err(&self.query_id, err)
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

    pub fn new(query_result: &QueryResult, worker_id: String, total_time_micros: u32) -> Self {
        Self {
            worker_id,
            query_id: query_result.query_id.clone(),
            total_time_micros,
            worker_signature: query_result.signature.clone(),
            result: match &query_result.result {
                Some(query_result::Result::Ok(QueryOk { data, last_block })) => {
                    Some(query_finished::Result::Ok(QueryOkSummary {
                        uncompressed_data_size: data.len() as u64,
                        data_hash: sha3_256(data).to_vec(),
                        last_block: *last_block,
                    }))
                }
                Some(query_result::Result::Err(err)) => {
                    Some(query_finished::Result::Err(err.clone()))
                }
                None => None,
            },
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

fn msg_to_sign_query_ok(query_id: &str, data_hash: &[u8], last_block: u64) -> Option<Vec<u8>> {
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

fn msg_to_sign_query_err(query_id: &str, err: &query_error::Err) -> Option<Vec<u8>> {
    if query_id.len() != UUID_V4_SIZE {
        return None;
    }
    let code = match err {
        query_error::Err::BadRequest(_) => 1,
        query_error::Err::NotFound(_) => 2,
        query_error::Err::ServerError(_) => 3,
        query_error::Err::TooManyRequests(_) => 4,
        query_error::Err::ServerOverloaded(_) => 5,
    };
    let mut msg = Vec::with_capacity(UUID_V4_SIZE + 1);
    msg.extend_from_slice(query_id.as_bytes());
    msg.push(code);
    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Range;

    fn sample_query() -> Query {
        Query {
            query_id: "6c9bb0e3-4a3f-4b7a-9b6a-2f2f6e1a0d51".to_string(),
            dataset: "s3://ethereum-mainnet".to_string(),
            query: r#"{"type": "evm"}"#.to_string(),
            chunk_id: "0000000000/0000808640-0000816499-b0486318".to_string(),
            timestamp_ms: 1_700_000_000_000,
            block_range: Some(Range {
                begin: 808640,
                end: 816499,
            }),
            ..Default::default()
        }
    }

    fn worker_id() -> PeerId {
        Keypair::generate_ed25519().public().to_peer_id()
    }

    #[test]
    fn sign_verify_roundtrip_with_query_engine() {
        let keypair = Keypair::generate_ed25519();
        let client_id = keypair.public().to_peer_id();
        let worker_id = worker_id();
        for block_range in [None, Some(Range { begin: 1, end: 2 })] {
            for query_engine in [0, 1] {
                for output_format in [0, 1] {
                    let mut query = sample_query();
                    query.block_range = block_range;
                    query.query_engine = query_engine;
                    query.output_format = output_format;
                    query.sign(&keypair, worker_id).unwrap();
                    assert!(query.verify_signature(client_id, worker_id));
                }
            }
        }
    }

    #[test]
    fn unspecified_engine_payload_is_unchanged() {
        // The signed payload with query_engine == 0 and output_format == 0 must be
        // byte-identical to the original encoding so that existing clients remain
        // compatible.
        let worker_id = worker_id();
        let query = sample_query();
        let msg = query.msg_to_sign(worker_id).unwrap();

        let mut expected = Vec::new();
        expected.extend_from_slice(query.query_id.as_bytes());
        expected.extend_from_slice(&worker_id.to_bytes());
        expected.extend_from_slice(&query.timestamp_ms.to_le_bytes());
        expected.extend_from_slice(&u32::try_from(query.dataset.len()).unwrap().to_le_bytes());
        expected.extend_from_slice(query.dataset.as_bytes());
        expected.extend_from_slice(&u32::try_from(query.query.len()).unwrap().to_le_bytes());
        expected.extend_from_slice(query.query.as_bytes());
        expected.extend_from_slice(&u32::try_from(query.chunk_id.len()).unwrap().to_le_bytes());
        expected.extend_from_slice(query.chunk_id.as_bytes());
        let range = query.block_range.unwrap();
        expected.extend_from_slice(&range.begin.to_le_bytes());
        expected.extend_from_slice(&range.end.to_le_bytes());
        assert_eq!(msg, expected);
    }

    #[test]
    fn engine_choice_is_covered_by_signature() {
        let keypair = Keypair::generate_ed25519();
        let client_id = keypair.public().to_peer_id();
        let worker_id = worker_id();
        let mut query = sample_query();
        query.query_engine = 1;
        query.sign(&keypair, worker_id).unwrap();

        // Flipping the engine choice must invalidate the signature.
        query.query_engine = 0;
        assert!(!query.verify_signature(client_id, worker_id));
        query.query_engine = 2;
        assert!(!query.verify_signature(client_id, worker_id));
    }

    #[test]
    fn output_format_is_covered_by_signature() {
        let keypair = Keypair::generate_ed25519();
        let client_id = keypair.public().to_peer_id();
        let worker_id = worker_id();
        let mut query = sample_query();
        query.output_format = 1;
        query.sign(&keypair, worker_id).unwrap();

        // Flipping the output format must invalidate the signature.
        query.output_format = 0;
        assert!(!query.verify_signature(client_id, worker_id));
        query.output_format = 2;
        assert!(!query.verify_signature(client_id, worker_id));
    }
}
