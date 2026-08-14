use std::{cmp::Ordering, collections::BTreeMap};

use anyhow::anyhow;
use crypto_box::{aead::Aead, PublicKey, SalsaBox, SecretKey};
use flatbuffers::{Follow, ForwardsUOffset, Vector};
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
        let opts = flatbuffers::VerifierOptions {
            max_tables: 1_000_000_000_000,
            max_apparent_size: 1 << 40, // 1TB
            ..Default::default()
        };
        AssignmentTryBuilder {
            buf,
            reader_builder: |buf| assignment_fb::root_as_assignment_with_opts(&opts, buf),
        }
        .try_build()
    }

    pub fn from_owned_unchecked(buf: Vec<u8>) -> Self {
        AssignmentBuilder {
            buf,
            reader_builder: |buf| unsafe { assignment_fb::root_as_assignment_unchecked(buf) },
        }
        .build()
    }

    pub fn get_worker_id(&self, index: u16) -> Result<PeerId, anyhow::Error> {
        let workers = self.borrow_reader().workers();
        let worker = workers.get(index as usize);
        Ok((*worker.worker_id()).try_into()?)
    }

    pub fn get_worker_by_index(&self, index: u16) -> Worker<'_> {
        let workers = self.borrow_reader().workers();
        let worker = workers.get(index as usize);
        Worker {
            assignment: *self.borrow_reader(),
            reader: worker,
            index,
        }
    }

    pub fn get_worker(&self, id: &PeerId) -> Option<Worker<'_>> {
        let workers = self.borrow_reader().workers();
        let index = lookup_index_by_key(&workers, |x| {
            let parsed: PeerId = (*x.worker_id()).try_into().unwrap_or_else(|e| {
                panic!("Couldn't parse peer id '{:?}': {}", x.worker_id().peer_id(), e);
            });
            parsed.cmp(id)
        })?;
        Some(Worker {
            assignment: *self.borrow_reader(),
            reader: workers.get(index),
            index: index as u16,
        })
    }

    pub fn workers(
        &self,
    ) -> flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<assignment_fb::WorkerEntry<'_>>> {
        self.borrow_reader().workers()
    }

    pub fn datasets(
        &self,
    ) -> flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<assignment_fb::Dataset<'_>>> {
        self.borrow_reader().datasets()
    }

    pub fn get_dataset(&self, dataset: &str) -> Option<assignment_fb::Dataset<'_>> {
        self.borrow_reader()
            .datasets()
            .lookup_by_key(dataset, |ds, key| ds.key_compare_with_value(key))
    }

    pub fn get_chunk(&self, r: ChunkRef) -> Option<assignment_fb::Chunk<'_>> {
        let datasets = self.borrow_reader().datasets();
        if (r.dataset_index as usize) >= datasets.len() {
            return None;
        }
        let chunks = datasets.get(r.dataset_index as usize).chunks();
        if (r.chunk_index as usize) >= chunks.len() {
            return None;
        }
        Some(chunks.get(r.chunk_index as usize))
    }

    pub fn find_chunk(
        &self,
        dataset: &str,
        block: u64,
    ) -> Result<assignment_fb::Chunk<'_>, ChunkNotFound> {
        let Some(dataset) = self.get_dataset(dataset) else {
            return Err(ChunkNotFound::UnknownDataset);
        };

        if block > dataset.last_block() {
            return Err(ChunkNotFound::AfterLast);
        }

        let chunks = dataset.chunks();

        // find last chunk with first_block <= block
        binary_search_by(Chunks(&chunks), |itm| itm.first_block().cmp(&block))
            .or_else(|e| match e {
                Some(idx) => Ok(idx),
                None => Err(ChunkNotFound::BeforeFirst),
            })
            .map(|idx| chunks.get(idx))
    }

    pub fn find_chunk_by_timestamp(
        &self,
        dataset: &str,
        ts: u64,
    ) -> Result<assignment_fb::Chunk<'_>, ChunkNotFound> {
        let Some(dataset) = self.get_dataset(dataset) else {
            return Err(ChunkNotFound::UnknownDataset);
        };

        let chunks = dataset.chunks();

        // find first chunk with last_block_timestamp >= ts
        binary_search_by(Chunks(&chunks), |itm| {
            itm.last_block_timestamp().unwrap_or(0).cmp(&ts) // 0-timestamps are problematic
        })
        .or_else(|e| match e {
            Some(idx) if idx + 1 < chunks.len() => Ok(idx + 1),
            None if !chunks.is_empty() => Ok(0), // this is the case BeforeFirst
            _ => Err(ChunkNotFound::AfterLast),
        })
        .map(|idx| {
            // for the case that the timestamps are equal,
            // we walk to the first of the sequence; this is clumsy but safe.
            if chunks.get(idx).last_block_timestamp() == Some(ts) {
                for i in (0..idx + 1).rev() {
                    if chunks.get(i).last_block_timestamp() != Some(ts) {
                        return chunks.get(i + 1);
                    } else if i == 0 {
                        return chunks.get(0);
                    }
                }
            }
            chunks.get(idx)
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkNotFound {
    UnknownDataset,
    BeforeFirst,
    AfterLast,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkRef {
    dataset_index: u32,
    chunk_index: u32,
}

pub struct Worker<'f> {
    assignment: assignment_fb::Assignment<'f>,
    reader: assignment_fb::WorkerEntry<'f>,
    index: u16,
}

impl Worker<'_> {
    pub fn iter_chunks(&self) -> impl Iterator<Item = assignment_fb::Chunk<'_>> + '_ {
        self.assignment.datasets().iter().flat_map(move |dataset| {
            dataset
                .chunks()
                .iter()
                .filter(move |chunk| chunk.worker_indexes().iter().any(|i| self.index == i))
        })
    }

    pub fn iter_chunks_with_ref(
        &self,
    ) -> impl Iterator<Item = (ChunkRef, assignment_fb::Chunk<'_>)> + '_ {
        self.assignment.datasets().iter().enumerate().flat_map(move |(d, dataset)| {
            dataset
                .chunks()
                .iter()
                .enumerate()
                .filter(move |(_, chunk)| chunk.worker_indexes().iter().any(|i| self.index == i))
                .map(move |(c, chunk)| {
                    (
                        ChunkRef {
                            dataset_index: d as u32,
                            chunk_index: c as u32,
                        },
                        chunk,
                    )
                })
        })
    }

    pub fn peer_id(&self) -> Result<PeerId, anyhow::Error> {
        Ok((*self.reader.worker_id()).try_into()?)
    }

    pub fn status(&self) -> WorkerStatus {
        status_from_fb(self.reader.status())
    }

    pub fn decrypt_headers(&self, key: &Keypair) -> anyhow::Result<BTreeMap<String, String>> {
        decrypt_headers(self.reader.encrypted_headers(), key)
    }
}

// ===== Worker-facing assignment (NET-1180) =====
//
// Diverges from `Assignment` above: no `find_chunk`/`find_chunk_by_timestamp` (query routing is a
// portal concern), no `last_block_hash`/`last_block_timestamp` on chunks (portal-only). Otherwise
// mirrors `Assignment`/`Worker` — see docs/assignment-wire-format.md in network-scheduler.

#[ouroboros::self_referencing]
pub struct WorkerAssignment {
    buf: Vec<u8>,

    #[borrows(buf)]
    #[covariant]
    reader: assignment_fb::WorkerAssignment<'this>,
}

impl WorkerAssignment {
    pub fn from_owned(buf: Vec<u8>) -> Result<Self, flatbuffers::InvalidFlatbuffer> {
        let opts = flatbuffers::VerifierOptions {
            max_tables: 1_000_000_000_000,
            max_apparent_size: 1 << 40, // 1TB
            ..Default::default()
        };
        WorkerAssignmentTryBuilder {
            buf,
            reader_builder: |buf| {
                flatbuffers::root_with_opts::<assignment_fb::WorkerAssignment>(&opts, buf)
            },
        }
        .try_build()
    }

    pub fn from_owned_unchecked(buf: Vec<u8>) -> Self {
        WorkerAssignmentBuilder {
            buf,
            reader_builder: |buf| unsafe {
                flatbuffers::root_unchecked::<assignment_fb::WorkerAssignment>(buf)
            },
        }
        .build()
    }

    pub fn get_worker_by_index(&self, index: u16) -> AssignedWorker<'_> {
        let workers = self.borrow_reader().workers();
        let worker = workers.get(index as usize);
        AssignedWorker {
            assignment: *self.borrow_reader(),
            reader: worker,
            index,
        }
    }

    pub fn get_worker(&self, id: &PeerId) -> Option<AssignedWorker<'_>> {
        let workers = self.borrow_reader().workers();
        let index = lookup_index_by_key(&workers, |x| {
            let parsed: PeerId = (*x.worker_id()).try_into().unwrap_or_else(|e| {
                panic!("Couldn't parse peer id '{:?}': {}", x.worker_id().peer_id(), e);
            });
            parsed.cmp(id)
        })?;
        Some(AssignedWorker {
            assignment: *self.borrow_reader(),
            reader: workers.get(index),
            index: index as u16,
        })
    }

    pub fn workers(
        &self,
    ) -> flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<assignment_fb::WorkerEntry<'_>>> {
        self.borrow_reader().workers()
    }

    pub fn datasets(
        &self,
    ) -> flatbuffers::Vector<
        '_,
        flatbuffers::ForwardsUOffset<assignment_fb::WorkerAssignmentDataset<'_>>,
    > {
        self.borrow_reader().datasets()
    }

    pub fn get_dataset(&self, dataset: &str) -> Option<assignment_fb::WorkerAssignmentDataset<'_>> {
        self.borrow_reader()
            .datasets()
            .lookup_by_key(dataset, |ds, key| ds.key_compare_with_value(key))
    }

    pub fn get_chunk(&self, r: ChunkRef) -> Option<assignment_fb::WorkerAssignmentChunk<'_>> {
        let datasets = self.borrow_reader().datasets();
        if (r.dataset_index as usize) >= datasets.len() {
            return None;
        }
        let chunks = datasets.get(r.dataset_index as usize).chunks();
        if (r.chunk_index as usize) >= chunks.len() {
            return None;
        }
        Some(chunks.get(r.chunk_index as usize))
    }

    /// Where a chunk's files live: its `dataset_base_url`, then the prefix of the generation its
    /// `version` names — nothing for version 0, the ingested layout — then the chunk id.
    ///
    /// `None` if a non-zero version names a generation the chunk's dataset doesn't carry, or if
    /// that dataset isn't in this assignment at all.
    pub fn chunk_url(&self, chunk: assignment_fb::WorkerAssignmentChunk<'_>) -> Option<String> {
        let mut url = chunk.dataset_base_url().to_owned();
        if chunk.version() != 0 {
            let dataset = self.get_dataset(chunk.dataset_id())?;
            push_segment(&mut url, dataset.get_generation(chunk.version())?.base_url());
        }
        push_segment(&mut url, chunk.id());
        Some(url)
    }

    /// The table roster of a write schema referenced by this assignment's chunks.
    pub fn get_write_schema(&self, write_schema_id: u32) -> Option<assignment_fb::TableRoster<'_>> {
        self.borrow_reader()
            .schemas()
            .lookup_by_key(write_schema_id, |roster, key| roster.key_compare_with_value(*key))
    }

    /// The tables a chunk contains: the whole roster when it sets no bitmap, otherwise the tables
    /// its bits select. `None` if the chunk's `write_schema_id` has no roster here.
    ///
    /// Bits beyond the roster are ignored, so a malformed buffer can't name a table outside it.
    pub fn chunk_tables<'a>(
        &'a self,
        chunk: assignment_fb::WorkerAssignmentChunk<'a>,
    ) -> Option<impl Iterator<Item = &'a str> + 'a> {
        let roster = self.get_write_schema(chunk.write_schema_id())?;
        let bits = chunk.tables_present().map(|bits| bits.bytes());
        Some(roster.tables().iter().enumerate().filter_map(move |(index, table)| {
            let present = bits.is_none_or(|bits| {
                bits.get(index / 8).is_some_and(|byte| byte & (1u8 << (index % 8)) != 0)
            });
            present.then_some(table)
        }))
    }
}

