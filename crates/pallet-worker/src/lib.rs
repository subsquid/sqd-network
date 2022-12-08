//! Pallet worker logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

pub use pallet::*;

pub mod traits;
pub mod weights;

#[frame_support::pallet]
pub mod pallet {
    use crate::traits::UpdateRequestStatus;

    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;
    use weights::WeightInfo;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Event type.
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        /// Worker task type.
        type Task: Parameter + MaxEncodedLen + Default;
        /// Task result type.
        type Result: Parameter + MaxEncodedLen;
        /// The type that to update task related request.
        type UpdateRequestStatus: UpdateRequestStatus<Task = Self::Task, Result = Self::Result>;
        /// The weight information provider type.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// The list of available workers in the network.
    #[pallet::storage]
    #[pallet::getter(fn workers)]
    pub type Workers<T: Config> = StorageMap<_, Twox64Concat, T::AccountId, T::Task, OptionQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// New worker has been registered in the network.
        NewWorker {
            /// The worker id.
            worker_id: T::AccountId,
        },
        /// The new task has been assigned for worker.
        RunTask {
            /// Worker id the task has been assigned to.
            worker_id: T::AccountId,
            /// The task that has been assigned.
            task: T::Task,
        },
        /// The task has been done.
        TaskDone {
            /// The worker of finished task.
            worker_id: T::AccountId,
            /// The task information.
            task: T::Task,
            /// The task result.
            result: T::Result,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Worker already registered.
        WorkerAlreadyRegistered,
        /// No worker was found.
        NoWorkerId,
        /// No submitted task was found.
        NoSubmittedTask,
        /// The worker is busy with another task.
        WorkerIsBusy,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(T::WeightInfo::register())]
        /// Register the worker in the network.
        pub fn register(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;

            if <Workers<T>>::contains_key(&who) {
                return Err(<Error<T>>::WorkerAlreadyRegistered.into());
            }

            Self::deposit_event(Event::NewWorker {
                worker_id: who.clone(),
            });

            <Workers<T>>::insert(who, T::Task::default());

            Ok(())
        }

        #[pallet::weight(T::WeightInfo::done())]
        /// Submit the task result.
        pub fn done(origin: OriginFor<T>, task: T::Task, result: T::Result) -> DispatchResult {
            let who = ensure_signed(origin)?;

            let current_task = Self::current_task(who.clone())?;

            if current_task != task {
                return Err(<Error<T>>::NoSubmittedTask.into());
            }

            T::UpdateRequestStatus::update_request_status(task.clone(), result.clone())?;

            Self::deposit_event(Event::TaskDone {
                worker_id: who.clone(),
                task,
                result,
            });

            <Workers<T>>::insert(who, T::Task::default());

            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        /// A helper function to get current task of the worker.
        fn current_task(worker_id: T::AccountId) -> sp_std::result::Result<T::Task, DispatchError> {
            let task = <Workers<T>>::get(worker_id).ok_or(<Error<T>>::NoWorkerId)?;
            Ok(task)
        }

        /// Assign the task to particular worker.
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
