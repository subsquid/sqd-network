//! Pallet.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub use pallet::*;

pub mod traits;
pub mod weights;

#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct Request {
    pub chain: [u8; 32],
    pub from: u32,
    pub to: u32,
    pub call: [u8; 32],
}

#[frame_support::pallet]
pub mod pallet {

    use crate::traits::{RequestIdGenerator, SchedulerInterface};

    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;
    use weights::WeightInfo;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Event type.
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        type Status: Parameter + MaxEncodedLen + Copy;
        type RequestId: Parameter + MaxEncodedLen + Copy;
        type RequestIdGenerator: RequestIdGenerator<Id = Self::RequestId, Data = Request>;
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
            who: T::AccountId,
            request_id: T::RequestId,
            request: Request,
        },
        StatusUpdate {
            request_id: T::RequestId,
            status: T::Status,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        RequestIdAlreadyExists,
        NoRequestId,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(T::WeightInfo::request())]
        pub fn request(origin: OriginFor<T>, request: Request) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let request_id = T::RequestIdGenerator::generate_id(request);

            if <RequestsData<T>>::contains_key(request_id) {
                return Err(<Error<T>>::RequestIdAlreadyExists.into());
            }

            Self::deposit_event(Event::NewRequest {
                who,
                request_id,
                request,
            });

            T::SchedulerInterface::schedule(request_id, request)?;

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn update_status(request_id: T::RequestId, status: T::Status) -> DispatchResult {
            <RequestsStatus<T>>::insert(request_id, status);

            Self::deposit_event(Event::StatusUpdate { request_id, status });

            todo!()
        }
    }
}
