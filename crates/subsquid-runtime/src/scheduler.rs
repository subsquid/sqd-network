use crate::{requests::Status, SubstrateNativeRequests};
use pallet_substrate_native_requests::{traits::SchedulerInterface, Request};
use sp_runtime::DispatchResult;

pub struct Scheduler;

impl SchedulerInterface for Scheduler {
    type RequestId = [u8; 32];
    type Request = Request;

    fn schedule(request_id: Self::RequestId, _request: Self::Request) -> DispatchResult {
        SubstrateNativeRequests::update_status(request_id, Status::Scheduling)?;
        // Schedule request
        SubstrateNativeRequests::update_status(request_id, Status::Scheduled)?;
        Ok(())
    }
}
