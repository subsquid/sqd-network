use crate::{requests::Status, Requests};
use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::traits::ConstU32;
use pallet_worker::traits::{GetTaskId as GetTaskIdT, UpdateRequestStatus as UpdateRequestStatusT};
use scale_info::TypeInfo;
use sp_core::H256;
use sp_runtime::BoundedVec;
use sp_std::{fmt::Debug, prelude::*};

pub type TaskId = [u8; 32];

pub type DockerImage = H256;

pub const MAX_BYTES_FOR_ARG: u32 = 100;
pub const MAX_ARGS: u32 = 100;
pub type Command = BoundedVec<BoundedVec<u8, ConstU32<MAX_BYTES_FOR_ARG>>, ConstU32<MAX_ARGS>>;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct TaskData {
    pub task_id: TaskId,
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

pub type ExitCode = u32;
pub type StdOut = BoundedVec<u8, ConstU32<MAX_STDOUT_BYTES>>;
pub type StdErr = BoundedVec<u8, ConstU32<MAX_STDERR_BYTES>>;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct TaskResult {
    pub exit_code: ExitCode,
    pub stdout: StdOut,
    pub stderr: StdErr,
}

pub struct GetTaskId;

impl GetTaskIdT for GetTaskId {
    type Task = Task;
    type TaskId = TaskId;

    fn get_id(task: &Self::Task) -> Option<Self::TaskId> {
        match task {
            Task::Execute(task) => Some(task.task_id),
            Task::Sleep => None,
        }
    }
}

pub struct UpdateRequestStatus;

impl UpdateRequestStatusT for UpdateRequestStatus {
    type TaskId = TaskId;
    type TaskResult = TaskResult;

    fn update_request_status(
        task_id: Self::TaskId,
        result: Self::TaskResult,
    ) -> sp_runtime::DispatchResult {
        let request_id = task_id;
        Requests::update_status(request_id, Status::Done(result))?;
        Ok(())
    }
}
