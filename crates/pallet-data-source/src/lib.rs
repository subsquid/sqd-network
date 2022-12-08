//! Pallet data sources logic.

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
        /// Data source info representation in the network.
        type DataSource: Parameter + MaxEncodedLen;
        /// The weight information provider type.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// The list of data sources registered in the network.
    #[pallet::storage]
    #[pallet::getter(fn data_sources)]
    pub type DataSources<T: Config> =
        StorageMap<_, Twox64Concat, T::AccountId, T::DataSource, OptionQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// The new data source has been registered in the network.
        NewDataSource {
            /// An account that owns data source.
            owner: T::AccountId,
            /// Data source detailed information.
            data_source: T::DataSource,
        },
        /// The current data source information has been updated.
        DataSourceInfoUpdate {
            /// An account that has checked data source current state.
            who: T::AccountId,
            /// Data source owner.
            owner: T::AccountId,
            /// Updated data source information.
            data_source: T::DataSource,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        /// Data source already registered in the network.
        DataSourceAlreadyRegistered,
        /// No data source was found.
        NoDataSource,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(T::WeightInfo::register())]
        /// Register the data source with owner and data source information itself.
        pub fn register(origin: OriginFor<T>, data_source: T::DataSource) -> DispatchResult {
            let who = ensure_signed(origin)?;

            if <DataSources<T>>::contains_key(&who) {
                return Err(<Error<T>>::DataSourceAlreadyRegistered.into());
            }

            <DataSources<T>>::insert(who.clone(), data_source.clone());

            Self::deposit_event(Event::NewDataSource {
                owner: who,
                data_source,
            });
            Ok(())
        }

        #[pallet::weight(T::WeightInfo::update_info())]
        /// Update data source information.
        ///
        /// TODO. Data info can be updated only by workers.
        pub fn update_data_source_info(
            origin: OriginFor<T>,
            owner: T::AccountId,
            data_source: T::DataSource,
        ) -> DispatchResult {
            let who = ensure_signed(origin)?;

            if !<DataSources<T>>::contains_key(&owner) {
                return Err(<Error<T>>::NoDataSource.into());
            }

            <DataSources<T>>::insert(owner.clone(), data_source.clone());

            Self::deposit_event(Event::DataSourceInfoUpdate {
                who,
                owner,
                data_source,
            });
            Ok(())
        }
    }
}
