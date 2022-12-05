//! Traits that pallet uses.

pub trait PrepareTask {
    /// The request type.
    type Request;
    /// The task type.
    type Task;

    /// Prepare the task based on provided request.
    fn prepare_task(request: Self::Request) -> Self::Task;
}
