use std::{cmp::Ordering, collections::BTreeMap};

use anyhow::anyhow;
use crypto_box::{aead::Aead, PublicKey, SalsaBox, SecretKey};
use flatbuffers::{Follow, ForwardsUOffset, Vector};
use libp2p_identity::{Keypair, PeerId};
use sha2::{digest::generic_array::GenericArray, Digest, Sha512};

use crate::{assignment_fb, assignment_fb::push_segment, WorkerStatus};

/// Why an assignment couldn't be read.
#[derive(Debug, thiserror::Error)]
pub enum InvalidAssignment {
    #[error(transparent)]
    Flatbuffer(#[from] flatbuffers::InvalidFlatbuffer),
    /// The buffer is well formed, but a dataset's columns contradict each other — which the
    /// flatbuffers verifier cannot see, since it checks each vector on its own.
    #[error("dataset '{dataset}': {detail}")]
    Columns { dataset: String, detail: String },
}

/// Checks one dataset's columns against the chunk count `first_blocks` implies.
///
/// Only what a reader would otherwise panic on, and only at O(datasets) or O(runs) — an O(chunks)
/// check would undo a format that loads in microseconds. What is left unchecked (offsets that
/// don't ascend, say) yields wrong answers rather than crashes; the accessors clamp.
fn check_columns(
    dataset: &str,
    chunks: usize,
    dense: &[(&str, usize)],
    runs: &[(&str, Option<u32>)],
) -> Result<(), InvalidAssignment> {
    let fail = |detail: String| InvalidAssignment::Columns {
        dataset: dataset.to_owned(),
        detail,
    };
    for (name, len) in dense {
        if *len != chunks {
            return Err(fail(format!("{name} holds {len} of {chunks} chunks")));
        }
    }
    for (name, first) in runs {
        match first {
            None => return Err(fail(format!("{name} is empty, so chunk 0 falls in no run"))),
            Some(first) if *first != 0 => {
                return Err(fail(format!("{name} starts at chunk {first}, not 0")))
            }
            Some(_) => {}
        }
    }
    Ok(())
}

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
    /// The block falls between two chunks. Gaps are legal, so this is an answer, not a defect.
    InGap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkRef {
    dataset_index: u32,
    chunk_index: u32,
}

impl ChunkRef {
    /// With [`WorkerAssignment::get_dataset_by_ref`], how a chunk is traced back to its dataset.
    pub fn dataset_index(&self) -> u32 {
        self.dataset_index
    }

    pub fn chunk_index(&self) -> u32 {
        self.chunk_index
    }
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

// ===== Worker-facing assignment =====
//
// Chunks are columns on the dataset (see worker_assignment.fbs); [`WorkerChunk`] is a cursor over
// one row. Query routing is a portal concern, so nothing here searches by block or timestamp.

#[ouroboros::self_referencing]
pub struct WorkerAssignment {
    buf: Vec<u8>,

    #[borrows(buf)]
    #[covariant]
    reader: assignment_fb::WorkerAssignment<'this>,
}

impl WorkerAssignment {
    /// Verifies the buffer, then checks that each dataset's columns agree in length — which every
    /// chunk accessor assumes and the flatbuffers verifier cannot establish.
    ///
    /// # Errors
    ///
    /// If the buffer is malformed, or a dataset's columns disagree.
    pub fn from_owned(buf: Vec<u8>) -> Result<Self, InvalidAssignment> {
        let opts = flatbuffers::VerifierOptions {
            max_tables: 1_000_000_000_000,
            max_apparent_size: 1 << 40, // 1TB
            ..Default::default()
        };
        let assignment = WorkerAssignmentTryBuilder {
            buf,
            reader_builder: |buf| {
                flatbuffers::root_with_opts::<assignment_fb::WorkerAssignment>(&opts, buf)
            },
        }
        .try_build()?;
        for dataset in assignment.datasets().iter() {
            let chunks = dataset.first_blocks().len();
            check_columns(
                dataset.id(),
                chunks,
                &[
                    ("block_deltas", dataset.block_deltas().len()),
                    ("hashes", dataset.hashes().len()),
                    ("sizes", dataset.sizes().len()),
                    ("write_schema_ids", dataset.write_schema_ids().len()),
                    ("worker_offsets", dataset.worker_offsets().len().saturating_sub(1)),
                ]
                .into_iter()
                .chain(dataset.versions().map(|c| ("versions", c.len())))
                .chain(
                    dataset
                        .tables_present_offsets()
                        .map(|c| ("tables_present_offsets", c.len().saturating_sub(1))),
                )
                .collect::<Vec<_>>(),
                &[(
                    "tops",
                    (!dataset.tops().is_empty()).then(|| dataset.tops().get(0).first_chunk_index()),
                )],
            )?;
        }
        Ok(assignment)
    }

