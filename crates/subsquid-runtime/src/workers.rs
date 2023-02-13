use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::traits::ConstU32;
use pallet_worker::traits::WorkerConstraints;
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::BoundedVec;
use sp_std::{fmt::Debug, prelude::*};

pub type TaskId = u64;

pub const MAX_BYTES_FOR_ARG: u32 = 100;
pub const MAX_ARGS: u32 = 100;
pub type Command = BoundedVec<BoundedVec<u8, ConstU32<MAX_BYTES_FOR_ARG>>, ConstU32<MAX_ARGS>>;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct TaskSpec {
    pub docker_image: DockerImage,
    pub command: Command,
}

pub const MAX_DOCKER_IMAGE_NAME_LEN: u32 = 100;
#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct DockerImage {
    pub name: BoundedVec<u8, ConstU32<MAX_DOCKER_IMAGE_NAME_LEN>>,
    pub digest: H256,
}

pub const MAX_STDOUT_BYTES: u32 = 1024;
pub const MAX_STDERR_BYTES: u32 = 1024;

pub type ExitCode = u32;
pub type StdOut = BoundedVec<u8, ConstU32<MAX_STDOUT_BYTES>>;
pub type StdErr = BoundedVec<u8, ConstU32<MAX_STDERR_BYTES>>;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct TaskResult {
    pub exit_code: ExitCode,
    pub stdout: StdOut,
    pub stderr: StdErr,
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct HardwareSpecs {
    pub num_cpu_cores: Option<u16>,
    pub memory_bytes: Option<u64>,
    pub storage_bytes: Option<u64>,
}

impl WorkerConstraints<HardwareSpecs> for HardwareSpecs {
    fn worker_suitable(&self, worker_spec: &HardwareSpecs) -> bool {
        match (self.num_cpu_cores, worker_spec.num_cpu_cores) {
            (Some(_), None) => return false,
            (Some(required), Some(given)) if given < required => return false,
            _ => {}
        }

        match (self.memory_bytes, worker_spec.memory_bytes) {
            (Some(_), None) => return false,
            (Some(required), Some(given)) if given < required => return false,
            _ => {}
        }

        match (self.storage_bytes, worker_spec.storage_bytes) {
            (Some(_), None) => return false,
            (Some(required), Some(given)) if given < required => return false,
            _ => {}
        }

        true
    }
}