/// A worker's entry in a [`WorkerAssignment`]: identity, status, sealed auth headers, and the
/// chunks it's assigned (via each chunk's `worker_indexes`).
pub struct AssignedWorker<'f> {
    assignment: assignment_fb::WorkerAssignment<'f>,
    reader: assignment_fb::WorkerEntry<'f>,
    index: u16,
}

impl AssignedWorker<'_> {
    pub fn iter_chunks(
        &self,
    ) -> impl Iterator<Item = assignment_fb::WorkerAssignmentChunk<'_>> + '_ {
        self.assignment.datasets().iter().flat_map(move |dataset| {
            dataset
                .chunks()
                .iter()
                .filter(move |chunk| chunk.worker_indexes().iter().any(|i| self.index == i))
        })
    }

    pub fn iter_chunks_with_ref(
        &self,
    ) -> impl Iterator<Item = (ChunkRef, assignment_fb::WorkerAssignmentChunk<'_>)> + '_ {
        self.assignment.datasets().iter().enumerate().flat_map(move |(d, dataset)| {
            dataset
                .chunks()
                .iter()
                .enumerate()
                .filter(move |(_, chunk)| chunk.worker_indexes().iter().any(|i| self.index == i))
                .map(move |(c, chunk)| {
                    (
                        ChunkRef {
                            dataset_index: d as u32,
                            chunk_index: c as u32,
                        },
                        chunk,
                    )
                })
        })
    }

    pub fn peer_id(&self) -> Result<PeerId, anyhow::Error> {
        Ok((*self.reader.worker_id()).try_into()?)
    }

    pub fn status(&self) -> WorkerStatus {
        status_from_fb(self.reader.status())
    }

    pub fn decrypt_headers(&self, key: &Keypair) -> anyhow::Result<BTreeMap<String, String>> {
        decrypt_headers(self.reader.encrypted_headers(), key)
    }
}

