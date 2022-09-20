//! Pallet worker logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use frame_support::{traits::StorageVersion, WeakBoundedVec};
use sp_runtime::RuntimeAppPublic;
use sp_std::prelude::*;

pub use pallet::*;

#[cfg(test)]
mod tests;
pub mod weights;

const STORAGE_VERSION: StorageVersion = StorageVersion::new(0);

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;
    use weights::WeightInfo;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;
        type WorkerId: Member
            + Parameter
            + RuntimeAppPublic
            + MaybeSerializeDeserialize
            + MaxEncodedLen;
        type MaxWorkers: Get<u32>;
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    #[pallet::storage]
    #[pallet::getter(fn workers)]
    pub type Workers<T: Config> =
        StorageValue<_, WeakBoundedVec<T::WorkerId, T::MaxWorkers>, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub workers: Vec<T::WorkerId>,
    }

    #[cfg(feature = "std")]
    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                workers: Default::default(),
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
        fn build(&self) {
            let bounded_workers =
                WeakBoundedVec::<_, T::MaxWorkers>::try_from(self.workers.clone())
                    .expect("Initial workers set must be less than T::MaxWorkers");
            <Workers<T>>::put(bounded_workers);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewWorker { worker_id: T::WorkerId },
    }

    #[pallet::error]
    pub enum Error<T> {
        WorkersLimitExceeded,
        WorkerAlreadyRegistered,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(origin: OriginFor<T>, worker_id: T::WorkerId) -> DispatchResult {
            let _sender = ensure_signed(origin)?;

            if Self::is_registered(&worker_id) {
                return Err(DispatchError::from(Error::<T>::WorkerAlreadyRegistered));
            }

            <Workers<T>>::try_mutate::<_, DispatchError, _>(|workers| {
                workers
                    .try_push(worker_id.clone())
                    .map_err(|_| Error::<T>::WorkersLimitExceeded)?;
                Self::deposit_event(Event::NewWorker { worker_id });
                Ok(())
            })?;
            Ok(())
        }
    }

    impl<T: Config> Pallet<T> {
        pub fn is_registered(worker_id: &T::WorkerId) -> bool {
            Workers::<T>::get().iter().any(|id| id == worker_id)
        }
    }
}