    /// # Panics
    ///
    /// A chunk accessor on a malformed buffer may panic. Use [`Self::from_owned`] for anything
    /// that didn't come from this process.
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

    pub fn get_chunk(&self, r: ChunkRef) -> Option<WorkerChunk<'_>> {
        self.get_dataset_by_ref(r)?.chunk(r.chunk_index)
    }

    /// How a caller recovers a chunk's dataset, which the chunk itself doesn't carry.
    pub fn get_dataset_by_ref(
        &self,
        r: ChunkRef,
    ) -> Option<assignment_fb::WorkerAssignmentDataset<'_>> {
        let datasets = self.borrow_reader().datasets();
        ((r.dataset_index as usize) < datasets.len())
            .then(|| datasets.get(r.dataset_index as usize))
    }

    /// [`WorkerChunk::url`], with the dataset resolved from the ref.
    pub fn chunk_url(&self, r: ChunkRef) -> Option<String> {
        self.get_chunk(r)?.url()
    }

    pub fn get_write_schema(&self, write_schema_id: u32) -> Option<assignment_fb::TableRoster<'_>> {
        self.borrow_reader()
            .schemas()
            .lookup_by_key(write_schema_id, |roster, key| roster.key_compare_with_value(*key))
    }

    /// The whole roster when the chunk sets no bitmap, otherwise the tables its bits select.
    /// `None` if its write schema has no roster here. Bits past the roster are ignored, so a
    /// malformed buffer can't name a table outside it.
    pub fn chunk_tables<'a>(
        &'a self,
        chunk: WorkerChunk<'a>,
    ) -> Option<impl Iterator<Item = &'a str> + 'a> {
        let roster = self.get_write_schema(chunk.write_schema_id())?;
        let bits = chunk.tables_present();
        Some(roster.tables().iter().enumerate().filter_map(move |(index, table)| {
            let present = bits.is_none_or(|bits| {
                bits.get(index / 8).is_some_and(|byte| byte & (1u8 << (index % 8)) != 0)
            });
            present.then_some(table)
        }))
    }
}

/// One row of a [`WorkerAssignmentDataset`](assignment_fb::WorkerAssignmentDataset)'s columns: a
/// dataset handle plus an index, so copying it is free.
#[derive(Clone, Copy, Debug)]
pub struct WorkerChunk<'a> {
    dataset: assignment_fb::WorkerAssignmentDataset<'a>,
    index: u32,
}

impl<'a> WorkerChunk<'a> {
    pub fn index(&self) -> u32 {
        self.index
    }

