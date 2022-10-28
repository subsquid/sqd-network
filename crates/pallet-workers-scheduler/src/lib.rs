//! Pallet.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::MaxEncodedLen;
use primitives_worker::{Status, Task};

pub use pallet::*;

pub mod traits;
pub mod weights;

#[frame_support::pallet]
pub mod pallet {

    use crate::traits::IsDataSourceSuit;

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
        type IsDataSourceSuit: IsDataSourceSuit<Request = Self::Request>;
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
        pub fn _schedule(_request_id: T::RequestId, request: T::Request) -> DispatchResult {
            let (worker_id, _) = pallet_worker::Workers::<T>::iter()
                .find(|(_, status)| status == &Status::Ready)
                .ok_or(<Error<T>>::NoAvailableWorkers)?;

            let (data_source_id, _) = pallet_data_source::DataSources::<T>::iter()
                .find(|(_, data)| T::IsDataSourceSuit::is_suit(data, &request))
                .ok_or(<Error<T>>::NoRequiredDataSource)?;

            let task = Task {
                docker_image: primitives_worker::DockerImage::SubstrateWorker,
                command: primitives_worker::Command::Run,
                result_storage: primitives_worker::ResultStorage::IPFS,
            };

            Self::deposit_event(Event::TaskScheduled { worker_id, task });

            Ok(())
        }
    }
}
