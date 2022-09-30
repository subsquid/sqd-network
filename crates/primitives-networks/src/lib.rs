//! Networks related primitives.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_std::{fmt::Debug, prelude::*};

pub type NetRange = ();

pub type NetFilter = ();

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum Network {
    Ethereum,
    Kusama,
    Acala,
}

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct NetRequest {
    pub network: Network,
    pub net_range: NetRange,
    pub net_filter: NetFilter,
}
