use std::{
    collections::{btree_map::Entry, BTreeMap, HashMap},
    ops::RangeInclusive,
};

use anyhow::Context as _;
use crypto_box::{
    aead::{rand_core::CryptoRngCore, OsRng},
    SecretKey,
};
use flatbuffers::{self as fb, WIPOffset};
use libp2p_identity::PeerId;

use crate::{
    common,
    signatures::{encrypt_with_rng, timed_hmac},
};

use super::assignment_fb::{self, Assignment, WorkerId};

fn status_to_fb(status: common::WorkerStatus) -> assignment_fb::WorkerStatus {
    match status {
        crate::WorkerStatus::Ok => assignment_fb::WorkerStatus::Ok,
        crate::WorkerStatus::Unreliable => assignment_fb::WorkerStatus::Unreliable,
        crate::WorkerStatus::DeprecatedVersion => assignment_fb::WorkerStatus::DeprecatedVersion,
        crate::WorkerStatus::UnsupportedVersion => assignment_fb::WorkerStatus::UnsupportedVersion,
    }
}

pub struct AssignmentBuilder<Rng: CryptoRngCore> {
    builder: fb::FlatBufferBuilder<'static>,
    rng: Rng,
    files_list_offsets: FileListOffsets,
    all_chunks: Vec<fb::WIPOffset<assignment_fb::Chunk<'static>>>,
    last_block: Option<u64>,
    current_chunks: Vec<fb::WIPOffset<assignment_fb::Chunk<'static>>>,
    current_dataset_id_offset: Option<fb::WIPOffset<&'static str>>,
    all_datasets: Vec<fb::WIPOffset<assignment_fb::Dataset<'static>>>,
    worker_assignments: Vec<(WorkerId, fb::WIPOffset<assignment_fb::WorkerEntry<'static>>)>,
    last_peer_id: Option<PeerId>,
    cloudflare_storage_secret: String,
    common_identity: fb::WIPOffset<fb::Vector<'static, u8>>,
    common_secret_key: SecretKey,
    check_continuity: bool,
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
            check_continuity: true,
        }
    }

    /// If this check is enabled, the chunk breaking the continuity condition won't be added and an error will be returned.
    /// If it's disabled, the chunk will be added but the error will still be returned for logging purposes.
    /// Enabled by default.
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn check_continuity(mut self, check: bool) -> Self {
        self.check_continuity = check;
        self
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

    /// `chunk_indexes` is accepted for backward API compatibility but no longer used: the
    /// generated `WorkerEntryArgs` excludes the `chunks` field entirely now that it's
    /// `(deprecated)` in the schema — flatc won't let a deprecated field be populated by new
    /// code. Chunk-to-worker mappings travel via each chunk's `worker_indexes` instead.
    pub fn add_worker(&mut self, id: PeerId, status: common::WorkerStatus, _chunk_indexes: &[u32]) {
        let timestamp = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs().try_into().unwrap();
        self.add_worker_with_timestamp(id, status, _chunk_indexes, timestamp);
    }

    pub fn add_worker_with_timestamp(
        &mut self,
        id: PeerId,
        status: common::WorkerStatus,
        _chunk_indexes: &[u32],
        timestamp: usize,
    ) {
        if let Some(last) = self.last_peer_id {
            assert!(last < id, "Workers must be added in ascending order of their PeerIDs");
        }
        self.last_peer_id = Some(id);

        let worker_id = WorkerId::from(id);
        let status = status_to_fb(status);

        let encrypted_headers = self
            .generate_encrypted_headers(&id, timestamp)
            .inspect_err(|e| {
                tracing::warn!("Failed to encrypt headers for worker {}: {}", id, e);
            })
            .ok();
        let offset = assignment_fb::WorkerEntry::create(
            &mut self.builder,
            &assignment_fb::WorkerEntryArgs {
                worker_id: Some(&worker_id),
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
    ) -> anyhow::Result<()> {
        let result = match self.last_block {
            Some(last) if last + 1 != *block_range.start() => Err(anyhow::anyhow!(
                "Chunks in the dataset must be contiguous, got {} -> {}",
                last,
                block_range.start()
            )),
            _ => Ok(()),
        };
        if result.is_ok() || !self.check_continuity {
            self.all_chunks.push(offset);
            self.last_block = Some(*block_range.end());
            self.current_chunks.push(offset);
            self.current_dataset_id_offset = Some(dataset);
        }
        result
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

        let (ciphertext, nonce) = encrypt_with_rng(
            peer_id,
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

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn id(mut self, id: &str) -> Self {
        self.id = Some(self.p.builder.create_string(id));
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn dataset_id(mut self, dataset_id: &str) -> Self {
        self.dataset_id = Some(self.p.builder.create_shared_string(dataset_id));
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn block_range(mut self, range: RangeInclusive<u64>) -> Self {
        self.block_range = Some(range);
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn size(mut self, size: u32) -> Self {
        self.size = Some(size);
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn last_block_hash(mut self, hash: &str) -> Self {
        self.last_block_hash = Some(self.p.builder.create_string(hash));
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn last_block_timestamp(mut self, timestamp: u64) -> Self {
        self.last_block_timestamp = Some(timestamp);
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn dataset_base_url(mut self, url: &str) -> Self {
        self.dataset_base_url = Some(self.p.builder.create_shared_string(url));
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn worker_indexes(mut self, indexes: &[u16]) -> Self {
        self.worker_indexes = Some(self.p.builder.create_vector(indexes));
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn files(mut self, files: &[String]) -> Self {
        self.files = Some(self.p.cache_files_list(files));
        self
    }

    pub fn finish(self) -> anyhow::Result<()> {
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
            .add_chunk(offset, self.dataset_id.expect("Dataset ID must be set"), block_range)
    }
}

// ===== Worker-facing assignment (NET-1180) =====
//
// Same continuity-checked chunk/dataset staging as `AssignmentBuilder`, minus the file-list
// machinery (a worker resolves its chunks' files from the schema, not a wire-provided list — see
// docs/assignment-wire-format.md in network-scheduler). `WorkerEntry` is shared with the legacy
// format unchanged.

/// The columns of the dataset currently being staged, taken by
/// [`WorkerAssignmentBuilder::finish_dataset`].
#[derive(Default)]
struct WorkerDatasetColumns {
    first_blocks: Vec<u64>,
    block_deltas: Vec<u32>,
    hashes: Vec<assignment_fb::ChunkHash>,
    /// One entry per run, appended only when a chunk's top differs from the previous chunk's.
    tops: Vec<assignment_fb::TopRun>,
    sizes: Vec<u32>,
    write_schema_ids: Vec<u32>,
    versions: Vec<u32>,
    /// Flattened per-chunk bitmaps, cut up by `tables_present_ends`.
    tables_present: Vec<u8>,
    /// Where each chunk's bitmap ends; the emitted column is a leading 0 then these.
    tables_present_ends: Vec<u32>,
    /// Whether any chunk trimmed its tables; if none did, both columns are dropped.
    any_tables_trimmed: bool,
    /// Where each chunk's worker slice ends; the emitted CSR column is a leading 0 then these.
    worker_ends: Vec<u32>,
    worker_indexes: Vec<u16>,
    /// The base url every chunk of the dataset hangs off, kept as a string so a second chunk
    /// naming a different one is caught rather than silently dropped.
    base_url: Option<(String, fb::WIPOffset<&'static str>)>,
}

pub struct WorkerAssignmentBuilder<Rng: CryptoRngCore> {
    builder: fb::FlatBufferBuilder<'static>,
    rng: Rng,
    last_block: Option<u64>,
    current_dataset_id_offset: Option<fb::WIPOffset<&'static str>>,
    all_datasets: Vec<fb::WIPOffset<assignment_fb::WorkerAssignmentDataset<'static>>>,
    worker_entries: Vec<(WorkerId, fb::WIPOffset<assignment_fb::WorkerEntry<'static>>)>,
    last_peer_id: Option<PeerId>,
    cloudflare_storage_secret: String,
    /// Written on first use, not at construction: an assignment whose workers all carry copied
    /// headers never seals anything, and eagerly writing 32 random bytes into the buffer would
    /// leave them unreferenced and make an otherwise reproducible build differ every run.
    common_identity: Option<fb::WIPOffset<fb::Vector<'static, u8>>>,
    common_secret_key: SecretKey,
    check_continuity: bool,
    /// `BTreeMap` so `finish` emits rosters id-sorted, as `TableRoster`'s `(key)` lookup requires.
    write_schemas: BTreeMap<u32, Vec<String>>,
    /// Generation prefixes of the dataset being staged, cleared by `finish_dataset` — unlike write
    /// schemas, they are per-dataset. `BTreeMap` so they reach the blob version-sorted, as
    /// `GenerationEntry`'s `(key)` lookup requires.
    current_generations: BTreeMap<u32, String>,
    columns: WorkerDatasetColumns,
}

impl WorkerAssignmentBuilder<OsRng> {
    pub fn new(cloudflare_storage_secret: impl Into<String>) -> Self {
        Self::new_with_rng(cloudflare_storage_secret, OsRng)
    }
}

impl<Rng: CryptoRngCore> WorkerAssignmentBuilder<Rng> {
    pub fn new_with_rng(cloudflare_storage_secret: impl Into<String>, mut rng: Rng) -> Self {
        let builder = flatbuffers::FlatBufferBuilder::new();
        let common_secret_key = SecretKey::generate(&mut rng);
        Self {
            builder,
            rng,
            last_block: None,
            current_dataset_id_offset: None,
            all_datasets: Vec::new(),
            worker_entries: Vec::new(),
            last_peer_id: None,
            cloudflare_storage_secret: cloudflare_storage_secret.into(),
            common_identity: None,
            common_secret_key,
            check_continuity: true,
            write_schemas: BTreeMap::new(),
            current_generations: BTreeMap::new(),
            columns: WorkerDatasetColumns::default(),
        }
    }

    /// See [`AssignmentBuilder::check_continuity`]. Note that a gap only trips this check; it is
    /// no longer something the reader can misread, since `block_deltas` carries each chunk's end.
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn check_continuity(mut self, check: bool) -> Self {
        self.check_continuity = check;
        self
    }

    /// Register a write schema's table roster: the schema's full declared table list, which each
    /// chunk's `tables_present` selects from. Every write schema a chunk references must be
    /// registered before [`Self::finish`]. `tables` must be sorted and duplicate-free.
    ///
    /// # Errors
    ///
    /// If `tables` is not strictly ascending, or if the id was already registered with a
    /// different roster — staged chunks' bitmaps are already encoded against the old ordering.
    pub fn register_write_schema<S: AsRef<str>>(
        &mut self,
        write_schema_id: u32,
        tables: &[S],
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            tables.is_sorted_by(|a, b| a.as_ref() < b.as_ref()),
            "write schema {write_schema_id}'s roster must be sorted and free of duplicates"
        );
        match self.write_schemas.entry(write_schema_id) {
            Entry::Occupied(existing) => {
                let registered = existing.get();
                let unchanged = registered.len() == tables.len()
                    && std::iter::zip(registered, tables).all(|(old, new)| old == new.as_ref());
                anyhow::ensure!(
                    unchanged,
                    "write schema {write_schema_id} re-registered with a different table roster"
                );
            }
            Entry::Vacant(slot) => {
                slot.insert(tables.iter().map(|t| t.as_ref().to_owned()).collect());
            }
        }
        Ok(())
    }

    /// Register the storage prefix a batch job wrote a generation of chunks under, relative to
    /// the dataset's base url (e.g. `_bf/01HQZK3M7X8P2NVWTC4RYFGDS9`). Applies to the dataset
    /// currently being staged, so it must be called before the chunks carrying that version and
    /// re-registered for every dataset the generation covers.
    ///
    /// # Errors
    ///
    /// If `version` is 0 — a perfectly normal version, but the ingested one, which is defined by
    /// having no entry here — or if the version was already registered for this dataset with a
    /// different prefix.
    pub fn register_generation(&mut self, version: u32, base_url: &str) -> anyhow::Result<()> {
        anyhow::ensure!(
            version != 0,
            "version 0 is the ingested layout, which needs no generation entry"
        );
        match self.current_generations.entry(version) {
            Entry::Occupied(existing) => anyhow::ensure!(
                existing.get() == base_url,
                "generation {version} re-registered with a different base url"
            ),
            Entry::Vacant(slot) => {
                slot.insert(base_url.to_owned());
            }
        }
        Ok(())
    }

    pub fn new_chunk(&mut self) -> WorkerAssignmentChunkBuilder<'_, Rng> {
        WorkerAssignmentChunkBuilder::new(self)
    }

    /// Emits the staged chunks as the dataset's columns.
    ///
    /// # Errors
    ///
    /// If no chunk was staged, or if the run columns don't start at chunk 0 and ascend — which
    /// the way chunks are staged already guarantees, but the reader's search depends on it.
    pub fn finish_dataset(&mut self) -> anyhow::Result<()> {
        let columns = std::mem::take(&mut self.columns);
        let generations = self.create_generations();
        let chunk_count = columns.first_blocks.len();
        anyhow::ensure!(chunk_count > 0, "At least one chunk should be present in the dataset");
        check_runs(columns.tops.iter().map(|run| run.first_chunk_index()), "top")?;
        let first_blocks = self.builder.create_vector(&columns.first_blocks);
        let block_deltas = self.builder.create_vector(&columns.block_deltas);
        let hashes = self.builder.create_vector(&columns.hashes);
        let tops = self.builder.create_vector(&columns.tops);
        let sizes = self.builder.create_vector(&columns.sizes);
        let write_schema_ids = self.builder.create_vector(&columns.write_schema_ids);
        let (tables_present_offsets, tables_present) = if columns.any_tables_trimmed {
            let mut offsets = Vec::with_capacity(columns.tables_present_ends.len() + 1);
            offsets.push(0);
            offsets.extend_from_slice(&columns.tables_present_ends);
            (
                Some(self.builder.create_vector(&offsets)),
                Some(self.builder.create_vector(&columns.tables_present)),
            )
        } else {
            (None, None)
        };
        let versions = columns
            .versions
            .iter()
            .any(|&version| version != 0)
            .then(|| self.builder.create_vector(&columns.versions));
        // A leading 0, so slot i is the start of chunk i's slice and slot i + 1 its end.
        let mut worker_offsets = Vec::with_capacity(columns.worker_ends.len() + 1);
        worker_offsets.push(0);
        worker_offsets.extend_from_slice(&columns.worker_ends);
        let worker_offsets = self.builder.create_vector(&worker_offsets);
        let worker_indexes = self.builder.create_vector(&columns.worker_indexes);

        let offset = assignment_fb::WorkerAssignmentDataset::create(
            &mut self.builder,
            &assignment_fb::WorkerAssignmentDatasetArgs {
                id: self.current_dataset_id_offset.take(),
                last_block: self
                    .last_block
                    .take()
                    .expect("At least one chunk should be present in the dataset"),
                base_url: columns.base_url.map(|(_, offset)| offset),
                generations,
                first_blocks: Some(first_blocks),
                block_deltas: Some(block_deltas),
                hashes: Some(hashes),
                tops: Some(tops),
                sizes: Some(sizes),
                write_schema_ids: Some(write_schema_ids),
                tables_present_offsets,
                tables_present,
                versions,
                worker_offsets: Some(worker_offsets),
                worker_indexes: Some(worker_indexes),
            },
        );
        self.all_datasets.push(offset);
        self.current_generations.clear();
        Ok(())
    }

    /// `None` for a dataset whose chunks are all version 0, so the common case stores nothing.
    fn create_generations(
        &mut self,
    ) -> Option<
        fb::WIPOffset<
            fb::Vector<'static, fb::ForwardsUOffset<assignment_fb::GenerationEntry<'static>>>,
        >,
    > {
        if self.current_generations.is_empty() {
            return None;
        }
        // Destructured so the prefixes can be read while `builder` is mutably borrowed.
        let Self {
            builder,
            current_generations,
            ..
        } = self;
        let offsets: Vec<_> = current_generations
            .iter()
            .map(|(version, base_url)| {
                let base_url = builder.create_shared_string(base_url);
                assignment_fb::GenerationEntry::create(
                    builder,
                    &assignment_fb::GenerationEntryArgs {
                        version: *version,
                        base_url: Some(base_url),
                    },
                )
            })
            .collect();
        Some(builder.create_vector(&offsets))
    }

    pub fn add_worker(&mut self, id: PeerId, status: common::WorkerStatus) {
        let timestamp = std::time::UNIX_EPOCH.elapsed().unwrap().as_secs().try_into().unwrap();
        self.add_worker_with_timestamp(id, status, timestamp);
    }

    pub fn add_worker_with_timestamp(
        &mut self,
        id: PeerId,
        status: common::WorkerStatus,
        timestamp: usize,
    ) {
        if let Some(last) = self.last_peer_id {
            assert!(last < id, "Workers must be added in ascending order of their PeerIDs");
        }
        self.last_peer_id = Some(id);

        let worker_id = WorkerId::from(id);
        let status = status_to_fb(status);
        let encrypted_headers = self
            .generate_encrypted_headers(&id, timestamp)
            .inspect_err(|e| {
                tracing::warn!("Failed to encrypt headers for worker {}: {}", id, e);
            })
            .ok();
        let offset = assignment_fb::WorkerEntry::create(
            &mut self.builder,
            &assignment_fb::WorkerEntryArgs {
                worker_id: Some(&worker_id),
                status,
                encrypted_headers,
            },
        );
        self.worker_entries.push((worker_id, offset));
    }

    /// Adds a worker carrying headers already sealed by someone else, copied byte for byte.
    ///
    /// [`Self::add_worker`] mints headers from the Cloudflare secret, which an assignment being
    /// re-emitted from another one doesn't have — the secret never travels with the blob. The
    /// sealed bytes do, and they stay valid for the worker that owns the key: `identity` is the
    /// public half of the ephemeral key they were sealed under, and the ciphertext is bound to it.
    ///
    /// The signature inside carries whatever timestamp it was minted with, so headers copied from
    /// an old assignment are as expired as their source.
    pub fn add_worker_with_sealed_headers(
        &mut self,
        id: PeerId,
        status: common::WorkerStatus,
        identity: &[u8],
        nonce: &[u8],
        ciphertext: &[u8],
    ) {
        if let Some(last) = self.last_peer_id {
            assert!(last < id, "Workers must be added in ascending order of their PeerIDs");
        }
        self.last_peer_id = Some(id);

        let identity = self.builder.create_vector(identity);
        let nonce = self.builder.create_vector(nonce);
        let ciphertext = self.builder.create_vector(ciphertext);
        let encrypted_headers = assignment_fb::EncryptedHeaders::create(
            &mut self.builder,
            &assignment_fb::EncryptedHeadersArgs {
                identity: Some(identity),
                nonce: Some(nonce),
                ciphertext: Some(ciphertext),
            },
        );

        let worker_id = WorkerId::from(id);
        let offset = assignment_fb::WorkerEntry::create(
            &mut self.builder,
            &assignment_fb::WorkerEntryArgs {
                worker_id: Some(&worker_id),
                status: status_to_fb(status),
                encrypted_headers: Some(encrypted_headers),
            },
        );
        self.worker_entries.push((worker_id, offset));
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let schemas = self.create_write_schema_rosters();
        let datasets = self.builder.create_vector(&self.all_datasets);
        let workers = self
            .builder
            .create_vector_from_iter(self.worker_entries.iter().map(|(_, offset)| *offset));

        let root = assignment_fb::WorkerAssignment::create(
            &mut self.builder,
            &assignment_fb::WorkerAssignmentArgs {
                datasets: Some(datasets),
                workers: Some(workers),
                schemas: Some(schemas),
            },
        );

        self.builder.finish(root, None);
        self.builder.finished_data().to_vec()
    }

    fn create_write_schema_rosters(
        &mut self,
    ) -> fb::WIPOffset<fb::Vector<'static, fb::ForwardsUOffset<assignment_fb::TableRoster<'static>>>>
    {
        // Destructured so the rosters can be read while `builder` is mutably borrowed.
        let Self {
            builder,
            write_schemas,
            ..
        } = self;
        let offsets: Vec<_> = write_schemas
            .iter()
            .map(|(write_schema_id, tables)| {
                let table_offsets: Vec<_> =
                    tables.iter().map(|t| builder.create_shared_string(t)).collect();
                let tables = builder.create_vector(&table_offsets);
                assignment_fb::TableRoster::create(
                    builder,
                    &assignment_fb::TableRosterArgs {
                        write_schema_id: *write_schema_id,
                        tables: Some(tables),
                    },
                )
            })
            .collect();
        builder.create_vector(&offsets)
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

        let (ciphertext, nonce) = encrypt_with_rng(
            peer_id,
            &self.common_secret_key,
            plaintext.as_bytes(),
            &mut self.rng,
        )?;

        let ciphertext_offset = self.builder.create_vector(&ciphertext);
        let nonce_offset = self.builder.create_vector(&nonce);
        let identity = match self.common_identity {
            Some(identity) => identity,
            None => {
                let bytes = *self.common_secret_key.public_key().as_bytes();
                *self.common_identity.insert(self.builder.create_vector(&bytes))
            }
        };
        Ok(assignment_fb::EncryptedHeaders::create(
            &mut self.builder,
            &assignment_fb::EncryptedHeadersArgs {
                identity: Some(identity),
                nonce: Some(nonce_offset),
                ciphertext: Some(ciphertext_offset),
            },
        ))
    }
}

/// The searched `tops` column is only sound if it starts at chunk 0 and ascends; otherwise the
/// reader's "last run at or before this chunk" has nothing to land on.
fn check_runs(mut starts: impl Iterator<Item = u32>, what: &str) -> anyhow::Result<()> {
    let Some(first) = starts.next() else {
        anyhow::bail!("a dataset with chunks must have at least one {what} run");
    };
    anyhow::ensure!(first == 0, "the first {what} run must start at chunk 0");
    let mut previous = first;
    for start in starts {
        anyhow::ensure!(start > previous, "{what} runs must strictly ascend by first_chunk_index");
        previous = start;
    }
    Ok(())
}

pub struct WorkerAssignmentChunkBuilder<'b, Rng: CryptoRngCore> {
    p: &'b mut WorkerAssignmentBuilder<Rng>,

    block_range: Option<RangeInclusive<u64>>,
    id: Option<String>,
    dataset_id: Option<fb::WIPOffset<&'static str>>,
    size: Option<u32>,
    dataset_base_url: Option<String>,
    version: u32,
    write_schema_id: Option<u32>,
    /// The bitmap and the write schema it was encoded against — they can diverge if
    /// `write_schema_id` is set again afterwards, which `finish` rejects.
    tables_present: Option<(u32, Vec<u8>)>,
    worker_indexes: Vec<u16>,
}

impl<'b, Rng: CryptoRngCore> WorkerAssignmentChunkBuilder<'b, Rng> {
    pub fn new(parent: &'b mut WorkerAssignmentBuilder<Rng>) -> Self {
        Self {
            p: parent,
            block_range: None,
            id: None,
            dataset_id: None,
            size: None,
            dataset_base_url: None,
            version: 0,
            write_schema_id: None,
            tables_present: None,
            worker_indexes: Vec::new(),
        }
    }

    /// The chunk id, e.g. `"0221000000/0221000000-0221000649-9QgFD"`. Not stored as such: it is
    /// split into the `tops`, `first_blocks`, `block_deltas` and `hashes` columns and reassembled
    /// on read, so it must parse — see [`Self::finish`].
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn id(mut self, id: &str) -> Self {
        self.id = Some(id.to_owned());
        self
    }

    /// Which dataset the chunk belongs to. Names the dataset it is staged into; chunks don't
    /// carry the id themselves, since they are only ever read through that dataset.
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn dataset_id(mut self, dataset_id: &str) -> Self {
        self.dataset_id = Some(self.p.builder.create_shared_string(dataset_id));
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn block_range(mut self, range: RangeInclusive<u64>) -> Self {
        self.block_range = Some(range);
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn size(mut self, size: u32) -> Self {
        self.size = Some(size);
        self
    }

    /// Where the dataset's files live. Held once on the dataset, so every chunk of one must name
    /// the same url — [`Self::finish`] rejects a chunk that disagrees.
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn dataset_base_url(mut self, url: &str) -> Self {
        self.dataset_base_url = Some(url.to_owned());
        self
    }

    /// Which copy of the chunk workers should download. Defaults to 0, the ingested one; any other
    /// value must have been registered with
    /// [`WorkerAssignmentBuilder::register_generation`] for this dataset.
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    /// The write schema the chunk was written under — the one covering every table and column
    /// physically present in it. Must be registered with
    /// [`WorkerAssignmentBuilder::register_write_schema`].
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn write_schema_id(mut self, write_schema_id: u32) -> Self {
        self.write_schema_id = Some(write_schema_id);
        self
    }

    /// The subset of the write schema's tables this chunk contains, encoded as a bitmap over its
    /// roster. Omit when every table is present. `tables` must be sorted and duplicate-free like
    /// the roster, so the two merge in one pass; only debug builds assert it, but an unsorted list
    /// still errors below rather than yielding a wrong bitmap.
    ///
    /// # Errors
    ///
    /// If [`Self::write_schema_id`] is unset or names an unregistered schema, or a table is
    /// absent from that schema's roster — which means chunk and write schema disagree.
    pub fn tables_present<S: AsRef<str>>(mut self, tables: &[S]) -> anyhow::Result<Self> {
        debug_assert!(
            tables.is_sorted_by(|a, b| a.as_ref() < b.as_ref()),
            "tables_present must be sorted and free of duplicates"
        );
        let write_schema_id = self
            .write_schema_id
            .context("write_schema_id must be set before tables_present")?;
        let roster = self
            .p
            .write_schemas
            .get(&write_schema_id)
            .with_context(|| format!("write schema {write_schema_id} is not registered"))?;
        let mut bits = vec![0u8; roster.len().div_ceil(8)];
        // Both sides are sorted, so one pass over each suffices: the roster cursor only ever
        // moves forward.
        let mut index = 0;
        for table in tables {
            let name = table.as_ref();
            while index < roster.len() && roster[index].as_str() < name {
                index += 1;
            }
            anyhow::ensure!(
                roster.get(index).is_some_and(|t| t == name),
                "table '{name}' is absent from write schema {write_schema_id}'s roster"
            );
            bits[index / 8] |= 1u8 << (index % 8);
            index += 1;
        }
        self.tables_present = Some((write_schema_id, bits));
        Ok(self)
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn worker_indexes(mut self, indexes: &[u16]) -> Self {
        self.worker_indexes = indexes.to_vec();
        self
    }

    /// # Panics
    ///
    /// If `block_range` or `size` was never set — those are API misuse, not bad input.
    ///
    /// # Errors
    ///
    /// If the id is unset or malformed, or disagrees with `block_range`; if `write_schema_id` is
    /// unset or names an unregistered schema; if a table isn't in that schema's roster; if a
    /// non-zero `version` has no generation registered for this dataset; if the chunk names a
    /// different base url than the dataset's other chunks; or if it breaks block continuity (see
    /// [`WorkerAssignmentBuilder::check_continuity`]).
    pub fn finish(self) -> anyhow::Result<()> {
        let block_range = self.block_range.expect("Block range must be set");
        let id = self.id.context("Chunk id must be set")?;
        let parsed = parse_chunk_id(&id)?;
        anyhow::ensure!(
            parsed.first_block == *block_range.start() && parsed.last_block == *block_range.end(),
            "chunk id '{id}' names blocks {}-{}, but the chunk covers {}-{}",
            parsed.first_block,
            parsed.last_block,
            block_range.start(),
            block_range.end()
        );
        let write_schema_id = self.write_schema_id.context("write_schema_id must be set")?;
        // Chunks with every table present never call `tables_present`, so this is the only place
        // their schema reference gets checked.
        anyhow::ensure!(
            self.p.write_schemas.contains_key(&write_schema_id),
            "write schema {write_schema_id} is not registered"
        );
        anyhow::ensure!(
            self.version == 0 || self.p.current_generations.contains_key(&self.version),
            "generation {} is not registered for this dataset",
            self.version
        );
        let bits = match self.tables_present {
            Some((encoded_against, bits)) => {
                anyhow::ensure!(
                    encoded_against == write_schema_id,
                    "tables_present is a bitmap over write schema {encoded_against}'s roster, \
                     but the chunk declares write schema {write_schema_id}"
                );
                bits
            }
            // An empty bitmap is how "every table present" travels.
            None => Vec::new(),
        };

        let base_url = self.dataset_base_url.context("dataset_base_url must be set")?;
        let dataset = self.dataset_id.expect("Dataset ID must be set");
        self.p.push_chunk(PushChunk {
            dataset,
            base_url,
            top: parsed.top,
            block_range,
            hash: parsed.hash,
            size: self.size.expect("Size must be set"),
            write_schema_id,
            version: self.version,
            bits,
            worker_indexes: &self.worker_indexes,
        })
    }
}

/// One row across every column, assembled by the chunk builder and appended by the parent.
struct PushChunk<'a> {
    dataset: WIPOffset<&'static str>,
    base_url: String,
    top: u64,
    block_range: RangeInclusive<u64>,
    hash: assignment_fb::ChunkHash,
    size: u32,
    write_schema_id: u32,
    version: u32,
    bits: Vec<u8>,
    worker_indexes: &'a [u16],
}

impl<Rng: CryptoRngCore> WorkerAssignmentBuilder<Rng> {
    /// Returns the continuity error, if any, on the same terms as
    /// [`AssignmentBuilder::check_continuity`]: with the check off the row is still appended and
    /// the error only reported.
    fn push_chunk(&mut self, chunk: PushChunk<'_>) -> anyhow::Result<()> {
        let continuity = match self.last_block {
            Some(last) if last + 1 != *chunk.block_range.start() => Err(anyhow::anyhow!(
                "Chunks in the dataset must be contiguous, got {} -> {}",
                last,
                chunk.block_range.start()
            )),
            _ => Ok(()),
        };
        if continuity.is_err() && self.check_continuity {
            return continuity;
        }

        match &self.columns.base_url {
            Some((staged, _)) => anyhow::ensure!(
                *staged == chunk.base_url,
                "chunks of one dataset must share a base url, got '{staged}' then '{}'",
                chunk.base_url
            ),
            None => {
                let offset = self.builder.create_shared_string(&chunk.base_url);
                self.columns.base_url = Some((chunk.base_url, offset));
            }
        }

        let index: u32 = self
            .columns
            .first_blocks
            .len()
            .try_into()
            .context("a dataset may hold at most u32::MAX chunks")?;
        let delta: u32 = (chunk.block_range.end() - chunk.block_range.start())
            .try_into()
            .context("a chunk may span at most u32::MAX blocks")?;

        if self.columns.tops.last().is_none_or(|run| run.top() != chunk.top) {
            self.columns.tops.push(assignment_fb::TopRun::new(index, chunk.top));
        }
        self.columns.first_blocks.push(*chunk.block_range.start());
        self.columns.block_deltas.push(delta);
        self.columns.hashes.push(chunk.hash);
        self.columns.sizes.push(chunk.size);
        self.columns.write_schema_ids.push(chunk.write_schema_id);
        self.columns.versions.push(chunk.version);
        self.columns.any_tables_trimmed |= !chunk.bits.is_empty();
        self.columns.push_bitmap(chunk.bits);
        self.columns.worker_indexes.extend_from_slice(chunk.worker_indexes);
        let end = self
            .columns
            .worker_indexes
            .len()
            .try_into()
            .context("a dataset may hold at most u32::MAX worker references")?;
        self.columns.worker_ends.push(end);

        self.last_block = Some(*chunk.block_range.end());
        self.current_dataset_id_offset = Some(chunk.dataset);
        continuity
    }
}

impl WorkerDatasetColumns {
    /// Appends one chunk's bitmap. An empty one keeps an empty slice, which is how "every table
    /// present" travels.
    ///
    /// Identical bitmaps are not shared: CSR offsets ascend, so a chunk's slice starts where the
    /// previous one ended and cannot point backwards. Nothing is lost by it — the per-chunk
    /// vectors were interned to save a 4-byte pointer each, and here the bits are inline and
    /// narrower than the pointer would have been.
    fn push_bitmap(&mut self, bits: Vec<u8>) {
        self.tables_present.extend_from_slice(&bits);
        self.tables_present_ends.push(self.tables_present.len() as u32);
    }
}

// ===== Portal-facing assignment =====
//
// No encryption, no RNG: portals never see `encrypted_headers`. Chunks are not built as tables at
// all -- each one appends a row across the dataset's columns, which `finish_dataset` then emits.
// See portal_assignment.fbs for why the columns are shaped the way they are.

/// The columns of the dataset currently being staged, taken by [`PortalAssignmentBuilder::finish_dataset`].
#[derive(Default)]
struct PortalDatasetColumns {
    first_blocks: Vec<u64>,
    block_deltas: Vec<u32>,
    hashes: Vec<assignment_fb::ChunkHash>,
    /// One entry per run, appended only when a chunk's top differs from the previous chunk's --
    /// which is what makes the first run start at index 0 and the rest strictly ascend.
    tops: Vec<assignment_fb::TopRun>,
    /// Absolute epoch milliseconds, as given. A dataset either times every chunk or none of them.
    timestamps: Vec<u64>,
    /// How many chunks carried a timestamp. The column is all-or-nothing, so by `finish_dataset`
    /// this is either 0 or the chunk count.
    timestamped: usize,
    versions: Vec<u32>,
    /// Where each chunk's worker slice *ends*; the emitted CSR column is a leading 0 followed by
    /// these, one longer than the other columns.
    worker_ends: Vec<u32>,
    worker_indexes: Vec<u16>,
}

#[derive(Default)]
pub struct PortalAssignmentBuilder {
    builder: fb::FlatBufferBuilder<'static>,
    last_block: Option<u64>,
    current_dataset_id_offset: Option<fb::WIPOffset<&'static str>>,
    all_datasets: Vec<fb::WIPOffset<assignment_fb::PortalAssignmentDataset<'static>>>,
    worker_entries: Vec<(WorkerId, fb::WIPOffset<assignment_fb::PortalEntry<'static>>)>,
    last_peer_id: Option<PeerId>,
    check_continuity: bool,
    columns: PortalDatasetColumns,
}

impl PortalAssignmentBuilder {
    pub fn new() -> Self {
        Self {
            builder: flatbuffers::FlatBufferBuilder::new(),
            check_continuity: true,
            ..Default::default()
        }
    }

    /// See [`AssignmentBuilder::check_continuity`]. Note that a gap only trips this check; it is
    /// no longer something the reader can misread, since `block_deltas` carries each chunk's end.
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn check_continuity(mut self, check: bool) -> Self {
        self.check_continuity = check;
        self
    }

    pub fn new_chunk(&mut self) -> PortalAssignmentChunkBuilder<'_> {
        PortalAssignmentChunkBuilder::new(self)
    }

    /// Emits the staged chunks as the dataset's columns.
    ///
    /// `read_schema_id` is the dataset's current read schema — a client-facing view that may hide
    /// tables, in a separate id space from [`WorkerAssignmentChunkBuilder::write_schema_id`].
    ///
    /// `last_block_hash` is the head block's full hash. The `hashes` column can't stand in for it:
    /// those are the truncated short hashes chunk ids are built from.
    ///
    /// # Errors
    ///
    /// If no chunk was staged, or if only some of them carried a timestamp — `ts_offsets` is one
    /// column over all chunks, so it is all or nothing.
    pub fn finish_dataset(
        &mut self,
        read_schema_id: u32,
        last_block_hash: Option<&str>,
    ) -> anyhow::Result<()> {
        let columns = std::mem::take(&mut self.columns);
        let chunk_count = columns.first_blocks.len();
        anyhow::ensure!(chunk_count > 0, "At least one chunk should be present in the dataset");
        anyhow::ensure!(
            columns.timestamped == 0 || columns.timestamped == chunk_count,
            "either every chunk of a dataset carries a timestamp or none does, got {} of {}",
            columns.timestamped,
            chunk_count
        );
        // Guaranteed by how `push_chunk` appends runs, but the reader's `- 1` underflows if it
        // ever stops holding, so it is checked rather than assumed.
        anyhow::ensure!(
            columns.tops.first().is_some_and(|run| run.first_chunk_index() == 0),
            "the first top run must start at chunk 0"
        );
        anyhow::ensure!(
            columns
                .tops
                .windows(2)
                .all(|w| w[0].first_chunk_index() < w[1].first_chunk_index()),
            "top runs must strictly ascend by first_chunk_index"
        );

        let last_block_hash = last_block_hash.map(|hash| self.builder.create_string(hash));
        let first_blocks = self.builder.create_vector(&columns.first_blocks);
        let block_deltas = self.builder.create_vector(&columns.block_deltas);
        let hashes = self.builder.create_vector(&columns.hashes);
        let tops = self.builder.create_vector(&columns.tops);
        let timestamps =
            (columns.timestamped > 0).then(|| self.builder.create_vector(&columns.timestamps));
        let versions = columns
            .versions
            .iter()
            .any(|&version| version != 0)
            .then(|| self.builder.create_vector(&columns.versions));
        // A leading 0, so slot i is the start of chunk i's slice and slot i + 1 its end.
        let mut worker_offsets = Vec::with_capacity(columns.worker_ends.len() + 1);
        worker_offsets.push(0);
        worker_offsets.extend_from_slice(&columns.worker_ends);
        let worker_offsets = self.builder.create_vector(&worker_offsets);
        let worker_indexes = self.builder.create_vector(&columns.worker_indexes);

        let offset = assignment_fb::PortalAssignmentDataset::create(
            &mut self.builder,
            &assignment_fb::PortalAssignmentDatasetArgs {
                id: self.current_dataset_id_offset.take(),
                last_block: self
                    .last_block
                    .take()
                    .expect("At least one chunk should be present in the dataset"),
                read_schema_id,
                last_block_hash,
                first_blocks: Some(first_blocks),
                block_deltas: Some(block_deltas),
                hashes: Some(hashes),
                tops: Some(tops),
                timestamps,
                versions,
                worker_offsets: Some(worker_offsets),
                worker_indexes: Some(worker_indexes),
            },
        );
        self.all_datasets.push(offset);
        Ok(())
    }

    pub fn add_worker(&mut self, id: PeerId, status: common::WorkerStatus) {
        if let Some(last) = self.last_peer_id {
            assert!(last < id, "Workers must be added in ascending order of their PeerIDs");
        }
        self.last_peer_id = Some(id);

        let worker_id = WorkerId::from(id);
        let offset = assignment_fb::PortalEntry::create(
            &mut self.builder,
            &assignment_fb::PortalEntryArgs {
                worker_id: Some(&worker_id),
                status: status_to_fb(status),
            },
        );
        self.worker_entries.push((worker_id, offset));
    }

    pub fn finish(&mut self) -> Vec<u8> {
        let datasets = self.builder.create_vector(&self.all_datasets);
        let workers = self
            .builder
            .create_vector_from_iter(self.worker_entries.iter().map(|(_, offset)| *offset));

        let root = assignment_fb::PortalAssignment::create(
            &mut self.builder,
            &assignment_fb::PortalAssignmentArgs {
                datasets: Some(datasets),
                workers: Some(workers),
            },
        );

        self.builder.finish(root, None);
        self.builder.finished_data().to_vec()
    }

    /// Appends one row across every column. Returns the continuity error, if any, on the same
    /// terms as [`AssignmentBuilder::check_continuity`]: with the check off the row is still
    /// appended and the error only reported.
    #[allow(clippy::too_many_arguments)]
    fn push_chunk(
        &mut self,
        dataset: WIPOffset<&'static str>,
        top: u64,
        block_range: RangeInclusive<u64>,
        hash: assignment_fb::ChunkHash,
        version: u32,
        timestamp: Option<u64>,
        worker_indexes: &[u16],
    ) -> anyhow::Result<()> {
        let continuity = match self.last_block {
            Some(last) if last + 1 != *block_range.start() => Err(anyhow::anyhow!(
                "Chunks in the dataset must be contiguous, got {} -> {}",
                last,
                block_range.start()
            )),
            _ => Ok(()),
        };
        if continuity.is_err() && self.check_continuity {
            return continuity;
        }

        let index: u32 = self
            .columns
            .first_blocks
            .len()
            .try_into()
            .context("a dataset may hold at most u32::MAX chunks")?;
        let delta: u32 = (block_range.end() - block_range.start())
            .try_into()
            .context("a chunk may span at most u32::MAX blocks")?;

        if self.columns.tops.last().is_none_or(|run| run.top() != top) {
            self.columns.tops.push(assignment_fb::TopRun::new(index, top));
        }
        self.columns.first_blocks.push(*block_range.start());
        self.columns.block_deltas.push(delta);
        self.columns.hashes.push(hash);
        self.columns.versions.push(version);
        match timestamp {
            Some(timestamp) => {
                self.columns.timestamps.push(timestamp);
                self.columns.timestamped += 1;
            }
            // Keeps the column aligned with the others; `finish_dataset` rejects the mix anyway.
            None => self.columns.timestamps.push(0),
        }
        self.columns.worker_indexes.extend_from_slice(worker_indexes);
        let end = self
            .columns
            .worker_indexes
            .len()
            .try_into()
            .context("a dataset may hold at most u32::MAX worker references")?;
        self.columns.worker_ends.push(end);

        self.last_block = Some(*block_range.end());
        self.current_dataset_id_offset = Some(dataset);
        continuity
    }
}

/// The pieces a chunk id is made of. The reader puts them back together the same way, so this is
/// the only place the format is written down for the portal assignment.
struct ParsedChunkId {
    top: u64,
    first_block: u64,
    last_block: u64,
    hash: assignment_fb::ChunkHash,
}

/// Splits `"0221000000/0221000000-0221000649-9QgFD"` into the parts the columns hold.
fn parse_chunk_id(id: &str) -> anyhow::Result<ParsedChunkId> {
    let (top, rest) = id
        .split_once('/')
        .with_context(|| format!("chunk id '{id}' has no top directory"))?;
    let mut parts = rest.splitn(3, '-');
    let (Some(first_block), Some(last_block), Some(hash)) =
        (parts.next(), parts.next(), parts.next())
    else {
        anyhow::bail!("chunk id '{id}' is not <top>/<first_block>-<last_block>-<hash>");
    };
    // A hash is `\w{5,8}` to every writer and to the worker's parser, so it never contains the
    // separator and always fits the fixed-width column.
    anyhow::ensure!(
        (1..=8).contains(&hash.len())
            && hash.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "chunk id '{id}' has a hash that is not 1 to 8 word characters"
    );
    let mut bytes = [0u8; 8];
    bytes[..hash.len()].copy_from_slice(hash.as_bytes());
    Ok(ParsedChunkId {
        top: top.parse().with_context(|| format!("chunk id '{id}' has a non-numeric top"))?,
        first_block: first_block
            .parse()
            .with_context(|| format!("chunk id '{id}' has a non-numeric first block"))?,
        last_block: last_block
            .parse()
            .with_context(|| format!("chunk id '{id}' has a non-numeric last block"))?,
        hash: assignment_fb::ChunkHash::new(&bytes),
    })
}

pub struct PortalAssignmentChunkBuilder<'b> {
    p: &'b mut PortalAssignmentBuilder,

    block_range: Option<RangeInclusive<u64>>,
    id: Option<String>,
    dataset_id: Option<fb::WIPOffset<&'static str>>,
    version: u32,
    last_block_timestamp: Option<u64>,
    worker_indexes: Vec<u16>,
}

impl<'b> PortalAssignmentChunkBuilder<'b> {
    pub fn new(parent: &'b mut PortalAssignmentBuilder) -> Self {
        Self {
            p: parent,
            block_range: None,
            id: None,
            dataset_id: None,
            version: 0,
            last_block_timestamp: None,
            worker_indexes: Vec::new(),
        }
    }

    /// The chunk id, e.g. `"0221000000/0221000000-0221000649-9QgFD"`. Not stored as such: it is
    /// split into the `tops`, `first_blocks`, `block_deltas` and `hashes` columns and reassembled
    /// on read, so it must parse — see [`Self::finish`].
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn id(mut self, id: &str) -> Self {
        self.id = Some(id.to_owned());
        self
    }

    /// Which dataset the chunk belongs to. Names the dataset it is staged into; chunks don't
    /// carry the id themselves, since they are only ever read through that dataset.
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn dataset_id(mut self, dataset_id: &str) -> Self {
        self.dataset_id = Some(self.p.builder.create_shared_string(dataset_id));
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn block_range(mut self, range: RangeInclusive<u64>) -> Self {
        self.block_range = Some(range);
        self
    }

    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn last_block_timestamp(mut self, timestamp: u64) -> Self {
        self.last_block_timestamp = Some(timestamp);
        self
    }

    /// Which copy of the chunk workers serve. Defaults to 0, the ingested one; must match the
    /// version the same chunk carries in the worker assignment (see
    /// [`WorkerAssignmentChunkBuilder::version`]).
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn version(mut self, version: u32) -> Self {
        self.version = version;
        self
    }

    /// Confirmed routing (which workers portals should route to), not the raw ideal placement.
    #[must_use = "a chunk is staged by `finish`; a builder that is dropped stages nothing"]
    pub fn worker_indexes(mut self, indexes: &[u16]) -> Self {
        self.worker_indexes = indexes.to_vec();
        self
    }

    /// # Panics
    ///
    /// If `block_range` was never set — API misuse, not bad input.
    ///
    /// # Errors
    ///
    /// If the id is unset or malformed, if it disagrees with `block_range` — the two encode the
    /// same block numbers, and the id is rebuilt from the range on read — or if the chunk breaks
    /// block continuity (see [`PortalAssignmentBuilder::check_continuity`]).
    pub fn finish(self) -> anyhow::Result<()> {
        let block_range = self.block_range.expect("Block range must be set");
        let id = self.id.context("Chunk id must be set")?;
        let parsed = parse_chunk_id(&id)?;
        anyhow::ensure!(
            parsed.first_block == *block_range.start() && parsed.last_block == *block_range.end(),
            "chunk id '{id}' names blocks {}-{}, but the chunk covers {}-{}",
            parsed.first_block,
            parsed.last_block,
            block_range.start(),
            block_range.end()
        );
        self.p.push_chunk(
            self.dataset_id.expect("Dataset ID must be set"),
            parsed.top,
            block_range,
            parsed.hash,
            self.version,
            self.last_block_timestamp,
            &self.worker_indexes,
        )
    }
}
