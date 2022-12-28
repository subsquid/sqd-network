//! Pallet workers scheduler logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

pub mod traits;

#[frame_support::pallet]
pub mod pallet {

    use crate::traits::PrepareTask;

    use super::*;
    use frame_support::pallet_prelude::*;

    #[pallet::config]
    pub trait Config: frame_system::Config + pallet_worker::Config {
        /// Event type.
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        /// The request type.
        type Request;
        /// An interface to prepare task for request.
        type PrepareTask: PrepareTask<Request = Self::Request, Task = Self::Task>;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// The task has been scheduled.
        TaskScheduled {
            /// The worker id that should execute the task.
            worker_id: T::AccountId,
            /// The task details.
            task: T::Task,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// No available workers was found.
        NoAvailableWorkers,
    }

    impl<T: Config> Pallet<T> {
        /// An algorithm to schedule the request to worker.
        ///
        /// At current moment we apply simple way just choosing any free available worker.
        /// One request per one worker.
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
