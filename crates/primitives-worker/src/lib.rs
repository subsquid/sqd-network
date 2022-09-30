//! Networks related primitives.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode, MaxEncodedLen};
use primitives_networks::{NetRange, NetRequest, Network};
use scale_info::TypeInfo;
use sp_std::{fmt::Debug, prelude::*};

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct DockerImage;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct Command;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct ResultStorage;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct Task {
    pub docker_image: DockerImage,
    pub command: Command,
    pub request: NetRequest,
    pub result_storage: ResultStorage,
    pub status: Status,
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum Status {
    InProgress,
    Done,
    Stopped,
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct DataAvailability {
    pub network: Network,
    pub range: NetRange,
}
