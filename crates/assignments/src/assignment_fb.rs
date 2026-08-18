//! FlatBuffers schema exports and helper implementations.

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
    pub fn chunk_count(&self) -> usize {
        self.first_blocks().len()
    }

    pub fn first_block(&self) -> u64 {
        self.first_blocks().get(0)
    }
}

impl<'a> WorkerAssignmentDataset<'a> {
    /// `None` for version 0, which has no prefix, and for a version never registered here.
    pub fn get_generation(&self, version: u32) -> Option<GenerationEntry<'a>> {
        self.generations()?
            .lookup_by_key(version, |generation, key| generation.key_compare_with_value(*key))
    }
}

/// Appends a path segment with exactly one separator, whichever side already carries it.
pub(crate) fn push_segment(url: &mut String, segment: &str) {
    if !url.ends_with('/') {
        url.push('/');
    }
    url.push_str(segment.trim_start_matches('/'));
}