// ===== Portal-facing assignment (NET-1180) =====
//
// Diverges from `Assignment`: no file/URL/`encrypted_headers` access at all (worker-only
// concerns), chunks carry `last_block_hash`/`last_block_timestamp` instead, and each dataset
// carries a `read_schema_id` — an unresolved reference, unlike the worker side's inline rosters.

#[ouroboros::self_referencing]
pub struct PortalAssignment {
    buf: Vec<u8>,

    #[borrows(buf)]
    #[covariant]
    reader: assignment_fb::PortalAssignment<'this>,
}

impl PortalAssignment {
    pub fn from_owned(buf: Vec<u8>) -> Result<Self, flatbuffers::InvalidFlatbuffer> {
        let opts = flatbuffers::VerifierOptions {
            max_tables: 1_000_000_000_000,
            max_apparent_size: 1 << 40, // 1TB
            ..Default::default()
        };
        PortalAssignmentTryBuilder {
            buf,
            reader_builder: |buf| {
                flatbuffers::root_with_opts::<assignment_fb::PortalAssignment>(&opts, buf)
            },
        }
        .try_build()
    }

    pub fn from_owned_unchecked(buf: Vec<u8>) -> Self {
        PortalAssignmentBuilder {
            buf,
            reader_builder: |buf| unsafe {
                flatbuffers::root_unchecked::<assignment_fb::PortalAssignment>(buf)
            },
        }
        .build()
    }

