//! Pallet.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub use pallet::*;

pub mod traits;
pub mod weights;

/// We should define a common behavior of Request for different networks
/// as we a going to operate it by making a decision at scheduling level on it.
#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct Request;

#[frame_support::pallet]
pub mod pallet {

    use crate::traits::{RequestInterface, SchedulerInterface};

    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;
    use weights::WeightInfo;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Event type.
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        type Status: Parameter + MaxEncodedLen;
        type RequestId: Parameter + MaxEncodedLen + Copy;
        type RequestInterface: RequestInterface<
            Id = Self::RequestId,
            Data = Request,
            Status = Self::Status,
        >;
        type SchedulerInterface: SchedulerInterface<RequestId = Self::RequestId, Request = Request>;
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Current requests.
    #[pallet::storage]
    #[pallet::getter(fn requests_data)]
    pub type RequestsData<T: Config> =
        StorageMap<_, Twox64Concat, T::RequestId, Request, OptionQuery>;

    /// Current status.
    #[pallet::storage]
    #[pallet::getter(fn status)]
    pub type RequestsStatus<T: Config> =
        StorageMap<_, Twox64Concat, T::RequestId, T::Status, OptionQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewRequest {
            request_id: T::RequestId,
            request: Request,
        },
        StatusUpdate {
            request_id: T::RequestId,
            status: T::Status,
        },
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(T::WeightInfo::request())]
        pub fn request(_origin: OriginFor<T>, request: Request) -> DispatchResult {
            let request_id = T::RequestInterface::generate_id(request);

            Self::deposit_event(Event::NewRequest {
                request_id,
                request,
            });

            T::SchedulerInterface::schedule(request_id, request)?;

            todo!()
        }
    }
}
