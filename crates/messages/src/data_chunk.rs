//! Shared chunk identifiers and their protobuf range conversion.

pub use sqd_data_chunk::DataChunk;

use crate::Range;

impl From<DataChunk> for Range {
    fn from(chunk: DataChunk) -> Self {
        Self::new(chunk.first_block(), chunk.last_block())
    }
}

#[cfg(test)]
mod tests {
    use super::{DataChunk, Range};

    #[test]
    fn shared_chunk_converts_to_message_range() {
        let chunk = sqd_data_chunk::DataChunk::new(0, 100, 199, "abcde").unwrap();
        let reexported: DataChunk = chunk;
        assert_eq!(Range::from(reexported), Range::new(100, 199));
    }
}
