use std::fmt::{Debug, Formatter};
use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
};

pub use prost::Message as ProstMsg;
use sha3::{Digest, Sha3_256};

pub mod data_chunk;
pub mod range;
#[cfg(feature = "signatures")]
pub mod signatures;

include!(concat!(env!("OUT_DIR"), "/messages.rs"));

impl Deref for WorkerState {
    type Target = HashMap<String, RangeSet>;

    fn deref(&self) -> &Self::Target {
        &self.datasets
    }
}

impl DerefMut for WorkerState {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.datasets
    }
}

impl From<HashMap<String, RangeSet>> for WorkerState {
    fn from(datasets: HashMap<String, RangeSet>) -> Self {
        Self { datasets }
    }
}

impl From<&query_result::Result> for query_finished::Result {
    fn from(result: &query_result::Result) -> Self {
        match result {
            query_result::Result::Ok(OkResult { data, .. }) => Self::Ok(SizeAndHash::compute(data)),
            query_result::Result::BadRequest(err) => Self::BadRequest(err.clone()),
            query_result::Result::ServerError(err) => Self::ServerError(err.clone()),
            query_result::Result::NoAllocation(()) => Self::NoAllocation(()),
            query_result::Result::Timeout(()) => Self::Timeout(()),
        }
    }
}

impl SizeAndHash {
    pub fn compute(data: impl AsRef<[u8]>) -> Self {
        let size = data.as_ref().len() as u32;
        let mut hasher = Sha3_256::new();
        hasher.update(data);
        let hash = hasher.finalize();
        Self {
            size: Some(size),
            sha3_256: hash.to_vec(),
        }
    }
}

#[cfg(feature = "semver")]
impl Ping {
    pub fn sem_version(&self) -> semver::Version {
        self.version
            .as_ref()
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| semver::Version::new(0, 0, 1))
    }
}

impl Debug for OkResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OkResult {{ data: <{} bytes>, exec_plan: <{} bytes> }}",
            self.data.len(),
            self.exec_plan.as_ref().map(|b| b.len()).unwrap_or_default()
        )
    }
}