    pub fn get_worker_id(&self, index: u16) -> Result<PeerId, anyhow::Error> {
        let workers = self.borrow_reader().workers();
        let worker = workers.get(index as usize);
        Ok((*worker.worker_id()).try_into()?)
    }

    pub fn get_worker_by_index(&self, index: u16) -> PortalWorker<'_> {
        PortalWorker {
            reader: self.borrow_reader().workers().get(index as usize),
        }
    }

    pub fn workers(
        &self,
    ) -> flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<assignment_fb::PortalEntry<'_>>> {
        self.borrow_reader().workers()
    }

    pub fn datasets(
        &self,
    ) -> flatbuffers::Vector<
        '_,
        flatbuffers::ForwardsUOffset<assignment_fb::PortalAssignmentDataset<'_>>,
    > {
        self.borrow_reader().datasets()
    }

    pub fn get_dataset(&self, dataset: &str) -> Option<assignment_fb::PortalAssignmentDataset<'_>> {
        self.borrow_reader()
            .datasets()
            .lookup_by_key(dataset, |ds, key| ds.key_compare_with_value(key))
    }

    pub fn get_chunk(&self, r: ChunkRef) -> Option<assignment_fb::PortalAssignmentChunk<'_>> {
        let datasets = self.borrow_reader().datasets();
        if (r.dataset_index as usize) >= datasets.len() {
            return None;
        }
        let chunks = datasets.get(r.dataset_index as usize).chunks();
        if (r.chunk_index as usize) >= chunks.len() {
            return None;
        }
        Some(chunks.get(r.chunk_index as usize))
    }

    pub fn find_chunk(
        &self,
        dataset: &str,
        block: u64,
    ) -> Result<assignment_fb::PortalAssignmentChunk<'_>, ChunkNotFound> {
        let Some(dataset) = self.get_dataset(dataset) else {
            return Err(ChunkNotFound::UnknownDataset);
        };

        if block > dataset.last_block() {
            return Err(ChunkNotFound::AfterLast);
        }

        let chunks = dataset.chunks();

        // find last chunk with first_block <= block
        binary_search_by(PortalChunks(&chunks), |itm| itm.first_block().cmp(&block))
            .or_else(|e| match e {
                Some(idx) => Ok(idx),
                None => Err(ChunkNotFound::BeforeFirst),
            })
            .map(|idx| chunks.get(idx))
    }

    pub fn find_chunk_by_timestamp(
        &self,
        dataset: &str,
        ts: u64,
    ) -> Result<assignment_fb::PortalAssignmentChunk<'_>, ChunkNotFound> {
        let Some(dataset) = self.get_dataset(dataset) else {
            return Err(ChunkNotFound::UnknownDataset);
        };

        let chunks = dataset.chunks();

        // find first chunk with last_block_timestamp >= ts
        binary_search_by(PortalChunks(&chunks), |itm| {
            itm.last_block_timestamp().unwrap_or(0).cmp(&ts) // 0-timestamps are problematic
        })
        .or_else(|e| match e {
            Some(idx) if idx + 1 < chunks.len() => Ok(idx + 1),
            None if !chunks.is_empty() => Ok(0), // this is the case BeforeFirst
            _ => Err(ChunkNotFound::AfterLast),
        })
        .map(|idx| {
            // for the case that the timestamps are equal,
            // we walk to the first of the sequence; this is clumsy but safe.
            if chunks.get(idx).last_block_timestamp() == Some(ts) {
                for i in (0..idx + 1).rev() {
                    if chunks.get(i).last_block_timestamp() != Some(ts) {
                        return chunks.get(i + 1);
                    } else if i == 0 {
                        return chunks.get(0);
                    }
                }
            }
            chunks.get(idx)
        })
    }
}

