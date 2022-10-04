use sp_runtime::DispatchResult;

pub trait SchedulerInterface {
    type RequestId;
    type Request;

    fn schedule(request_id: Self::RequestId, request: Self::Request) -> DispatchResult;
}

pub trait RequestInterface {
    type Id;
    type Data;
    type Status;

    fn generate_id(request: Self::Data) -> Self::Id;
    fn update_status(request_id: Self::Id, status: Self::Status) -> DispatchResult;
}
