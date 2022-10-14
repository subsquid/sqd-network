//! Pallet data sources logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_std::{fmt::Debug, prelude::*};

pub use pallet::*;

pub mod weights;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct DataSource;

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
        /// DataSourceId type.
        type DataSourceId: Parameter + MaxEncodedLen;
        /// WeightInfo type that should implement `WeightInfo` trait.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Current data sources state.
    #[pallet::storage]
    #[pallet::getter(fn data_sources)]
    pub type DataSources<T: Config> =
        StorageMap<_, Twox64Concat, T::DataSourceId, DataSource, OptionQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewDataSource { data_source_id: T::DataSourceId },
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Executte register call by WorkerCandidate.
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(_origin: OriginFor<T>, data_source_id: T::DataSourceId) -> DispatchResult {
            Self::deposit_event(Event::NewDataSource { data_source_id });
            todo!()
        }

        /// Announce data source.
        #[pallet::weight(T::WeightInfo::register())]
        pub fn announce_data(_origin: OriginFor<T>, _data_source: DataSource) -> DispatchResult {
            todo!()
        }
    }

    impl<T: Config> Pallet<T> {
        fn _current_data(
            _data_source_id: T::DataSourceId,
        ) -> sp_std::result::Result<DataSource, DispatchError> {
            todo!()
        }
    }
}
