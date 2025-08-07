// sqd-contract-client, client for accessing the on-chain data in the SQD Network.
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

mod cli;
mod client;
mod contracts;
mod error;
mod transport;

pub use ethers::types::{Address, U256};
pub use libp2p::PeerId;

pub use cli::{Network, RpcArgs};
pub use client::{
    get_client, Allocation, Client, EpochStream, NetworkNodes, NodeStream, PortalCluster, Worker,
};
pub use error::ClientError;