    pub fn dataset(&self) -> assignment_fb::WorkerAssignmentDataset<'a> {
        self.dataset
    }

    pub fn first_block(&self) -> u64 {
        self.dataset.first_blocks().get(self.index as usize)
    }

    pub fn last_block(&self) -> u64 {
        self.first_block() + self.dataset.block_deltas().get(self.index as usize) as u64
    }

    pub fn size(&self) -> u32 {
        self.dataset.sizes().get(self.index as usize)
    }

    /// Which copy of the chunk to download; 0 is the ingested one.
    pub fn version(&self) -> u32 {
        self.dataset.versions().map_or(0, |column| column.get(self.index as usize))
    }

    pub fn write_schema_id(&self) -> u32 {
        self.dataset.write_schema_ids().get(self.index as usize)
    }

    /// The top directory, resolved through the run this chunk falls in.
    ///
    /// # Panics
    ///
    /// If `tops` is empty or doesn't start at chunk 0 — which [`WorkerAssignment::from_owned`]
    /// rejects and `from_owned_unchecked` does not.
    pub fn top(&self) -> u64 {
        self.dataset
            .top_at(self.index)
            .expect("the first top run covers chunk 0 onwards")
    }

    /// The short hash, NUL padding trimmed. `None` only if the bytes aren't UTF-8.
    pub fn hash(&self) -> Option<&'a str> {
        let hash = self.dataset.hashes().get(self.index as usize);
        let bytes = &hash.0[..hash.0.iter().position(|&b| b == 0).unwrap_or(hash.0.len())];
        std::str::from_utf8(bytes).ok()
    }

    /// The chunk id, e.g. `"0221000000/0221000000-0221000649-9QgFD"`, rebuilt from its columns.
    /// `None` on the same terms as [`Self::hash`].
    pub fn id(&self) -> Option<String> {
        let mut id = String::with_capacity(ID_CAPACITY);
        push_chunk_id(&mut id, self.top(), self.first_block(), self.last_block(), self.hash()?);
        Some(id)
    }

    /// `None` when the chunk holds every table of its write schema — both an empty slice and a
    /// dataset carrying no bitmaps at all.
    pub fn tables_present(&self) -> Option<&'a [u8]> {
        let offsets = self.dataset.tables_present_offsets()?;
        let start = offsets.get(self.index as usize) as usize;
        let end = offsets.get(self.index as usize + 1) as usize;
        let bits = self.dataset.tables_present()?.bytes().get(start..end)?;
        (!bits.is_empty()).then_some(bits)
    }

    /// Where the chunk's files live: the dataset's `base_url`, the prefix of the generation its
    /// `version` names (nothing for version 0), then the chunk id.
    ///
    /// `None` if a non-zero version names a generation the dataset doesn't carry, or per
    /// [`Self::hash`].
    pub fn url(&self) -> Option<String> {
        let hash = self.hash()?;
        let mut url = String::with_capacity(URL_CAPACITY);
        url.push_str(self.dataset.base_url());
        if self.version() != 0 {
            push_segment(&mut url, self.dataset.get_generation(self.version())?.base_url());
        }
        if !url.ends_with('/') {
            url.push('/');
        }
        push_chunk_id(&mut url, self.top(), self.first_block(), self.last_block(), hash);
        Some(url)
    }

    pub fn worker_indexes(&self) -> impl Iterator<Item = u16> + 'a {
        let offsets = self.dataset.worker_offsets();
        let start = offsets.get(self.index as usize) as usize;
        let end = offsets.get(self.index as usize + 1) as usize;
        let indexes = self.dataset.worker_indexes();
        (start..end.min(indexes.len())).map(move |i| indexes.get(i))
    }
}

impl<'a> assignment_fb::WorkerAssignmentDataset<'a> {
    pub fn chunk_count(&self) -> usize {
        self.first_blocks().len()
    }

    pub fn chunk(&self, index: u32) -> Option<WorkerChunk<'a>> {
        ((index as usize) < self.chunk_count()).then_some(WorkerChunk {
            dataset: *self,
            index,
        })
    }

    pub fn chunks(self) -> impl Iterator<Item = WorkerChunk<'a>> {
        (0..self.chunk_count() as u32).map(move |index| WorkerChunk {
            dataset: self,
            index,
        })
    }

    /// The last run starting at or before `index`.
    fn top_at(&self, index: u32) -> Option<u64> {
        let tops = self.tops();
        let above = partition_point(tops.len(), |i| tops.get(i).first_chunk_index() <= index);
        Some(tops.get(above.checked_sub(1)?).top())
    }
}

/// A worker's entry in a [`WorkerAssignment`]: identity, status, sealed headers, and the chunks
/// naming it in their `worker_indexes`.
pub struct AssignedWorker<'f> {
    assignment: assignment_fb::WorkerAssignment<'f>,
    reader: assignment_fb::WorkerEntry<'f>,
    index: u16,
}

