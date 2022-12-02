//! Traits that pallet uses.

use sp_runtime::DispatchResult;

/// An interface to update request status.
pub trait UpdateRequestStatus {
    /// The task type.
    type Task;
    /// The result type.
    type Result;

    /// Update request status.
    fn update_request_status(task: Self::Task, result: Self::Result) -> DispatchResult;
}
