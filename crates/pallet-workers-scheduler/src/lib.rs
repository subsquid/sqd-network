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

    use crate::traits::{DataSourceController, WorkerController};

    use super::*;
    use frame_support::pallet_prelude::*;
    use weights::WeightInfo;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// Event type.
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        type RequestId: Parameter + MaxEncodedLen;
        type Request;
        type WorkerId: Parameter + MaxEncodedLen;
        type WorkerController: WorkerController<
            WorkerId = Self::WorkerId,
            Task = Task,
            Status = Status,
        >;
        type DataSourceId: Parameter + MaxEncodedLen;
        type DataSource;
        type DataSourceController: DataSourceController<
            Id = Self::DataSourceId,
            Data = Self::DataSource,
        >;
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

    impl<T: Config> Pallet<T> {
        fn _schedule(_request_id: T::RequestId, _request: T::Request) -> DispatchResult {
            // Analyze current workers status and available data sources
            // and decide which task is going to be applied.

            // let scheduled_task =
            // Self::deposit_event(Event::TaskScheduled { worker_id, task: scheduled_task });

            todo!()
        }
    }
}
