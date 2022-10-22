//! Pallet data sources logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode, MaxEncodedLen};
use scale_info::TypeInfo;
use sp_std::{fmt::Debug, prelude::*};

pub use pallet::*;

pub mod weights;

#[derive(PartialEq, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub struct Data {
    pub from: u32,
    pub to: u32,
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
        /// DataSourceId type.
        type DataSourceId: Parameter + MaxEncodedLen + Copy;
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
        StorageMap<_, Twox64Concat, T::DataSourceId, Data, OptionQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewDataSource {
            who: T::AccountId,
            id: T::DataSourceId,
        },
        DataAnnounced {
            id: T::DataSourceId,
            data: Data,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        DataSourceAlreadyRegistered,
        NoDataSource,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(origin: OriginFor<T>, id: T::DataSourceId) -> DispatchResult {
            let who = ensure_signed(origin)?;

            if <DataSources<T>>::contains_key(id) {
                return Err(<Error<T>>::DataSourceAlreadyRegistered.into());
            }

            <DataSources<T>>::insert(id, Data { from: 0, to: 0 });

            Self::deposit_event(Event::NewDataSource { who, id });
            Ok(())
        }

        #[pallet::weight(T::WeightInfo::register())]
        pub fn announce_data(
            _origin: OriginFor<T>,
            id: T::DataSourceId,
            data: Data,
        ) -> DispatchResult {
            if !<DataSources<T>>::contains_key(id) {
                return Err(<Error<T>>::NoDataSource.into());
            }

            <DataSources<T>>::insert(id, data.clone());
            Self::deposit_event(Event::DataAnnounced { id, data });
            Ok(())
        }
    }
}
