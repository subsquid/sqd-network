//! Pallet worker logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

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
        /// WeightInfo type that should implement `WeightInfo` trait.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    #[pallet::storage]
    #[pallet::getter(fn workers)]
    pub type Workers<T: Config> = StorageMap<_, Twox64Concat, T::AccountId, Status, OptionQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewWorker { worker_id: T::AccountId },
        RunTask { worker_id: T::AccountId, task: Task },
        TaskDone { worker_id: T::AccountId, task: Task },
    }

    #[pallet::error]
    pub enum Error<T> {
        NoWorkerId,
        NoSubmittedTask,
        WorkerIsBusy,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            Self::deposit_event(Event::NewWorker {
                worker_id: who.clone(),
            });

            <Workers<T>>::insert(who, Status::Ready);

            Ok(())
        }

        #[pallet::weight(T::WeightInfo::done())]
        pub fn done(origin: OriginFor<T>, task: Task) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let current_status = Self::current_status(who.clone())?;

            if current_status != Status::Run(task.clone()) {
                return Err(<Error<T>>::NoSubmittedTask.into());
            }

            Self::deposit_event(Event::TaskDone {
                worker_id: who.clone(),
                task,
            });

            <Workers<T>>::insert(who, Status::Ready);

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn current_status(
            worker_id: T::AccountId,
        ) -> sp_std::result::Result<Status, DispatchError> {
            let status = <Workers<T>>::get(worker_id).ok_or(<Error<T>>::NoWorkerId)?;
            Ok(status)
        }

        pub fn run_task(worker_id: T::AccountId, task: Task) -> DispatchResult {
            let current_status = Self::current_status(worker_id.clone())?;

            if current_status != Status::Ready {
                return Err(<Error<T>>::WorkerIsBusy.into());
            }

            Self::deposit_event(Event::RunTask {
                worker_id: worker_id.clone(),
                task: task.clone(),
            });

            <Workers<T>>::insert(worker_id, Status::Run(task));
            Ok(())
        }
    }
}
