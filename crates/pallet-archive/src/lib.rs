//! Pallet archive logic.

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
        type ArchiveId: Member
            + Parameter
            + RuntimeAppPublic
            + MaybeSerializeDeserialize
            + MaxEncodedLen;
        type MaxArchives: Get<u32>;
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::storage_version(STORAGE_VERSION)]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    #[pallet::storage]
    #[pallet::getter(fn archives)]
    pub type Archives<T: Config> =
        StorageValue<_, WeakBoundedVec<T::ArchiveId, T::MaxArchives>, ValueQuery>;

    #[pallet::genesis_config]
    pub struct GenesisConfig<T: Config> {
        pub archives: Vec<T::ArchiveId>,
    }

    #[cfg(feature = "std")]
    impl<T: Config> Default for GenesisConfig<T> {
        fn default() -> Self {
            Self {
                archives: Default::default(),
            }
        }
    }

    #[pallet::genesis_build]
    impl<T: Config> GenesisBuild<T> for GenesisConfig<T> {
        fn build(&self) {
            let bounded_archives =
                WeakBoundedVec::<_, T::MaxArchives>::try_from(self.archives.clone())
                    .expect("Initial archives set must be less than T::MaxArchives");
            <Archives<T>>::put(bounded_archives);
        }
    }

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewArchive { archive_id: T::ArchiveId },
    }

    #[pallet::error]
    pub enum Error<T> {
        ArchivesLimitExceeded,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(origin: OriginFor<T>, archive_id: T::ArchiveId) -> DispatchResult {
            let _sender = ensure_signed(origin)?;

            <Archives<T>>::try_mutate::<_, DispatchError, _>(|archives| {
                archives
                    .try_push(archive_id.clone())
                    .map_err(|_| Error::<T>::ArchivesLimitExceeded)?;
                Self::deposit_event(Event::NewArchive { archive_id });
                Ok(())
            })?;
            Ok(())
        }
    }
}