/// A worker's entry in a [`PortalAssignment`]: just identity and routing eligibility — no
/// `encrypted_headers`/chunk iteration; a portal never downloads and never needs a specific
/// worker's whole chunk list.
pub struct PortalWorker<'f> {
    reader: assignment_fb::PortalEntry<'f>,
}

impl PortalWorker<'_> {
    pub fn peer_id(&self) -> Result<PeerId, anyhow::Error> {
        Ok((*self.reader.worker_id()).try_into()?)
    }

    pub fn status(&self) -> WorkerStatus {
        status_from_fb(self.reader.status())
    }
}

/// Appends a path segment with exactly one separator, whichever side already carries it.
fn push_segment(url: &mut String, segment: &str) {
    if !url.ends_with('/') {
        url.push('/');
    }
    url.push_str(segment.trim_start_matches('/'));
}

fn status_from_fb(status: assignment_fb::WorkerStatus) -> WorkerStatus {
    match status {
        assignment_fb::WorkerStatus::Ok => WorkerStatus::Ok,
        assignment_fb::WorkerStatus::Unreliable => WorkerStatus::Unreliable,
        assignment_fb::WorkerStatus::DeprecatedVersion => WorkerStatus::DeprecatedVersion,
        assignment_fb::WorkerStatus::UnsupportedVersion => WorkerStatus::UnsupportedVersion,
        _ => WorkerStatus::UnsupportedVersion,
    }
}