impl AssignedWorker<'_> {
    pub fn iter_chunks(&self) -> impl Iterator<Item = WorkerChunk<'_>> + '_ {
        self.iter_chunks_with_dataset().map(|(_, chunk)| chunk)
    }

    /// Paired with the dataset holding each, which carries the generations a `version` resolves
    /// against and the id the chunk doesn't repeat.
    pub fn iter_chunks_with_dataset(
        &self,
    ) -> impl Iterator<Item = (assignment_fb::WorkerAssignmentDataset<'_>, WorkerChunk<'_>)> + '_
    {
        self.assignment
            .datasets()
            .iter()
            .flat_map(move |dataset| self.holdings(dataset).map(move |chunk| (dataset, chunk)))
    }

    pub fn iter_chunks_with_ref(&self) -> impl Iterator<Item = (ChunkRef, WorkerChunk<'_>)> + '_ {
        self.assignment.datasets().iter().enumerate().flat_map(move |(d, dataset)| {
            self.holdings(dataset).map(move |chunk| {
                (
                    ChunkRef {
                        dataset_index: d as u32,
                        chunk_index: chunk.index(),
                    },
                    chunk,
                )
            })
        })
    }

    /// This worker's chunks within one dataset.
    ///
    /// The routing columns are resolved once per dataset, not per chunk: this scan visits every
    /// chunk in the assignment to find the few thousand that are ours, and reaching the columns
    /// through each one costs two vtable lookups and an iterator apiece.
    fn holdings<'a>(
        &self,
        dataset: assignment_fb::WorkerAssignmentDataset<'a>,
    ) -> impl Iterator<Item = WorkerChunk<'a>> + 'a {
        let offsets = dataset.worker_offsets();
        let indexes = dataset.worker_indexes();
        let (index, count, len) = (self.index, dataset.chunk_count(), indexes.len());
        (0..count).filter_map(move |chunk| {
            let start = offsets.get(chunk) as usize;
            let end = (offsets.get(chunk + 1) as usize).min(len);
            (start..end).any(|slot| indexes.get(slot) == index).then_some(WorkerChunk {
                dataset,
                index: chunk as u32,
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

// ===== Portal-facing assignment =====
//
// Same column layout as the worker side, minus everything about downloading.

#[ouroboros::self_referencing]
pub struct PortalAssignment {
    buf: Vec<u8>,

    #[borrows(buf)]
    #[covariant]
    reader: assignment_fb::PortalAssignment<'this>,
}

impl PortalAssignment {
    /// See [`WorkerAssignment::from_owned`].
    ///
    /// # Errors
    ///
    /// If the buffer is malformed, or a dataset's columns disagree.
    pub fn from_owned(buf: Vec<u8>) -> Result<Self, InvalidAssignment> {
        let opts = flatbuffers::VerifierOptions {
            max_tables: 1_000_000_000_000,
            max_apparent_size: 1 << 40, // 1TB
            ..Default::default()
        };
        let assignment = PortalAssignmentTryBuilder {
            buf,
            reader_builder: |buf| {
                flatbuffers::root_with_opts::<assignment_fb::PortalAssignment>(&opts, buf)
            },
        }
        .try_build()?;
        for dataset in assignment.datasets().iter() {
            let chunks = dataset.first_blocks().len();
            check_columns(
                dataset.id(),
                chunks,
                &[
                    ("block_deltas", dataset.block_deltas().len()),
                    ("hashes", dataset.hashes().len()),
                    ("worker_offsets", dataset.worker_offsets().len().saturating_sub(1)),
                ]
                .into_iter()
                .chain(dataset.timestamps().map(|c| ("timestamps", c.len())))
                .chain(dataset.versions().map(|c| ("versions", c.len())))
                .collect::<Vec<_>>(),
                &[(
                    "tops",
                    (!dataset.tops().is_empty()).then(|| dataset.tops().get(0).first_chunk_index()),
                )],
            )?;
        }
        Ok(assignment)
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

    pub fn get_chunk(&self, r: ChunkRef) -> Option<PortalChunk<'_>> {
        let datasets = self.borrow_reader().datasets();
        if (r.dataset_index as usize) >= datasets.len() {
            return None;
        }
        datasets.get(r.dataset_index as usize).chunk(r.chunk_index)
    }

    /// The chunk holding `block`. `block_deltas` gives each chunk its own end, so a block in a gap
    /// is [`ChunkNotFound::InGap`] rather than silently attributed to the chunk before it.
    pub fn find_chunk(&self, dataset: &str, block: u64) -> Result<PortalChunk<'_>, ChunkNotFound> {
        let Some(dataset) = self.get_dataset(dataset) else {
            return Err(ChunkNotFound::UnknownDataset);
        };
        if block > dataset.last_block() {
            return Err(ChunkNotFound::AfterLast);
        }

        let first_blocks = dataset.first_blocks();
        // One past the last chunk starting at or before `block`.
        let above = partition_point(first_blocks.len(), |i| first_blocks.get(i) <= block);
        let Some(index) = above.checked_sub(1) else {
            return Err(ChunkNotFound::BeforeFirst);
        };
        let chunk = dataset.chunk(index as u32).expect("index came from the column's own length");
        if block > chunk.last_block() {
            return Err(ChunkNotFound::InGap);
        }
        Ok(chunk)
    }

    /// The first chunk whose timestamp is at or after `ts`. No `timestamps` column reads as every
    /// chunk sitting at 0.
    ///
    /// Bisecting assumes the column ascends, which ingest doesn't guarantee: an unrecorded
    /// timestamp is 0, and a few step backwards outright. A lookup near one lands on a neighbour.
    pub fn find_chunk_by_timestamp(
        &self,
        dataset: &str,
        ts: u64,
    ) -> Result<PortalChunk<'_>, ChunkNotFound> {
        let Some(dataset) = self.get_dataset(dataset) else {
            return Err(ChunkNotFound::UnknownDataset);
        };

        let count = dataset.chunk_count();
        // The column is read once, not re-resolved through the vtable on every probe.
        let index = match dataset.timestamps() {
            // Bisecting on `< ts` lands on the first chunk at or after it, so runs of equal
            // timestamps resolve to their first member without walking back.
            Some(timestamps) => partition_point(count, |i| timestamps.get(i) < ts),
            // No column reads as every chunk sitting at 0.
            None if ts == 0 => 0,
            None => count,
        };
        if index == count {
            return Err(ChunkNotFound::AfterLast);
        }
        Ok(dataset.chunk(index as u32).expect("index is below the chunk count"))
    }
}

/// One row of a [`PortalAssignmentDataset`](assignment_fb::PortalAssignmentDataset)'s columns: a
/// dataset handle plus an index, so copying it is free.
#[derive(Clone, Copy, Debug)]
pub struct PortalChunk<'a> {
    dataset: assignment_fb::PortalAssignmentDataset<'a>,
    index: u32,
}

impl<'a> PortalChunk<'a> {
    pub fn index(&self) -> u32 {
        self.index
    }

    pub fn dataset(&self) -> assignment_fb::PortalAssignmentDataset<'a> {
        self.dataset
    }

    pub fn first_block(&self) -> u64 {
        self.dataset.first_blocks().get(self.index as usize)
    }

    pub fn last_block(&self) -> u64 {
        self.first_block() + self.dataset.block_deltas().get(self.index as usize) as u64
    }

    /// Absolute epoch milliseconds; `None` when the dataset carries no timestamps at all.
    pub fn last_block_timestamp(&self) -> Option<u64> {
        self.dataset.timestamps().map(|column| column.get(self.index as usize))
    }

    /// Which copy of the chunk workers serve; 0 is the ingested one.
    pub fn version(&self) -> u32 {
        self.dataset.versions().map_or(0, |versions| versions.get(self.index as usize))
    }

    /// The top-level directory the chunk lives under, resolved through the run it falls in.
    pub fn top(&self) -> u64 {
        self.dataset
            .top_at(self.index)
            .expect("the first top run covers chunk 0 onwards")
    }

    /// The short hash, NUL padding trimmed. `None` only if the bytes aren't UTF-8.
    pub fn hash(&self) -> Option<&'a str> {
        let hash = self.dataset.hashes().get(self.index as usize);
        let bytes = &hash.0[..hash.0.iter().position(|&b| b == 0).unwrap_or(hash.0.len())];
        std::str::from_utf8(bytes).ok()
    }

    /// The chunk id a query names, rebuilt from its columns — same form as before, so it drops
    /// straight into `Query.chunk_id`. `None` on the same terms as [`Self::hash`].
    pub fn id(&self) -> Option<String> {
        let mut id = String::with_capacity(ID_CAPACITY);
        push_chunk_id(&mut id, self.top(), self.first_block(), self.last_block(), self.hash()?);
        Some(id)
    }

    pub fn worker_indexes(&self) -> impl Iterator<Item = u16> + 'a {
        let offsets = self.dataset.worker_offsets();
        let start = offsets.get(self.index as usize) as usize;
        let end = offsets.get(self.index as usize + 1) as usize;
        let indexes = self.dataset.worker_indexes();
        (start..end.min(indexes.len())).map(move |i| indexes.get(i))
    }
}

