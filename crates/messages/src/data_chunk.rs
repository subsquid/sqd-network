use crate::Range;

#[derive(Clone, Ord, PartialOrd, Eq, PartialEq)]
pub struct DataChunk {
    top: u64,
    first_block: u64,
    last_block: u64,
    last_hash: String,
}

impl DataChunk {
    pub fn new(top: u64, first_block: u64, last_block: u64, last_hash: String) -> Self {
        assert!(top <= first_block);
        assert!(first_block <= last_block);
        Self {
            top,
            first_block,
            last_block,
            last_hash,
        }
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
}

impl std::fmt::Display for DataChunk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:010}/{:010}-{:010}-{}",
            self.top, self.first_block, self.last_block, self.last_hash
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

    #[allow(clippy::get_first)]
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let top_range_split = s.split('/').collect::<Vec<_>>();
        let top: u64 = top_range_split.get(0).ok_or(())?.parse().or(Err(()))?;
        let range_str = top_range_split.get(1).ok_or(())?;
        let range_split = range_str.split('-').collect::<Vec<_>>();
        let beg: u64 = range_split.get(0).ok_or(())?.parse().or(Err(()))?;
        let end: u64 = range_split.get(1).ok_or(())?.parse().or(Err(()))?;
        let hash = range_split.get(2).ok_or(())?;
        if top <= beg && beg <= end {
            Ok(Self::new(top, beg, end, (*hash).to_string()))
        } else {
            Err(())
        }
    }
}

impl From<DataChunk> for Range {
    fn from(chunk: DataChunk) -> Self {
        Self::new(chunk.first_block, chunk.last_block)
    }
}
