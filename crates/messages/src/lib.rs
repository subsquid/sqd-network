// sqd-messages, message definitions for the SQD Network.
// Copyright (C) 2024 Subsquid Labs GmbH

// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.

// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.

// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use std::{
    collections::HashMap,
    fmt::{Debug, Formatter},
    ops::{Deref, DerefMut},
};

pub use prost::Message as ProstMsg;
use sha3::{Digest, Sha3_256};

pub mod data_chunk;
pub mod range;
#[cfg(feature = "signatures")]
pub mod signatures;
#[cfg(feature = "semver")]
mod versions;

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
            query_result::Result::TimeoutV1(()) => Self::Timeout("worker timeout".to_string()),
            query_result::Result::Timeout(msg) => Self::Timeout(msg.clone()),
        }
    }
}

impl SizeAndHash {
    pub fn compute(data: impl AsRef<[u8]>) -> Self {
        let size = u32::try_from(data.as_ref().len()).expect("Data size above u32::MAX");
        let mut hasher = Sha3_256::new();
        hasher.update(data);
        let hash = hasher.finalize();
        Self {
            size: Some(size),
            sha3_256: hash.to_vec(),
        }
    }
}

impl Debug for OkResult {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "OkResult {{ data: <{} bytes>, exec_plan: <{} bytes> }}",
            self.data.len(),
            self.exec_plan.as_ref().map(Vec::len).unwrap_or_default()
        )
    }
}

impl From<worker_logs_msg::Msg> for WorkerLogsMsg {
    fn from(msg: worker_logs_msg::Msg) -> Self {
        Self { msg: Some(msg) }
    }
}

impl From<Vec<QueryExecuted>> for WorkerLogsMsg {
    fn from(queries_executed: Vec<QueryExecuted>) -> Self {
        worker_logs_msg::Msg::QueryLogs(queries_executed.into()).into()
    }
}

impl From<gateway_log_msg::Msg> for GatewayLogMsg {
    fn from(msg: gateway_log_msg::Msg) -> Self {
        Self { msg: Some(msg) }
    }
}

impl From<Vec<QueryExecuted>> for QueryLogs {
    fn from(queries_executed: Vec<QueryExecuted>) -> Self {
        Self { queries_executed }
    }
}

impl QueryResult {
    pub fn new(query_id: String, result: query_result::Result) -> Self {
        Self {
            query_id,
            result: Some(result),
        }
    }
}

impl WorkerLogsMsg {
    pub fn push(&mut self, log: QueryExecuted) {
        match &mut self.msg {
            None => {
                self.msg = Some(worker_logs_msg::Msg::QueryLogs(QueryLogs {
                    queries_executed: vec![log],
                }));
            }
            Some(worker_logs_msg::Msg::QueryLogs(QueryLogs { queries_executed })) => {
                queries_executed.push(log)
            }
        }
    }

    pub fn len(&self) -> usize {
        match &self.msg {
            None => 0,
            Some(worker_logs_msg::Msg::QueryLogs(QueryLogs { queries_executed })) => {
                queries_executed.len()
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn pop(&mut self) -> Option<QueryExecuted> {
        match &mut self.msg {
            None => None,
            Some(worker_logs_msg::Msg::QueryLogs(QueryLogs { queries_executed })) => {
                queries_executed.pop()
            }
        }
    }
}
