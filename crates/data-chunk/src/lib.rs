//! Validated data chunk identifiers shared by assignments and network messages.

const HASH_MIN_LEN: usize = 5;
const HASH_MAX_LEN: usize = 8;

#[derive(Clone, Copy, Hash, Ord, PartialOrd, Eq, PartialEq)]
pub struct DataChunk {
    top: u64,
    first_block: u64,
    last_block: u64,
    /// NUL-padded, like the assignments' `ChunkHash`.
    last_hash: [u8; HASH_MAX_LEN],
}

impl DataChunk {
    /// `None` unless `top <= first_block <= last_block` and the hash is 5 to 8 word characters.
    pub fn new(top: u64, first_block: u64, last_block: u64, last_hash: &str) -> Option<Self> {
        let valid_hash = (HASH_MIN_LEN..=HASH_MAX_LEN).contains(&last_hash.len())
            && last_hash.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_');
        if !(valid_hash && top <= first_block && first_block <= last_block) {
            return None;
        }
        let mut hash = [0; HASH_MAX_LEN];
        hash[..last_hash.len()].copy_from_slice(last_hash.as_bytes());
        Some(Self {
            top,
            first_block,
            last_block,
            last_hash: hash,
        })
    }

    #[inline]
    pub fn top(&self) -> u64 {
        self.top
    }

    #[inline]
    pub fn first_block(&self) -> u64 {
        self.first_block
    }

    #[inline]
    pub fn last_block(&self) -> u64 {
        self.last_block
    }

    #[inline]
    pub fn last_hash(&self) -> &str {
        let len = self.last_hash.iter().position(|&b| b == 0).unwrap_or(HASH_MAX_LEN);
        // `new` only admits ASCII.
        std::str::from_utf8(&self.last_hash[..len]).expect("ASCII hash")
    }
}

impl std::fmt::Display for DataChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:010}/{:010}-{:010}-{}",
            self.top,
            self.first_block,
            self.last_block,
            self.last_hash()
        )
    }
}

impl std::fmt::Debug for DataChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self}")
    }
}

impl std::str::FromStr for DataChunk {
    type Err = ();

    /// Parses `<top>/<first_block>-<last_block>-<hash>`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (top, range) = s.split_once('/').ok_or(())?;
        let mut parts = range.splitn(3, '-');
        let (Some(first_block), Some(last_block), Some(hash)) =
            (parts.next(), parts.next(), parts.next())
        else {
            return Err(());
        };
        let num = |s: &str| s.parse::<u64>().map_err(|_| ());
        Self::new(num(top)?, num(first_block)?, num(last_block)?, hash).ok_or(())
    }
}

#[cfg(test)]
mod tests {
    use super::DataChunk;

    #[test]
    fn round_trips_through_its_id() {
        for id in [
            "0221000000/0221000000-0221000649-9QgFD",
            "0000000000/0000000100-0000000199-274f02d8",
        ] {
            assert_eq!(id.parse::<DataChunk>().unwrap().to_string(), id);
        }
        assert_eq!("0/5-9-ab_de".parse::<DataChunk>().unwrap().last_hash(), "ab_de");
    }

    #[test]
    fn rejects_malformed_ids() {
        for id in [
            "0000000000-0000000099-abcde",
            "0000000000/0000000000-abcde",
            "000000000x/0000000000-0000000099-abcde",
            "0000000100/0000000000-0000000099-abcde",
            "0000000000/0000000100-0000000099-abcde",
            "0000000000/0000000000-0000000099-",
            "0000000000/0000000000-0000000099-abcd",
            "0000000000/0000000000-0000000099-toolonghash",
            "0000000000/0000000000-0000000099-has-dash",
        ] {
            assert!(id.parse::<DataChunk>().is_err(), "{id}");
        }
    }
}
