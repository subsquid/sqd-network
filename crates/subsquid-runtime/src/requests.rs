use crate::workers::TaskResult;
use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::traits::ConstU32;
use pallet_requests::traits::RequestIdGenerator as RequestIdGeneratorT;
use scale_info::TypeInfo;
use sp_core::H256;
use sp_io::hashing::keccak_256;
use sp_runtime::BoundedVec;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub enum ArchiveQuery {
    /// Inline textual query
    Text(BoundedVec<u8, ConstU32<256>>),
    /// Ipfs content with (big) query
    IPFS(H256),
}

pub type RequestId = [u8; 32];

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct NativeEthRequest {
    pub data_src: H256,
    pub query: ArchiveQuery,
    pub first_block: u32,
    pub last_block: Option<u32>,
}

#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct NativeSubstrateRequest {
    pub chain: [u8; 32],
}

#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct SubstrateEvmRequest {
    pub chain: [u8; 32],
}

pub type Request =
    pallet_requests::Request<NativeEthRequest, NativeSubstrateRequest, SubstrateEvmRequest>;

#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum Error {
    NotAllTaskSuccesfullyFinished,
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub enum Status {
    Scheduled,
    Done(TaskResult),
    Fail(Error),
}

pub struct RequestIdGenerator;

// TODO. Add an entropy.
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
