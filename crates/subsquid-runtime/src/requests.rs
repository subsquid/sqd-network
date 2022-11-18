use codec::{Decode, Encode, MaxEncodedLen};
use pallet_requests::traits::RequestIdGenerator as RequestIdGeneratorT;
use scale_info::TypeInfo;
use sp_io::hashing::keccak_256;

pub type RequestId = [u8; 32];

#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct NativeEthRequest {
    pub chain: [u8; 32],
    pub from: u32,
    pub to: u32,
    pub call: [u8; 32],
}

#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct NativeSubstrateRequest {
    pub chain: [u8; 32],
    pub from: u32,
    pub to: u32,
    pub call: [u8; 32],
}

#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct SubstrateEvmRequest {
    pub chain: [u8; 32],
    pub from: u32,
    pub to: u32,
    pub call: [u8; 32],
}

pub type Request =
    pallet_requests::Request<NativeEthRequest, NativeSubstrateRequest, SubstrateEvmRequest>;

#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum Status {
    Scheduling,
    Scheduled,
    Done,
}

impl Default for Status {
    fn default() -> Self {
        Status::Scheduling
    }
}

pub struct RequestIdGenerator;

impl RequestIdGeneratorT for RequestIdGenerator {
    type Id = RequestId;
    type Request = Request;

    fn generate_id(request: Self::Request) -> Self::Id {
        match request {
            Request::NativeEthRequest(req) => keccak_256(&req.encode()),
            Request::NativeSubstrateRequest(req) => keccak_256(&req.encode()),
            Request::SubstrateEvmRequest(req) => keccak_256(&req.encode()),
        }
    }
}
