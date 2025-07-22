use std::{collections::HashMap, ops::RangeInclusive};

use crypto_box::{
    aead::{rand_core::CryptoRngCore, OsRng},
    SecretKey,
};
use flatbuffers::{self as fb, WIPOffset};
use libp2p_identity::PeerId;
use sqd_messages::assignments::timed_hmac;

use crate::common;

use super::assignment_fb::{self, Assignment, WorkerId};

pub struct AssignmentBuilder<Rng: CryptoRngCore> {
    builder: fb::FlatBufferBuilder<'static>,
    rng: Rng,
    files_list_offsets: FileListOffsets,
    all_chunks: Vec<fb::WIPOffset<assignment_fb::Chunk<'static>>>,
    last_block: Option<u64>,
    current_chunks: Vec<fb::WIPOffset<assignment_fb::Chunk<'static>>>,
    current_dataset_id_offset: Option<fb::WIPOffset<&'static str>>,
    all_datasets: Vec<fb::WIPOffset<assignment_fb::Dataset<'static>>>,
    worker_assignments: Vec<(WorkerId, fb::WIPOffset<assignment_fb::WorkerAssignment<'static>>)>,
    last_peer_id: Option<PeerId>,
    cloudflare_storage_secret: String,
    common_identity: fb::WIPOffset<fb::Vector<'static, u8>>,
    common_secret_key: SecretKey,
}

impl AssignmentBuilder<OsRng> {
    pub fn new(cloudflare_storage_secret: impl Into<String>) -> Self {
        Self::new_with_rng(cloudflare_storage_secret, OsRng)
    }
}

impl<Rng: CryptoRngCore> AssignmentBuilder<Rng> {
    pub fn new_with_rng(cloudflare_storage_secret: impl Into<String>, mut rng: Rng) -> Self {
        let mut builder = flatbuffers::FlatBufferBuilder::new();
        let common_secret_key = SecretKey::generate(&mut rng);
        let common_public_key_bytes = *common_secret_key.public_key().as_bytes();
        let common_identity = builder.create_vector(&common_public_key_bytes);
        Self {
            builder,
            rng,
            files_list_offsets: HashMap::new(),
            all_chunks: Vec::new(),
            last_block: None,
            current_chunks: Vec::new(),
            current_dataset_id_offset: None,
            all_datasets: Vec::new(),
            worker_assignments: Vec::new(),
            last_peer_id: None,
            cloudflare_storage_secret: cloudflare_storage_secret.into(),
            common_identity,
            common_secret_key,
        }
    }

    pub fn new_chunk(&mut self) -> ChunkBuilder<'_, Rng> {
        ChunkBuilder::new(self)
    }

    pub fn finish_dataset(&mut self) {
        let chunks = self.builder.create_vector(&self.current_chunks);
        let offset = assignment_fb::Dataset::create(
            &mut self.builder,
            &assignment_fb::DatasetArgs {
                id: self.current_dataset_id_offset.take(),
                chunks: Some(chunks),
                last_block: self
                    .last_block
                    .take()
                    .expect("At least one chunk should be present in the dataset"),
            },
        );
        self.all_datasets.push(offset);
        self.current_chunks.clear();
    }

    pub fn add_worker(&mut self, id: PeerId, status: common::WorkerStatus, chunk_indexes: &[u32]) {
        let timestamp = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs().try_into().unwrap();
        self.add_worker_with_timestamp(id, status, chunk_indexes, timestamp);
    }

