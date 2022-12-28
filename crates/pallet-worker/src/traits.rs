//! Traits that pallet uses.

use sp_runtime::DispatchResult;

/// An interface to update request status.
pub trait UpdateRequestStatus {
    /// The task id type.
    type TaskId;
    /// The task result type.
    type TaskResult;

    /// Update request status.
    fn update_request_status(
        task_id: Self::TaskId,
        task_result: Self::TaskResult,
    ) -> DispatchResult;
}

/// An interface to extract task id from task
pub trait GetTaskId {
    /// The task type.
    type Task;
    /// The task id type.
    type TaskId;

    /// Get task id.
    fn get_id(task: &Self::Task) -> Option<Self::TaskId>;
}
