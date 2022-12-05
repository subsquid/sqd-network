//! Pallet vesting logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::traits::{Currency, LockableCurrency, WithdrawReasons};
use scale_info::TypeInfo;
use sp_runtime::traits::StaticLookup;

pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

type BalanceOf<T> =
    <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

type AccountIdLookupOf<T> = <<T as frame_system::Config>::Lookup as StaticLookup>::Source;

/// Struct to encode the vesting schedule of an individual account.
#[derive(Encode, Decode, Debug, Copy, Clone, PartialEq, Eq, MaxEncodedLen, TypeInfo)]
pub struct VestingInfo<Balance, BlockNumber> {
    /// Locked amount at genesis.
    locked: Balance,
    /// Amount that gets unlocked every block after `starting_block`.
    per_block: Balance,
    /// Starting block for unlocking(vesting).
    starting_block: BlockNumber,
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::pallet_prelude::*;
    use frame_system::pallet_prelude::*;

    #[pallet::config]
    pub trait Config: frame_system::Config {
        /// The overarching event type.
        type Event: From<Event<Self>> + IsType<<Self as frame_system::Config>::Event>;

        /// The currency trait.
        type Currency: LockableCurrency<Self::AccountId>;

        /// The minimum amount transferred to call `vested_transfer`.
        #[pallet::constant]
        type MinVestedTransfer: Get<BalanceOf<Self>>;

        /// Weight information for extrinsics in this pallet.
        type WeightInfo: WeightInfo;

        /// Reasons that determine under which conditions the balance may drop below
        /// the unvested amount.
        type UnvestedFundsAllowedWithdrawReasons: Get<WithdrawReasons>;

        /// Maximum number of vesting schedules an account may have at a given moment.
        const MAX_VESTING_SCHEDULES: u32;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Information regarding the vesting of a given account.
    #[pallet::storage]
    #[pallet::getter(fn vesting)]
    pub type Vesting<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, VestingInfo<BalanceOf<T>, T::BlockNumber>>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// An \[account\] has become fully vested.
        VestingCompleted { account: T::AccountId },
    }

    #[pallet::call]
    impl<T: Config> Pallet<T> {
        /// Unlock any vested funds of the sender account.
        ///
        /// The dispatch origin for this call must be _Signed_ and the sender must have funds still
        /// locked under this pallet.
        #[pallet::weight(T::WeightInfo::vest_locked())]
        pub fn vest(origin: OriginFor<T>) -> DispatchResult {
            let who = ensure_signed(origin)?;
            todo!()
        }

        /// Unlock any vested funds of a `target` account.
        ///
        /// The dispatch origin for this call must be _Signed_.
        ///
        /// - `target`: The account whose vested funds should be unlocked. Must have funds still
        /// locked under this pallet.
        #[pallet::weight(T::WeightInfo::vest_locked())]
        pub fn vest_other(origin: OriginFor<T>, target: AccountIdLookupOf<T>) -> DispatchResult {
            ensure_signed(origin)?;
            let who = T::Lookup::lookup(target)?;
            todo!()
        }

        /// Create a vested transfer.
        ///
        /// The dispatch origin for this call must be _Signed_.
        ///
        /// - `target`: The account receiving the vested funds.
        /// - `schedule`: The vesting schedule attached to the transfer.
        ///
        /// Emits `VestingCreated`.
        #[pallet::weight(T::WeightInfo::vest_locked())]
        pub fn vested_transfer(
            origin: OriginFor<T>,
            target: AccountIdLookupOf<T>,
            schedule: VestingInfo<BalanceOf<T>, T::BlockNumber>,
        ) -> DispatchResult {
            let transactor = ensure_signed(origin)?;
            let transactor = <T::Lookup as StaticLookup>::unlookup(transactor);
            todo!()
        }

        /// Force a vested transfer.
        ///
        /// The dispatch origin for this call must be _Root_.
        ///
        /// - `source`: The account whose funds should be transferred.
        /// - `target`: The account that should be transferred the vested funds.
        /// - `schedule`: The vesting schedule attached to the transfer.
        ///
        /// Emits `VestingCreated`.
        #[pallet::weight(T::WeightInfo::vest_locked())]
        pub fn force_vested_transfer(
            origin: OriginFor<T>,
            source: AccountIdLookupOf<T>,
            target: AccountIdLookupOf<T>,
            schedule: VestingInfo<BalanceOf<T>, T::BlockNumber>,
        ) -> DispatchResult {
            ensure_root(origin)?;
            todo!()
        }

        /// Remove vesting (sudo or owner)?
        #[pallet::weight(T::WeightInfo::vest_locked())]
        pub fn remove_vest(origin: OriginFor<T>, target: AccountIdLookupOf<T>) -> DispatchResult {
            ensure_root(origin)?;
            todo!()
        }
    }
}
