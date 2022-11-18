use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_std::{fmt::Debug, prelude::*};

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum DockerImage {
    SubstrateWorker,
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum Command {
    Parse,
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct ExecTask {
    pub docker_image: DockerImage,
    pub command: Command,
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum Task {
    Sleep,
    Execute(ExecTask),
}

impl Default for Task {
    fn default() -> Self {
        Task::Sleep
    }
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum ResultStorage {
    Ipfs([u8; 32]),
}