fn decrypt_headers(
    headers: Option<assignment_fb::EncryptedHeaders<'_>>,
    key: &Keypair,
) -> anyhow::Result<BTreeMap<String, String>> {
    let secret_key = key.clone().try_into_ed25519()?.secret();
    let headers = headers.ok_or(anyhow!("EncryptedHeaders field missing"))?;
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

#[derive(Copy, Clone)]
struct PortalChunks<'a>(&'a Vector<'a, ForwardsUOffset<assignment_fb::PortalAssignmentChunk<'a>>>);

impl<'a> IndexGet for PortalChunks<'a> {
    type Item = assignment_fb::PortalAssignmentChunk<'a>;

    fn len(&self) -> usize {
        self.0.len()
    }

    fn get(&self, idx: usize) -> Self::Item {
        self.0.get(idx)
    }
}

fn lookup_index_by_key<'a, T: Follow<'a> + 'a>(
    v: &flatbuffers::Vector<'a, T>,
    f: impl Fn(&<T as Follow<'a>>::Inner) -> Ordering,
) -> Option<usize> {
    if v.is_empty() {
        return None;
    }

    let mut left: usize = 0;
    let mut right = v.len() - 1;

    while left <= right {
        let mid = (left + right) / 2;
        let value = v.get(mid);
        match f(&value) {
            Ordering::Equal => return Some(mid),
            Ordering::Less => left = mid + 1,
            Ordering::Greater => {
                if mid == 0 {
                    return None;
                }
                right = mid - 1;
            }
        }
    }

    None
}

trait IndexGet {
    type Item;

    fn len(&self) -> usize;
    fn get(&self, idx: usize) -> Self::Item;
}

#[derive(Copy, Clone)]
struct Chunks<'a>(&'a Vector<'a, ForwardsUOffset<assignment_fb::Chunk<'a>>>);

impl<'a> IndexGet for Chunks<'a> {
    type Item = assignment_fb::Chunk<'a>;

    fn len(&self) -> usize {
        self.0.len()
    }

    fn get(&self, idx: usize) -> Self::Item {
        self.0.get(idx)
    }
}

/// Finds the greatest item for which cmp is less or equal. Result:
/// Ok(i): cmp returned equal for the item at index i
/// Err(Some(i)): No equal item was found and
///               i is the index of the greatest item for which cmp returned less
/// Err(None): No item was found for which cmp returns less or equal
fn binary_search_by<V, F>(v: V, mut cmp: F) -> Result<usize, Option<usize>>
where
    V: IndexGet,
    F: FnMut(&V::Item) -> Ordering,
{
    let mut left = -1;
    let mut right = v.len() as isize;

    while left + 1 < right {
        let mid = (left + right) / 2;
        let item = v.get(mid as usize);

        match cmp(&item) {
            Ordering::Less => {
                left = mid;
            }
            Ordering::Greater => {
                right = mid;
            }
            Ordering::Equal => return Ok(mid as usize),
        }
    }

    Err(if left == -1 {
        None
    } else {
        Some(left as usize)
    })
}

#[cfg(test)]
mod test {
    use super::*;

