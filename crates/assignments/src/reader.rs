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
            parsed.cmp(&id)
        })?;
        Some(Worker {
            assignment: *self.borrow_reader(),
            reader: workers.get(index),
            index: index as u16,
        })
    }

    pub fn workers(
        &self,
    ) -> flatbuffers::Vector<'_, flatbuffers::ForwardsUOffset<assignment_fb::WorkerAssignment<'_>>>
    {
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

        // find last chunk with first_block <= block
        let chunks = dataset.chunks();
        // let k = chunks.binary_search_by_key(&block, |chunk| chunk.first_block());

        let left = binary_search_by(Chunks(&chunks), |itm: &assignment_fb::Chunk<'_>| {
            itm.first_block().cmp(&block)
        });

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
            return Err(ChunkNotFound::BeforeFirst);
        }
        Ok(chunks.get(left as usize))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkNotFound {
    UnknownDataset,
    BeforeFirst,
    AfterLast,
}

pub struct Worker<'f> {
    assignment: assignment_fb::Assignment<'f>,
    reader: assignment_fb::WorkerAssignment<'f>,
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

    pub fn peer_id(&self) -> Result<PeerId, anyhow::Error> {
        Ok((*self.reader.worker_id()).try_into()?)
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

// Needed for Generic
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

fn binary_search_by<'a, V, F>(v: V, mut cmp: F) -> Result<usize, Option<usize>>
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

    let result = if left == -1 {
        None
    } else {
        Some(left as usize)
    };

    Err(result) // watch out what to return!
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
            TestItem(17),
            TestItem(19),
            TestItem(21),
        ]
    }

    fn binary_search_g_le(k: u64, v: &[TestItem]) -> Result<usize, Option<usize>> {
        binary_search_by(TestSlice(&v), |itm| itm.0.cmp(&k))
    }

    fn binary_search_l_ge(k: u64, v: &[TestItem]) -> Result<usize, Option<usize>> {
        match binary_search_by(TestSlice(&v), |itm| itm.0.cmp(&k)) {
            Ok(idx) => Ok(idx),
            Err(None) if 0 < v.len() => Err(Some(0)),
            Err(Some(idx)) if idx + 1 < v.len() => Err(Some(idx + 1)),
            _ => Err(None),
        }
    }

    #[test]
    fn test_find_greatest_le() {
        let v = make_test_vec();

        assert_eq!(binary_search_g_le(11, &v), Ok(0));
        assert_eq!(binary_search_g_le(13, &v), Ok(1));
        assert_eq!(binary_search_g_le(15, &v), Ok(2));
        assert_eq!(binary_search_g_le(17, &v), Ok(3));
        assert_eq!(binary_search_g_le(19, &v), Ok(4));
        assert_eq!(binary_search_g_le(21, &v), Ok(5));

        assert_eq!(binary_search_g_le(10, &v), Err(None));

        assert_eq!(binary_search_g_le(12, &v), Err(Some(0)));
        assert_eq!(binary_search_g_le(14, &v), Err(Some(1)));
        assert_eq!(binary_search_g_le(16, &v), Err(Some(2)));
        assert_eq!(binary_search_g_le(18, &v), Err(Some(3)));
        assert_eq!(binary_search_g_le(20, &v), Err(Some(4)));
        assert_eq!(binary_search_g_le(22, &v), Err(Some(5)));
    }

    #[test]
    fn test_find_least_ge() {
        let v = make_test_vec();

        assert_eq!(binary_search_l_ge(11, &v), Ok(0));
        assert_eq!(binary_search_l_ge(13, &v), Ok(1));
        assert_eq!(binary_search_l_ge(15, &v), Ok(2));
        assert_eq!(binary_search_l_ge(17, &v), Ok(3));
        assert_eq!(binary_search_l_ge(19, &v), Ok(4));
        assert_eq!(binary_search_l_ge(21, &v), Ok(5));

        assert_eq!(binary_search_l_ge(10, &v), Err(Some(0)));
        assert_eq!(binary_search_l_ge(12, &v), Err(Some(1)));
        assert_eq!(binary_search_l_ge(14, &v), Err(Some(2)));
        assert_eq!(binary_search_l_ge(16, &v), Err(Some(3)));
        assert_eq!(binary_search_l_ge(18, &v), Err(Some(4)));
        assert_eq!(binary_search_l_ge(20, &v), Err(Some(5)));

        assert_eq!(binary_search_l_ge(22, &v), Err(None));
    }
}
