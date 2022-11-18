use crate::common::{BlockRange, ChainId};
use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub type IpnsHash = [u8; 32];

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub enum ChainType {
    NativeEth,
    NativeSubstrate,
    SubstrateEvm,
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct DataSource {
    pub chain_type: ChainType,
    pub chain_id: ChainId,
    pub block_range: BlockRange,
    pub ipns: IpnsHash,
}
