use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub type ChainId = [u8; 32];

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct BlockRange {
    pub from: u32,
    pub to: u32,
}