impl<'a> assignment_fb::PortalAssignmentDataset<'a> {
    pub fn chunk(&self, index: u32) -> Option<PortalChunk<'a>> {
        ((index as usize) < self.chunk_count()).then_some(PortalChunk {
            dataset: *self,
            index,
        })
    }

    pub fn chunks(self) -> impl Iterator<Item = PortalChunk<'a>> {
        (0..self.chunk_count() as u32).map(move |index| PortalChunk {
            dataset: self,
            index,
        })
    }

    /// The last run starting at or before `index`; `None` only if the runs don't start at 0.
    fn top_at(&self, index: u32) -> Option<u64> {
        let tops = self.tops();
        let above = partition_point(tops.len(), |i| tops.get(i).first_chunk_index() <= index);
        Some(tops.get(above.checked_sub(1)?).top())
    }
}

/// `{:010}` without `std::fmt`, which costs ~35ns a value — several times the column reads around
/// it. Values too wide for the pad print in full, as `{:010}` does.
fn push_padded(out: &mut String, value: u64) {
    const PAD: u64 = 10_000_000_000;
    if value >= PAD {
        out.push_str(&value.to_string());
        return;
    }
    let mut digits = [b'0'; 10];
    let mut rest = value;
    let mut at = digits.len();
    while rest > 0 {
        at -= 1;
        digits[at] = b'0' + (rest % 10) as u8;
        rest /= 10;
    }
    out.push_str(std::str::from_utf8(&digits).expect("ascii digits"));
}