    pub fn add_worker_with_timestamp(
        &mut self,
        id: PeerId,
        status: common::WorkerStatus,
        chunk_indexes: &[u32],
        timestamp: usize,
    ) {
        if let Some(last) = self.last_peer_id {
            assert!(last < id, "Workers must be added in ascending order of their PeerIDs");
        }
        self.last_peer_id = Some(id);

        let worker_id = WorkerId::from(id);
        let chunks = self
            .builder
            .create_vector_from_iter(chunk_indexes.iter().map(|&i| self.all_chunks[i as usize]));
        let status = match status {
            crate::WorkerStatus::Ok => assignment_fb::WorkerStatus::Ok,
            crate::WorkerStatus::Unreliable => assignment_fb::WorkerStatus::Unreliable,
            crate::WorkerStatus::DeprecatedVersion => {
                assignment_fb::WorkerStatus::DeprecatedVersion
            }
            crate::WorkerStatus::UnsupportedVersion => {
                assignment_fb::WorkerStatus::UnsupportedVersion
            }
        };

        let encrypted_headers = self
            .generate_encrypted_headers(&id, timestamp)
            .inspect_err(|e| {
                tracing::warn!("Failed to encrypt headers for worker {}: {}", id, e);
            })
            .ok();
        let offset = assignment_fb::WorkerAssignment::create(
            &mut self.builder,
            &assignment_fb::WorkerAssignmentArgs {
                worker_id: Some(&worker_id),
                chunks: Some(chunks),
                status,
                encrypted_headers,
            },
        );
        self.worker_assignments.push((worker_id, offset));
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let datasets = self.builder.create_vector(&self.all_datasets);

        let workers = self
            .builder
            .create_vector_from_iter(self.worker_assignments.iter().map(|(_, offset)| *offset));

        let assignment = Assignment::create(
            &mut self.builder,
            &assignment_fb::AssignmentArgs {
                datasets: Some(datasets),
                workers: Some(workers),
            },
        );

        self.builder.finish(assignment, None);
        self.builder.finished_data().to_vec()
    }

    fn add_chunk(
        &mut self,
        offset: fb::WIPOffset<assignment_fb::Chunk<'static>>,
        dataset: WIPOffset<&'static str>,
        block_range: RangeInclusive<u64>,
    ) {
        self.all_chunks.push(offset);
        if let Some(last) = self.last_block {
            assert_eq!(last + 1, *block_range.start(), "Chunks in the dataset must be contiguous");
        }
        self.last_block = Some(*block_range.end());
        self.current_chunks.push(offset);
        self.current_dataset_id_offset = Some(dataset);
    }

    fn cache_files_list(
        &mut self,
        files: &[String],
    ) -> fb::WIPOffset<fb::Vector<'static, fb::ForwardsUOffset<assignment_fb::FileUrl<'static>>>>
    {
        match self.files_list_offsets.get(files) {
            Some(&offset) => offset,
            None => {
                let file_offsets: Vec<_> = files
                    .iter()
                    .map(|f| {
                        let name = self.builder.create_string(f);
                        assignment_fb::FileUrl::create(
                            &mut self.builder,
                            &assignment_fb::FileUrlArgs {
                                filename: Some(name),
                                url: Some(name),
                            },
                        )
                    })
                    .collect();
                let offset = self.builder.create_vector(&file_offsets);
                self.files_list_offsets.insert(files.to_vec(), offset);
                offset
            }
        }
    }

    fn generate_encrypted_headers(
        &mut self,
        peer_id: &PeerId,
        timestamp: usize,
    ) -> anyhow::Result<fb::WIPOffset<assignment_fb::EncryptedHeaders<'static>>> {
        let id = peer_id.to_string();
        let worker_signature = timed_hmac(&id, &self.cloudflare_storage_secret, timestamp);
        let plaintext =
            format!(r#"{{"worker-id":"{}","worker-signature":"{}"}}"#, id, worker_signature);

        let (ciphertext, nonce) = sqd_messages::assignments::encrypt_with_rng(
            &id,
            &self.common_secret_key,
            plaintext.as_bytes(),
            &mut self.rng,
        )?;

        let ciphertext_offset = self.builder.create_vector(&ciphertext);
        let nonce_offset = self.builder.create_vector(&nonce);
        Ok(assignment_fb::EncryptedHeaders::create(
            &mut self.builder,
            &assignment_fb::EncryptedHeadersArgs {
                identity: Some(self.common_identity),
                nonce: Some(nonce_offset),
                ciphertext: Some(ciphertext_offset),
            },
        ))
    }
}

