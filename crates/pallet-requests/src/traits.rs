use sp_runtime::DispatchError;
use sp_std::result::Result;

pub trait SchedulerInterface {
    type RequestId;
    type Request;
    type Status;

    fn schedule(
        request_id: Self::RequestId,
        request: Self::Request,
    ) -> Result<Self::Status, DispatchError>;
}

pub trait RequestIdGenerator {
    type Id;
    type Request;

    fn generate_id(request: Self::Request) -> Self::Id;
}
