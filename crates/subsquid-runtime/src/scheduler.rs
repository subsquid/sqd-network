use crate::{requests::Status, Requests, WorkersScheduler};
use pallet_requests::{traits::SchedulerInterface, Request};
use sp_runtime::DispatchResult;

pub struct Scheduler;

impl SchedulerInterface for Scheduler {
    type RequestId = [u8; 32];
    type Request = Request;

    fn schedule(request_id: Self::RequestId, request: Self::Request) -> DispatchResult {
        Requests::update_status(request_id, Status::Scheduling)?;
        WorkersScheduler::schedule(request)?;
        Requests::update_status(request_id, Status::Scheduled)?;
        Ok(())
    }
}
