//! Pallet.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::MaxEncodedLen;

pub use pallet::*;

pub mod traits;
pub mod weights;

#[frame_support::pallet]
pub mod pallet {

    use crate::traits::PrepareTask;

    use super::*;
    use frame_support::pallet_prelude::*;
    use weights::WeightInfo;

    #[pallet::config]
    pub trait Config: frame_system::Config + pallet_worker::Config {
        /// Event type.
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        type RequestId: Parameter + MaxEncodedLen;
        type Request;
        type PrepareTask: PrepareTask<Request = Self::Request, Task = Self::Task>;
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        TaskScheduled {
            worker_id: T::AccountId,
            task: T::Task,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        NoAvailableWorkers,
    }

    impl<T: Config> Pallet<T> {
        // One worker per one request for demo purposes.
        pub fn schedule(request: T::Request) -> DispatchResult {
            let (worker_id, _) = pallet_worker::Workers::<T>::iter()
                .find(|(_, task)| task == &T::Task::default())
                .ok_or(<Error<T>>::NoAvailableWorkers)?;

            let task = T::PrepareTask::prepare_task(request);

            pallet_worker::Pallet::<T>::run_task(worker_id.clone(), task.clone())?;

            Self::deposit_event(Event::TaskScheduled { worker_id, task });

            Ok(())
        }
    }
}
