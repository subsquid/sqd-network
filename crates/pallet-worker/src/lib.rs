//! Pallet worker logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::MaxEncodedLen;
use frame_support::BoundedVec;
use primitives_networks::Network;
use primitives_worker::{DataAvailability, Task};

pub use pallet::*;

pub mod weights;

/// Expose controller logic to manage workers state.
pub trait WorkerController<WorkerId> {
    /// Get current worker status.
    fn current_status(worker_id: WorkerId) -> (Vec<Task>, Vec<DataAvailability>);
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
        /// Max tasks number.
        type MaxTasks: Get<u32>;
        /// Max da number.
        type MaxDA: Get<u32>;
        /// WeightInfo type that should implement `WeightInfo` trait.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Current workers tasks in `StorageMap` view: `WorkerId` -> `BoundedVec<Tasks>`.
    #[pallet::storage]
    #[pallet::getter(fn workers_tasks)]
    pub type WorkersTasks<T: Config> =
        StorageMap<_, Twox64Concat, T::WorkerId, BoundedVec<Task, T::MaxTasks>, OptionQuery>;

    /// Current workers data availability in `StorageMap` view: `WorkerId` -> `BoundedVec<DA>`.
    #[pallet::storage]
    #[pallet::getter(fn workers_da)]
    pub type WorkersDA<T: Config> = StorageMap<
        _,
        Twox64Concat,
        T::WorkerId,
        BoundedVec<DataAvailability, T::MaxDA>,
        OptionQuery,
    >;

    /// Current networks data availability in `StorageMap` view: `Network` -> `WorkerId`.
    #[pallet::storage]
    #[pallet::getter(fn networks_data)]
    pub type NetworksData<T: Config> =
        StorageMap<_, Twox64Concat, Network, T::WorkerId, OptionQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewWorker { worker_id: T::WorkerId },
        RunTask { task: Task },
        StopTask { task: Task },
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
        pub fn submit_task_result(_origin: OriginFor<T>, _task_result: Task) -> DispatchResult {
            todo!()
        }
    }
}

impl<T: Config> WorkerController<T::WorkerId> for Pallet<T> {
    fn current_status(_worker_id: T::WorkerId) -> (Vec<Task>, Vec<DataAvailability>) {
        todo!()
    }

    fn run_task(_worker_id: T::WorkerId, _task: Task) {
        todo!()
    }

    fn stop_task(_worker_id: T::WorkerId, _task: Task) {
        todo!()
    }
}