#[test]
fn test_json_formatting() {
    let s = format!(r#"{{"worker-id":"{}","worker-signature":"{}"}}"#, "test-id", "test-signature");
    assert_eq!(s, "{\"worker-id\":\"test-id\",\"worker-signature\":\"test-signature\"}");
}

type FileListOffsets = HashMap<
    Vec<String>,
    fb::WIPOffset<fb::Vector<'static, fb::ForwardsUOffset<assignment_fb::FileUrl<'static>>>>,
>;

pub struct ChunkBuilder<'b, Rng: CryptoRngCore> {
    p: &'b mut AssignmentBuilder<Rng>,

    block_range: Option<RangeInclusive<u64>>,
    id: Option<fb::WIPOffset<&'static str>>,
    dataset_id: Option<fb::WIPOffset<&'static str>>,
    size: Option<u32>,
    last_block_hash: Option<fb::WIPOffset<&'static str>>,
    last_block_timestamp: Option<u64>,
    dataset_base_url: Option<fb::WIPOffset<&'static str>>,
    files: Option<
        fb::WIPOffset<fb::Vector<'static, fb::ForwardsUOffset<assignment_fb::FileUrl<'static>>>>,
    >,
    worker_indexes: Option<fb::WIPOffset<fb::Vector<'static, u16>>>,
}

impl<'b, Rng: CryptoRngCore> ChunkBuilder<'b, Rng> {
    pub fn new(parent: &'b mut AssignmentBuilder<Rng>) -> Self {
        Self {
            p: parent,
            block_range: None,
            id: None,
            dataset_id: None,
            size: None,
            last_block_hash: None,
            last_block_timestamp: None,
            dataset_base_url: None,
            files: None,
            worker_indexes: None,
        }
    }

    pub fn id(mut self, id: &str) -> Self {
        self.id = Some(self.p.builder.create_string(id));
        self
    }

    pub fn dataset_id(mut self, dataset_id: &str) -> Self {
        self.dataset_id = Some(self.p.builder.create_shared_string(dataset_id));
        self
    }

    pub fn block_range(mut self, range: RangeInclusive<u64>) -> Self {
        self.block_range = Some(range);
        self
    }

    pub fn size(mut self, size: u32) -> Self {
        self.size = Some(size);
        self
    }

    pub fn last_block_hash(mut self, hash: &str) -> Self {
        self.last_block_hash = Some(self.p.builder.create_string(hash));
        self
    }

    pub fn last_block_timestamp(mut self, timestamp: u64) -> Self {
        self.last_block_timestamp = Some(timestamp);
        self
    }

    pub fn dataset_base_url(mut self, url: &str) -> Self {
        self.dataset_base_url = Some(self.p.builder.create_shared_string(url));
        self
    }

    pub fn worker_indexes(mut self, indexes: &[u16]) -> Self {
        self.worker_indexes = Some(self.p.builder.create_vector(indexes));
        self
    }

    pub fn files(mut self, files: &[String]) -> Self {
        self.files = Some(self.p.cache_files_list(files));
        self
    }

    pub fn finish(self) {
        let block_range = self.block_range.expect("Block range must be set");
        let offset = assignment_fb::Chunk::create(
            &mut self.p.builder,
            &assignment_fb::ChunkArgs {
                id: self.id,
                first_block: *block_range.start(),
                last_block_hash: self.last_block_hash,
                last_block_timestamp: self.last_block_timestamp,
                dataset_id: self.dataset_id,
                size: self.size.expect("Size must be set"),
                dataset_base_url: self.dataset_base_url,
                base_url: self.id,
                files: self.files,
                worker_indexes: self.worker_indexes,
            },
        );
        self.p
            .add_chunk(offset, self.dataset_id.expect("Dataset ID must be set"), block_range);
    }
}