    struct TestSlice<'a>(&'a [TestItem]);

    #[derive(Clone, Copy, Debug)]
    struct TestItem(u64);

    impl<'a> IndexGet for TestSlice<'a> {
        type Item = TestItem;

        fn len(&self) -> usize {
            self.0.len()
        }

        fn get(&self, idx: usize) -> Self::Item {
            *self.0.get(idx).unwrap()
        }
    }

    fn make_test_vec() -> Vec<TestItem> {
        vec![
            TestItem(11),
            TestItem(13),
            TestItem(15),
            TestItem(15),
            TestItem(15),
            TestItem(19),
            TestItem(21),
        ]
    }

    fn binary_search_g_le(k: u64, v: &[TestItem]) -> Result<usize, Option<usize>> {
        binary_search_by(TestSlice(v), |itm| itm.0.cmp(&k))
    }

    fn binary_search_l_ge(k: u64, v: &[TestItem]) -> Result<usize, Option<usize>> {
        match binary_search_by(TestSlice(v), |itm| itm.0.cmp(&k)) {
            Ok(idx) => {
                for i in (0..idx + 1).rev() {
                    if v[i].0 != k {
                        return Ok(i + 1);
                    } else if i == 0 {
                        return Ok(0);
                    }
                }
                Ok(idx)
            }
            Err(None) if !v.is_empty() => Err(Some(0)),
            Err(Some(idx)) if idx + 1 < v.len() => Err(Some(idx + 1)),
            _ => Err(None),
        }
    }

    #[test]
    fn test_find_greatest_le() {
        let v = make_test_vec();

        assert_eq!(binary_search_g_le(11, &v), Ok(0));
        assert_eq!(binary_search_g_le(13, &v), Ok(1));
        assert_eq!(binary_search_g_le(15, &v), Ok(3));
        assert_eq!(binary_search_g_le(19, &v), Ok(5));
        assert_eq!(binary_search_g_le(21, &v), Ok(6));

        assert_eq!(binary_search_g_le(10, &v), Err(None));

        assert_eq!(binary_search_g_le(12, &v), Err(Some(0)));
        assert_eq!(binary_search_g_le(14, &v), Err(Some(1)));
        assert_eq!(binary_search_g_le(16, &v), Err(Some(4)));
        assert_eq!(binary_search_g_le(17, &v), Err(Some(4)));
        assert_eq!(binary_search_g_le(18, &v), Err(Some(4)));
        assert_eq!(binary_search_g_le(20, &v), Err(Some(5)));
        assert_eq!(binary_search_g_le(22, &v), Err(Some(6)));
    }

    #[test]
    fn test_find_least_ge() {
        let v = make_test_vec();

        assert_eq!(binary_search_l_ge(11, &v), Ok(0));
        assert_eq!(binary_search_l_ge(13, &v), Ok(1));
        assert_eq!(binary_search_l_ge(15, &v), Ok(2));
        assert_eq!(binary_search_l_ge(19, &v), Ok(5));
        assert_eq!(binary_search_l_ge(21, &v), Ok(6));

        assert_eq!(binary_search_l_ge(0, &v), Err(Some(0)));
        assert_eq!(binary_search_l_ge(10, &v), Err(Some(0)));
        assert_eq!(binary_search_l_ge(12, &v), Err(Some(1)));
        assert_eq!(binary_search_l_ge(14, &v), Err(Some(2)));
        assert_eq!(binary_search_l_ge(17, &v), Err(Some(5)));
        assert_eq!(binary_search_l_ge(16, &v), Err(Some(5)));
        assert_eq!(binary_search_l_ge(18, &v), Err(Some(5)));
        assert_eq!(binary_search_l_ge(20, &v), Err(Some(6)));

        assert_eq!(binary_search_l_ge(22, &v), Err(None));
    }

    #[test]
    fn test_find_least_ge_edge_cases() {
        let v = vec![TestItem(15), TestItem(15), TestItem(15), TestItem(19), TestItem(21)];

        assert_eq!(binary_search_l_ge(15, &v), Ok(0));
        assert_eq!(binary_search_l_ge(19, &v), Ok(3));
        assert_eq!(binary_search_l_ge(17, &v), Err(Some(3)));
        assert_eq!(binary_search_l_ge(18, &v), Err(Some(3)));

        let v = vec![TestItem(14), TestItem(15), TestItem(15), TestItem(15)];

        assert_eq!(binary_search_l_ge(15, &v), Ok(1));
        assert_eq!(binary_search_l_ge(14, &v), Ok(0));
        assert_eq!(binary_search_l_ge(13, &v), Err(Some(0)));
        assert_eq!(binary_search_l_ge(16, &v), Err(None));

        let v = vec![TestItem(15), TestItem(15), TestItem(15)];

        assert_eq!(binary_search_l_ge(15, &v), Ok(0));

        let v = vec![TestItem(15)];
        assert_eq!(binary_search_l_ge(15, &v), Ok(0));
        assert_eq!(binary_search_l_ge(16, &v), Err(None));
        assert_eq!(binary_search_l_ge(14, &v), Err(Some(0)));
    }
}
