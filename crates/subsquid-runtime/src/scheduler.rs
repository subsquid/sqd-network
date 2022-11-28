use crate::requests::{Request, RequestId, Status};
use crate::WorkersScheduler;
use pallet_requests::traits::SchedulerInterface;
use sp_runtime::DispatchError;
use sp_std::result::Result;

pub struct Scheduler;

impl SchedulerInterface for Scheduler {
    type RequestId = RequestId;
    type Request = Request;
    type Status = Status;

    fn schedule(_request_id: RequestId, request: Request) -> Result<Status, DispatchError> {
        WorkersScheduler::schedule(request)?;
        Ok(Status::Scheduled)
    }
}
