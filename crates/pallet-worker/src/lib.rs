//! Pallet worker logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

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
        type Task: Parameter + MaxEncodedLen + Default;
        type ResultStorage: Parameter + MaxEncodedLen;
        /// WeightInfo type that should implement `WeightInfo` trait.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    #[pallet::storage]
    #[pallet::getter(fn workers)]
    pub type Workers<T: Config> = StorageMap<_, Twox64Concat, T::AccountId, T::Task, OptionQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewWorker {
            worker_id: T::AccountId,
        },
        RunTask {
            worker_id: T::AccountId,
            task: T::Task,
        },
        TaskDone {
            worker_id: T::AccountId,
            task: T::Task,
            result_storage: T::ResultStorage,
        },
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

            <Workers<T>>::insert(who, T::Task::default());

            Ok(())
        }

        #[pallet::weight(T::WeightInfo::done())]
        pub fn done(
            origin: OriginFor<T>,
            task: T::Task,
            result_storage: T::ResultStorage,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let current_task = Self::current_task(who.clone())?;

            if current_task != task {
                return Err(<Error<T>>::NoSubmittedTask.into());
            }

            Self::deposit_event(Event::TaskDone {
                worker_id: who.clone(),
                task,
                result_storage,
            });

            <Workers<T>>::insert(who, T::Task::default());

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn current_task(
            worker_id: T::AccountId,
        ) -> sp_std::result::Result<T::Task, DispatchError> {
            let task = <Workers<T>>::get(worker_id).ok_or(<Error<T>>::NoWorkerId)?;
            Ok(task)
        }

        pub fn run_task(worker_id: T::AccountId, task: T::Task) -> DispatchResult {
            let current_task = Self::current_task(worker_id.clone())?;

            if current_task != T::Task::default() {
                return Err(<Error<T>>::WorkerIsBusy.into());
            }

            Self::deposit_event(Event::RunTask {
                worker_id: worker_id.clone(),
                task: task.clone(),
            });

            <Workers<T>>::insert(worker_id, task);
            Ok(())
        }
    }
}