/// Appends `top/first_block-last_block-hash`, the chunk id both formats split into columns.
fn push_chunk_id(out: &mut String, top: u64, first_block: u64, last_block: u64, hash: &str) {
    push_padded(out, top);
    out.push('/');
    push_padded(out, first_block);
    out.push('-');
    push_padded(out, last_block);
    out.push('-');
    out.push_str(hash);
}

/// Three ten-digit numbers, three separators, a hash of up to eight.
const ID_CAPACITY: usize = 3 * 10 + 3 + 8;
/// Plus a base url and a generation prefix, so a url is one allocation.
const URL_CAPACITY: usize = ID_CAPACITY + 96;

/// `slice::partition_point` over anything subscriptable. `pred` must hold for a prefix only.
fn partition_point(len: usize, mut pred: impl FnMut(usize) -> bool) -> usize {
    let (mut low, mut high) = (0, len);
    while low < high {
        let mid = low + (high - low) / 2;
        if pred(mid) {
            low = mid + 1;
        } else {
            high = mid;
        }
    }
    low
}

/// Identity and routing eligibility only: a portal never downloads, so it gets no headers, and
/// never needs one worker's whole chunk list.
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

#[cfg(test)]
mod chunk_id_format {
    use super::{push_chunk_id, push_padded};

