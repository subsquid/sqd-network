use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub type IpnsHash = [u8; 32];

pub type Image = ();

// Check the link from Eldar.
pub type DataChank = ();

#[derive(PartialEq, Eq, Clone, Encode, Decode, Debug, TypeInfo, MaxEncodedLen)]
pub struct DataSource {
    pub image: Image,
    pub ipns: IpnsHash,
    pub data_chunks: DataChank,
}
