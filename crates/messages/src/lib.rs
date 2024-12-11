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

use std::fmt::{Debug, Formatter};

pub use prost::Message as ProstMsg;

#[cfg(any(feature = "assignment_reader", feature = "assignment_writer"))]
pub mod assignments;
#[cfg(feature = "bitstring")]
pub mod bitstring;
pub mod data_chunk;
pub mod range;
#[cfg(feature = "signatures")]
pub mod signatures;
#[cfg(feature = "semver")]
mod versions;

include!(concat!(env!("OUT_DIR"), "/messages.rs"));

impl Debug for QueryOk {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "QueryOk {{ data: <{} bytes> }}", self.data.len(),)
    }
}

impl From<query_error::Err> for query_result::Result {
    fn from(err: query_error::Err) -> Self {
        query_result::Result::Err(QueryError { err: Some(err) })
    }
}

impl From<query_error::Err> for query_executed::Result {
    fn from(err: query_error::Err) -> Self {
        query_executed::Result::Err(QueryError { err: Some(err) })
    }
}
