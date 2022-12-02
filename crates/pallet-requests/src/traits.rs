//! Traits that pallet uses.

use sp_runtime::DispatchError;
use sp_std::result::Result;

/// Schedule interface to pass an incoming requests to be scheduled.
pub trait SchedulerInterface {
    /// Requests id type.
    type RequestId;
    /// Request type.
    type Request;
    /// Request status type.
    type Status;

    /// Schedule an incoming requests.
    fn schedule(
        request_id: Self::RequestId,
        request: Self::Request,
    ) -> Result<Self::Status, DispatchError>;
}

/// Generate an request id based on provided request content.
pub trait RequestIdGenerator {
    /// The id type that is used.
    type Id;
    /// The request type.
    type Request;

    /// Generate an requests id.
    fn generate_id(request: Self::Request) -> Self::Id;
}
