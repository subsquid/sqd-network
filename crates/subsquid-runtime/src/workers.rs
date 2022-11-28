use crate::{
    requests::{RequestId, Status},
    Requests,
};
use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::traits::ConstU32;
use pallet_worker::traits::UpdateRequestStatus as UpdateRequestStatusT;
use scale_info::TypeInfo;
use sp_runtime::{BoundedVec, DispatchError};
use sp_std::{fmt::Debug, prelude::*};

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub enum DockerImage {
    EthNetwork,
}

pub const MAX_COMMAND_BYTES: u32 = 100;
pub type Command = BoundedVec<u8, ConstU32<MAX_COMMAND_BYTES>>;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct TaskData {
    pub request_id: RequestId,
    pub docker_image: DockerImage,
    pub command: Command,
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub enum Task {
    Sleep,
    Execute(TaskData),
}

impl Default for Task {
    fn default() -> Self {
        Task::Sleep
    }
}

pub const MAX_STDOUT_BYTES: u32 = 100;
pub const MAX_STDERR_BYTES: u32 = 100;

pub type StdOut = BoundedVec<u8, ConstU32<MAX_STDOUT_BYTES>>;
pub type StdErr = BoundedVec<u8, ConstU32<MAX_STDERR_BYTES>>;
pub type ExitCode = u32;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct Result {
    pub std_out: StdOut,
    pub std_err: StdErr,
    pub exit_code: ExitCode,
}

pub struct UpdateRequestStatus;

impl UpdateRequestStatusT for UpdateRequestStatus {
    type Task = Task;
    type Result = Result;

    fn update_request_status(task: Self::Task, result: Self::Result) -> sp_runtime::DispatchResult {
        match task {
            Task::Sleep => return Err(DispatchError::Other("InvalidTask")),
            Task::Execute(task_data) => {
                let request_id = task_data.request_id;
                Requests::update_status(request_id, Status::Done(result))?;
                Ok(())
            }
        }
    }
}
