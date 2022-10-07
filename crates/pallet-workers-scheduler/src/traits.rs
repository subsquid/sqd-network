use sp_runtime::{DispatchError, DispatchResult};
use sp_std::result::Result;

pub trait WorkerController {
    type WorkerId;
    type Status;
    type Task;

    /// Get current worker status.
    fn current_status(worker_id: Self::WorkerId) -> Result<Self::Status, DispatchError>;
    /// Run the task for particular worker.
    fn run_task(worker_id: Self::WorkerId, task: Self::Task) -> DispatchResult;
}
