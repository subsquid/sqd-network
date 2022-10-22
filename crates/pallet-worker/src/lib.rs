//! Pallet worker logic.

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
    use frame_system::pallet_prelude::*;
    use weights::WeightInfo;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Event type.
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        /// WorkerId type.
        type WorkerId: Parameter + MaxEncodedLen + Copy;
        /// WeightInfo type that should implement `WeightInfo` trait.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Current workers state in `StorageMap` view: `WorkerId` -> `Status`.
    #[pallet::storage]
    #[pallet::getter(fn workers)]
    pub type Workers<T: Config> = StorageMap<_, Twox64Concat, T::WorkerId, Status, OptionQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewWorker { worker_id: T::WorkerId },
        RunTask { task: Task },
    }

    #[pallet::error]
    pub enum Error<T> {
        NoWorkerId,
        NoAssignedTask,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Executte register call by WorkerCandidate.
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(origin: OriginFor<T>, worker_id: T::WorkerId) -> DispatchResult {
            let _ = ensure_signed(origin);

            Self::deposit_event(Event::NewWorker { worker_id });

            <Workers<T>>::insert(worker_id, Status::Ready);

            Ok(())
        }

        #[pallet::weight(T::WeightInfo::register())]
        pub fn done(_origin: OriginFor<T>, worker_id: T::WorkerId) -> DispatchResult {
            let current_status = Self::current_status(worker_id)?;

            if current_status == Status::Ready {
                return Err(<Error<T>>::NoAssignedTask.into());
            }

            <Workers<T>>::insert(worker_id, Status::Ready);

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn current_status(
            worker_id: T::WorkerId,
        ) -> sp_std::result::Result<Status, DispatchError> {
            let status = <Workers<T>>::get(worker_id).ok_or(<Error<T>>::NoWorkerId)?;
            Ok(status)
        }

        pub fn run_task(worker_id: T::WorkerId, task: Task) -> DispatchResult {
            <Workers<T>>::insert(worker_id, Status::Run(task));

            Ok(())
        }
    }
}
