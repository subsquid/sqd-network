//! Pallet.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::MaxEncodedLen;
use primitives_worker::{Status, Task};

pub use pallet::*;

pub mod weights;

#[frame_support::pallet]
pub mod pallet {

    use super::*;
    use frame_support::pallet_prelude::*;
    use weights::WeightInfo;

    #[pallet::config]
    pub trait Config:
        frame_system::Config + pallet_worker::Config + pallet_data_source::Config
    {
        /// Event type.
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        type RequestId: Parameter + MaxEncodedLen;
        type Request;
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        TaskScheduled { worker_id: T::WorkerId, task: Task },
    }

    #[pallet::error]
    pub enum Error<T> {
        NoAvailableWorkers,
        NoRequiredDataSource,
    }

    impl<T: Config> Pallet<T> {
        pub fn _schedule(_request_id: T::RequestId, _request: T::Request) -> DispatchResult {
            // let worker_id = pallet_worker::Workers::iter if Ready take it.

            // let data_source = pallet_data_source iter if the same take it.

            // let task =

            // Self::deposit_event(Event::TaskScheduled { worker_id, task: scheduled_task });

            Ok(())
        }
    }
}