    /// The hand-rolled padding exists because `{:010}` costs ~35ns a value, so it has to agree
    /// with `{:010}` exactly — including where mainnet cannot reach it. The largest block number
    /// in the whole assignment is about 70 million, so nothing there exercises a value wide enough
    /// to overflow the pad, which is precisely where a hand-rolled formatter would be wrong.
    #[test]
    fn padding_matches_std_fmt() {
        for value in [
            0,
            1,
            9,
            10,
            99,
            1_000_000_000,
            9_999_999_999,  // the widest the pad holds
            10_000_000_000, // one wider: `{:010}` prints all eleven digits
            123_456_789_012_345,
            u64::MAX,
        ] {
            let mut ours = String::new();
            push_padded(&mut ours, value);
            assert_eq!(ours, format!("{value:010}"), "padding {value}");
        }
    }

    /// A portal parses a chunk id by fixed offsets rather than by pattern: separators at 10, 21
    /// and 32, and a total length of 38..=41. Deriving the string from columns has to keep landing
    /// inside that window, or the format would produce ids its own reader refuses.
    #[test]
    fn ids_fit_the_portals_fixed_offsets() {
        for (top, first, last, hash) in [
            (0, 0, 0, "abcde"),
            (221_000_000, 221_000_000, 221_000_649, "9QgFD"),
            (9_999_999_999, 9_999_999_999, 9_999_999_999, "274f02d8"),
        ] {
            let mut id = String::new();
            push_chunk_id(&mut id, top, first, last, hash);
            assert!((38..=41).contains(&id.len()), "{id} is {} bytes", id.len());
            assert_eq!(id.as_bytes()[10], b'/', "{id}");
            assert_eq!(id.as_bytes()[21], b'-', "{id}");
            assert_eq!(id.as_bytes()[32], b'-', "{id}");
        }
    }

    #[test]
    fn ids_match_std_fmt() {
        for (top, first, last, hash) in [
            (0, 0, 0, "a"),
            (221_000_000, 221_000_000, 221_000_649, "9QgFD"),
            (0, 116_400, 127_899, "274f02d8"),
            (9_999_999_999, 9_999_999_999, 9_999_999_999, "abcdefgh"),
            (10_000_000_000, u64::MAX, u64::MAX, "z"),
        ] {
            let mut ours = String::new();
            push_chunk_id(&mut ours, top, first, last, hash);
            assert_eq!(ours, format!("{top:010}/{first:010}-{last:010}-{hash}"));
        }
    }
}

#[cfg(all(test, feature = "reader"))]
mod malformed {
    use super::WorkerAssignment as ReadWorkerAssignment;
    use crate::assignment_fb::{
        ChunkHash, TableRoster, TopRun, WorkerAssignment, WorkerAssignmentArgs,
        WorkerAssignmentDataset, WorkerAssignmentDatasetArgs, WorkerEntry,
    };

    /// A blob the flatbuffers verifier accepts, with three chunks by `first_blocks` and `dense`
    /// entries in every other column. Only a hand-built one can disagree with itself — the builder
    /// appends to every column together.
    fn blob(dense: usize, tops: &[TopRun]) -> Vec<u8> {
        let mut fbb = flatbuffers::FlatBufferBuilder::new();
        let id = fbb.create_string("s3://short");
        let base_url = fbb.create_string("https://short.sqd-datasets.io");
        let first_blocks = fbb.create_vector(&[0u64, 1000, 2000]);
        let block_deltas = fbb.create_vector(&vec![999u32; dense]);
        let hashes = fbb.create_vector(&vec![ChunkHash::new(b"abcdefgh"); dense]);
        let tops = fbb.create_vector(tops);
        let sizes = fbb.create_vector(&vec![1u32; dense]);
        let write_schema_ids = fbb.create_vector(&vec![1u32; dense]);
        let worker_offsets = fbb.create_vector(&vec![0u32; dense + 1]);
        let worker_indexes = fbb.create_vector::<u16>(&[]);
        let dataset = WorkerAssignmentDataset::create(
            &mut fbb,
            &WorkerAssignmentDatasetArgs {
                id: Some(id),
                last_block: 2999,
                base_url: Some(base_url),
                first_blocks: Some(first_blocks),
                block_deltas: Some(block_deltas),
                hashes: Some(hashes),
                tops: Some(tops),
                sizes: Some(sizes),
                write_schema_ids: Some(write_schema_ids),
                worker_offsets: Some(worker_offsets),
                worker_indexes: Some(worker_indexes),
                ..Default::default()
            },
        );
        let datasets = fbb.create_vector(&[dataset]);
        let workers = fbb.create_vector::<flatbuffers::WIPOffset<WorkerEntry>>(&[]);
        let schemas = fbb.create_vector::<flatbuffers::WIPOffset<TableRoster>>(&[]);
        let root = WorkerAssignment::create(
            &mut fbb,
            &WorkerAssignmentArgs {
                datasets: Some(datasets),
                workers: Some(workers),
                schemas: Some(schemas),
            },
        );
        fbb.finish(root, None);
        fbb.finished_data().to_vec()
    }

