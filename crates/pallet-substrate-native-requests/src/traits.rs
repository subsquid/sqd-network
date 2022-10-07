use sp_runtime::DispatchResult;

pub trait SchedulerInterface {
    type RequestId;
    type Request;

    fn schedule(request_id: Self::RequestId, request: Self::Request) -> DispatchResult;
}

pub trait RequestIdGenerator {
    type Id;
    type Data;

    fn generate_id(request: Self::Data) -> Self::Id;
}
