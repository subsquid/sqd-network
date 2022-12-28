//! Pallet external requests logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;

pub use pallet::*;

pub mod traits;
pub mod weights;

/// Requests that are supported in the network.
#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum Request<NativeEthRequest, NativeSubstrateRequest, SubstrateEvmRequest> {
    /// Native ethereum requests.
    NativeEthRequest(NativeEthRequest),
    /// Native substrate requests.
    NativeSubstrateRequest(NativeSubstrateRequest),
    /// Substrate evm specific requests.
    SubstrateEvmRequest(SubstrateEvmRequest),
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
        /// Request status type.
        type Status: Parameter + MaxEncodedLen;
        /// The struct to process native ethereum requests.
        type NativeEthRequest: Parameter + MaxEncodedLen;
        /// The struct to process native substrate requests.
        type NativeSubstrateRequest: Parameter + MaxEncodedLen;
        /// The struct to process native substrate evm specific requests.
        type SubstrateEvmRequest: Parameter + MaxEncodedLen;
        /// An request id type.
        type RequestId: Parameter + MaxEncodedLen + Copy;
        /// An interface to calculate the request id.
        type RequestIdGenerator: RequestIdGenerator<
            Id = Self::RequestId,
            Request = Request<
                Self::NativeEthRequest,
                Self::NativeSubstrateRequest,
                Self::SubstrateEvmRequest,
            >,
        >;
        /// An interface to send request to the schedure.
        type SchedulerInterface: SchedulerInterface<
            RequestId = Self::RequestId,
            Request = Request<
                Self::NativeEthRequest,
                Self::NativeSubstrateRequest,
                Self::SubstrateEvmRequest,
            >,
            Status = Self::Status,
        >;
        /// The weight information provider type.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Requests to non-native blockchain with their statuses.
    #[pallet::storage]
    #[pallet::getter(fn requests)]
    pub type Requests<T: Config> = StorageMap<
        _,
        Twox64Concat,
        T::RequestId,
        (
            T::Status,
            Request<T::NativeEthRequest, T::NativeSubstrateRequest, T::SubstrateEvmRequest>,
        ),
        OptionQuery,
    >;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// The new request has been submitted.
        NewRequest {
            /// An account that has submitted the requeest.
            who: T::AccountId,
            /// Request id.
            request_id: T::RequestId,
            /// The request information itself.
            request:
                Request<T::NativeEthRequest, T::NativeSubstrateRequest, T::SubstrateEvmRequest>,
        },
        /// The request status has been updated.
        StatusUpdate {
            /// Request id.
            request_id: T::RequestId,
            /// Updated status.
            status: T::Status,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Request already exists.
        RequestIdAlreadyExists,
        /// No request was found based on provided id.
        NoRequestId,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(T::WeightInfo::request())]
        /// Process native ethereum related requests.
        pub fn native_eth_request(
            origin: OriginFor<T>,
            request: T::NativeEthRequest,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let request = Request::NativeEthRequest(request);

            Self::process(who, request)
        }

        /// Process native substrate related requests.
        #[pallet::weight(T::WeightInfo::request())]
        pub fn native_substrate_request(
            origin: OriginFor<T>,
            request: T::NativeSubstrateRequest,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let request = Request::NativeSubstrateRequest(request);

            Self::process(who, request)
        }

        /// Process substrate evm related requests.
        #[pallet::weight(T::WeightInfo::request())]
        pub fn substrate_evm_request(
            origin: OriginFor<T>,
            request: T::SubstrateEvmRequest,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;
            let request = Request::SubstrateEvmRequest(request);

            Self::process(who, request)
        }
    }

    impl<T: Config> Pallet<T> {
        /// Process an incoming request.
        fn process(
            who: T::AccountId,
            request: Request<
                T::NativeEthRequest,
                T::NativeSubstrateRequest,
                T::SubstrateEvmRequest,
            >,
        ) -> DispatchResult {
            let request_id = T::RequestIdGenerator::generate_id(request.clone());

            if <Requests<T>>::contains_key(request_id) {
                return Err(<Error<T>>::RequestIdAlreadyExists.into());
            }

            let status = T::SchedulerInterface::schedule(request_id, request.clone())?;

            Self::deposit_event(Event::StatusUpdate {
                request_id,
                status: status.clone(),
            });

            <Requests<T>>::insert(request_id, (status, request.clone()));

            Self::deposit_event(Event::NewRequest {
                who,
                request_id,
                request,
            });

            Ok(())
        }

        pub fn update_status(request_id: T::RequestId, status: T::Status) -> DispatchResult {
            match <Requests<T>>::get(request_id) {
                Some((_, request)) => <Requests<T>>::insert(request_id, (status.clone(), request)),
                None => return Err(<Error<T>>::NoRequestId.into()),
            }

            Self::deposit_event(Event::StatusUpdate { request_id, status });

            Ok(())
        }
    }
}
