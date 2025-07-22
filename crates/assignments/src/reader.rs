use std::collections::BTreeMap;

use anyhow::anyhow;
use crypto_box::{aead::Aead, PublicKey, SalsaBox, SecretKey};
use libp2p_identity::{Keypair, PeerId};
use sha2::{digest::generic_array::GenericArray, Digest, Sha512};

use crate::{assignment_fb, WorkerStatus};

#[ouroboros::self_referencing]
pub struct Assignment {
    buf: Vec<u8>,

    #[borrows(buf)]
    #[covariant]
    reader: assignment_fb::Assignment<'this>,
}

impl Assignment {
    pub fn from_owned(buf: Vec<u8>) -> Result<Self, flatbuffers::InvalidFlatbuffer> {
        AssignmentTryBuilder {
            buf,
            reader_builder: |buf| assignment_fb::root_as_assignment(buf),
        }
        .try_build()
    }

    pub fn get_worker(&self, id: PeerId) -> Option<Worker<'_>> {
        let workers = self.borrow_reader().workers();
        let worker = workers.lookup_by_key(id, |x, key| {
            let id: PeerId = (*x.worker_id()).try_into().unwrap_or_else(|e| {
                panic!("Couldn't parse peer id '{:?}': {}", x.worker_id().peer_id(), e);
            });
            id.cmp(key)
        })?;
        Some(Worker { reader: worker })
    }

    pub fn get_dataset(&self, dataset: &str) -> Option<assignment_fb::Dataset<'_>> {
        self.borrow_reader()
            .datasets()
            .lookup_by_key(dataset, |ds, key| ds.key_compare_with_value(key))
    }

    pub fn find_chunk(&self, dataset: &str, block: u64) -> Option<assignment_fb::Chunk<'_>> {
        let dataset = self.get_dataset(dataset)?;

        if block > dataset.last_block() {
            return None;
        }

        // find last chunk with first_block <= block
        let chunks = dataset.chunks();
        // left is always either equal to -1 or points to the chunk with first_block <= block
        let mut left = -1;
        // right is always either equal to chunks.len() or points to the chunk with first_block > block
        let mut right = chunks.len() as isize;
        while left + 1 < right {
            let mid = (left + right) / 2;
            let chunk = chunks.get(mid as usize);
            if chunk.first_block() <= block {
                left = mid;
            } else {
                right = mid;
            }
        }
        if left == -1 {
            return None; // block is below the first chunk
        }
        Some(chunks.get(left as usize))
    }
}

pub struct Worker<'f> {
    reader: assignment_fb::WorkerAssignment<'f>,
}

impl Worker<'_> {
    pub fn chunks(
        &self,
    ) -> flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<assignment_fb::Chunk<'_>>> {
        self.reader.chunks()
    }

    pub fn status(&self) -> WorkerStatus {
        match self.reader.status() {
            assignment_fb::WorkerStatus::Ok => WorkerStatus::Ok,
            assignment_fb::WorkerStatus::Unreliable => WorkerStatus::Unreliable,
            assignment_fb::WorkerStatus::DeprecatedVersion => WorkerStatus::DeprecatedVersion,
            assignment_fb::WorkerStatus::UnsupportedVersion => WorkerStatus::UnsupportedVersion,
            _ => WorkerStatus::UnsupportedVersion,
        }
    }

    pub fn decrypt_headers(&self, key: &Keypair) -> anyhow::Result<BTreeMap<String, String>> {
        let secret_key = key.clone().try_into_ed25519()?.secret();
        let headers = self
            .reader
            .encrypted_headers()
            .ok_or(anyhow!("EncryptedHeaders field missing"))?;
        let common_public_key = PublicKey::from_slice(headers.identity().bytes())?;
        let secret_hash = Sha512::digest(secret_key);
        let worker_secret_key = SecretKey::from_slice(&secret_hash[..32])?;
        let shared_box = SalsaBox::new(&common_public_key, &worker_secret_key);
        let nonce = GenericArray::from_slice(headers.nonce().bytes());
        let plaintext_bytes = shared_box.decrypt(nonce, headers.ciphertext().bytes())?;

        let plaintext = std::str::from_utf8(&plaintext_bytes)?;
        let json = serde_json::from_str::<serde_json::Value>(plaintext)?;
        let map = json
            .as_object()
            .ok_or(anyhow!("Parsed headers JSON is not an object"))?
            .into_iter()
            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_owned())))
            .collect();
        Ok(map)
    }
}
