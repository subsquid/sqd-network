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
        type DataSource: Parameter + MaxEncodedLen;
        /// WeightInfo type that should implement `WeightInfo` trait.
        type WeightInfo: WeightInfo;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    #[pallet::storage]
    #[pallet::getter(fn data_sources)]
    pub type DataSources<T: Config> =
        StorageMap<_, Twox64Concat, T::AccountId, T::DataSource, OptionQuery>;

    /// Possible events list.
    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        NewDataSource {
            who: T::AccountId,
            data_source: T::DataSource,
        },
        DataAnnounced {
            who: T::AccountId,
            data_source: T::DataSource,
        },
    }

    #[pallet::error]
    pub enum Error<T> {
        DataSourceAlreadyRegistered,
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        #[pallet::weight(T::WeightInfo::register())]
        pub fn register(origin: OriginFor<T>, data_source: T::DataSource) -> DispatchResult {
            let who = ensure_signed(origin)?;

            if <DataSources<T>>::contains_key(&who) {
                return Err(<Error<T>>::DataSourceAlreadyRegistered.into());
            }

            <DataSources<T>>::insert(who.clone(), data_source.clone());

            Self::deposit_event(Event::NewDataSource { who, data_source });
            Ok(())
        }
    }
}
