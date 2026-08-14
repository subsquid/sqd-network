//! Facade over the three raw flatc-generated schemas (`assignment_generated`,
//! `worker_assignment_generated`, `portal_assignment_generated`), re-exported here into one flat
//! namespace, plus small hand-written impls for all three -- despite the module's name, it's not
//! legacy-only.

use libp2p_identity::PeerId;

pub(crate) use crate::{
    assignment_generated::*, portal_assignment_generated::*, worker_assignment_generated::*,
};

impl Eq for WorkerId {}

impl TryInto<PeerId> for WorkerId {
    type Error = libp2p_identity::ParseError;

    fn try_into(self) -> Result<PeerId, Self::Error> {
        PeerId::from_bytes(&self.0)
    }
}

impl From<PeerId> for WorkerId {
    fn from(peer_id: PeerId) -> Self {
        let buf = peer_id.to_bytes();
        let (bytes, rest) =
            buf.split_first_chunk().expect("PeerId should always have a valid length");
        debug_assert_eq!(rest, &[] as &[u8], "PeerId should not have extra bytes");
        WorkerId(*bytes)
    }
}

#[test]
fn test_worker_id_conversion() {
    let peer_id = libp2p_identity::Keypair::generate_ed25519().public().to_peer_id();
    let worker_id: WorkerId = peer_id.into();
    let converted_peer_id: PeerId = worker_id.try_into().expect("Conversion should succeed");
    assert_eq!(peer_id, converted_peer_id, "PeerId conversion failed");
}

impl Dataset<'_> {
    pub fn first_block(&self) -> u64 {
        self.chunks().get(0).first_block()
    }

    pub fn last_block_hash(&self) -> Option<&str> {
        self.chunks().get(self.chunks().len() - 1).last_block_hash()
    }
}

impl PortalAssignmentDataset<'_> {
    pub fn first_block(&self) -> u64 {
        self.chunks().get(0).first_block()
    }
}

impl<'a> WorkerAssignmentDataset<'a> {
    /// The generation a chunk version's files were written under. `None` for version 0, which has
    /// no prefix, and for a version this dataset never registered.
    pub fn get_generation(&self, version: u32) -> Option<GenerationEntry<'a>> {
        self.generations()?
            .lookup_by_key(version, |generation, key| generation.key_compare_with_value(*key))
    }

    /// Where one of this dataset's chunks keeps its files: the chunk's `dataset_base_url`, then
    /// the prefix of the generation its `version` names -- nothing for version 0, the ingested
    /// layout -- then the chunk id.
    ///
    /// Lives on the dataset because that is what holds the generations, and what the caller is
    /// holding anyway: chunks are only reachable through it.
    ///
    /// `None` if a non-zero version names a generation this dataset doesn't carry.
    pub fn chunk_url(&self, chunk: WorkerAssignmentChunk<'_>) -> Option<String> {
        let mut url = chunk.dataset_base_url().to_owned();
        if chunk.version() != 0 {
            push_segment(&mut url, self.get_generation(chunk.version())?.base_url());
        }
        push_segment(&mut url, chunk.id());
        Some(url)
    }
}

/// Appends a path segment with exactly one separator, whichever side already carries it.
fn push_segment(url: &mut String, segment: &str) {
    if !url.ends_with('/') {
        url.push('/');
    }
    url.push_str(segment.trim_start_matches('/'));
}
