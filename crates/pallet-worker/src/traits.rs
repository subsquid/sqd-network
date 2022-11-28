use sp_runtime::DispatchResult;

pub trait UpdateRequestStatus {
    type Task;
    type Result;

    fn update_request_status(task: Self::Task, result: Self::Result) -> DispatchResult;
}
