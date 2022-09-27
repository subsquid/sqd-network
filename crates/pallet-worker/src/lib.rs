//! Pallet worker logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::BoundedVec;
use scale_info::TypeInfo;
use sp_std::{fmt::Debug, prelude::*};

pub use pallet::*;

pub mod weights;

/// Possible task statuses that is used to manage workers usage.
#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum Status<Task> {
    Ready,
    Waiting(Task),
    Run(Task),
    Terminating(Task),
}

/// Expose controller logic to manage workers state.
pub trait WorkerController<WorkerId, Task> {
    /// Get current worker status.
    fn current_status(worker_id: WorkerId);
    /// Run the task for particular worker.
    fn run_task(worker_id: WorkerId, task: Task);
    /// Stop the task for particular worker.
    fn stop_task(worker_id: WorkerId, task: Task);
}

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
        type WorkerId: Parameter + MaxEncodedLen;
        /// Max workers number.
        type MaxWorkers: Get<u32>;
        /// Task type.
        type Task: Parameter + MaxEncodedLen;
        /// Max tasks number.
        type MaxTasks: Get<u32>;
        /// WeightInfo type that should implement `WeightInfo` trait.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Current workers state in `StorageMap` view: `WorkerId` -> `Status`.
    #[pallet::storage]
    #[pallet::getter(fn workers)]
    pub type Workers<T: Config> = StorageMap<
        _,
        Twox64Concat,
        T::WorkerId,
        BoundedVec<Status<T::Task>, T::MaxTasks>,
        OptionQuery,
    >;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewWorker { worker_id: T::WorkerId },
        RunTask { task: T::Task },
        StopTask { task: T::Task },
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Executte register call by WorkerCandidate.
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(_origin: OriginFor<T>, worker_id: T::WorkerId) -> DispatchResult {
            Self::deposit_event(Event::NewWorker { worker_id });
            todo!()
        }

        /// Execute submit_task_result call by Worker.
        #[pallet::weight(T::WeightInfo::register())]
        pub fn submit_task_result(_origin: OriginFor<T>, _task_result: T::Task) -> DispatchResult {
            todo!()
        }
    }
}

impl<T: Config> WorkerController<T::WorkerId, T::Task> for Pallet<T> {
    fn current_status(_worker_id: T::WorkerId) {
        todo!()
    }

    fn run_task(_worker_id: T::WorkerId, _task: T::Task) {
        todo!()
    }

    fn stop_task(_worker_id: T::WorkerId, _task: T::Task) {
        todo!()
    }
}