    /// Every chunk accessor subscripts a column with an index taken from `first_blocks`, so a
    /// short column used to mean a panic from inside flatbuffers — after `from_owned` had said the
    /// blob verified.
    #[test]
    fn columns_that_disagree_are_rejected() {
        let Err(error) = ReadWorkerAssignment::from_owned(blob(1, &[TopRun::new(0, 0)])) else {
            panic!("the columns cannot all be subscripted by the same index");
        };
        let message = error.to_string();
        assert!(message.contains("s3://short"), "unexpected error: {message}");
        assert!(message.contains("holds 1 of 3 chunks"), "unexpected error: {message}");
    }

    /// The portal carries its own column list, so it needs its own proof that the list is right —
    /// a wrong field name there would check nothing and pass everything.
    #[test]
    fn the_portal_checks_its_own_columns() {
        use crate::assignment_fb::{
            PortalAssignment, PortalAssignmentArgs, PortalAssignmentDataset,
            PortalAssignmentDatasetArgs, PortalEntry,
        };

        let mut fbb = flatbuffers::FlatBufferBuilder::new();
        let id = fbb.create_string("s3://short");
        let first_blocks = fbb.create_vector(&[0u64, 1000, 2000]);
        let block_deltas = fbb.create_vector(&[999u32]); // one delta for three chunks
        let hashes = fbb.create_vector(&[ChunkHash::new(b"abcdefgh"); 3]);
        let tops = fbb.create_vector(&[TopRun::new(0, 0)]);
        let worker_offsets = fbb.create_vector(&[0u32; 4]);
        let worker_indexes = fbb.create_vector::<u16>(&[]);
        let dataset = PortalAssignmentDataset::create(
            &mut fbb,
            &PortalAssignmentDatasetArgs {
                id: Some(id),
                last_block: 2999,
                first_blocks: Some(first_blocks),
                block_deltas: Some(block_deltas),
                hashes: Some(hashes),
                tops: Some(tops),
                worker_offsets: Some(worker_offsets),
                worker_indexes: Some(worker_indexes),
                ..Default::default()
            },
        );
        let datasets = fbb.create_vector(&[dataset]);
        let workers = fbb.create_vector::<flatbuffers::WIPOffset<PortalEntry>>(&[]);
        let root = PortalAssignment::create(
            &mut fbb,
            &PortalAssignmentArgs {
                datasets: Some(datasets),
                workers: Some(workers),
            },
        );
        fbb.finish(root, None);

        let Err(error) = super::PortalAssignment::from_owned(fbb.finished_data().to_vec()) else {
            panic!("block_deltas cannot be subscripted by every chunk index");
        };
        assert!(
            error.to_string().contains("block_deltas holds 1 of 3 chunks"),
            "unexpected error: {error}"
        );
    }

    /// `top()` resolves a chunk through the last run at or before it, so an empty column leaves
    /// chunk 0 with nothing to land on. The dense columns agree here, so only the runs are wrong.
    #[test]
    fn a_run_column_must_cover_chunk_zero() {
        for (tops, expected) in
            [(&[][..], "is empty"), (&[TopRun::new(1, 0)][..], "starts at chunk 1, not 0")]
        {
            let Err(error) = ReadWorkerAssignment::from_owned(blob(3, tops)) else {
                panic!("no run covers chunk 0");
            };
            assert!(error.to_string().contains(expected), "unexpected error: {error}");
        }
    }
}
