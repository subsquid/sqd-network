//! Archive related primitives.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use sp_application_crypto::KeyTypeId;

/// Keystore Bioauth Account ID key type definition.
pub const KEY_TYPE: KeyTypeId = KeyTypeId(*b"arid");

/// App key definition.
mod app {
    use sp_application_crypto::{app_crypto, sr25519};
    app_crypto!(sr25519, super::KEY_TYPE);
}

/// App key export.
pub type ArchiveId = app::Public;
