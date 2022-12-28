//! Pallet vesting logic.

// Ensure we're `no_std` when compiling for Wasm.
#![cfg_attr(not(feature = "std"), no_std)]

use codec::{Decode, Encode, MaxEncodedLen};
use frame_support::traits::{Currency, LockIdentifier, LockableCurrency, WithdrawReasons};
use scale_info::TypeInfo;

pub use pallet::*;
pub use weights::WeightInfo;

pub mod weights;

#[cfg(test)]
mod mock;
#[cfg(test)]
mod tests;

type BalanceOf<T> =
    <<T as Config>::Currency as Currency<<T as frame_system::Config>::AccountId>>::Balance;

const VESTING_ID: LockIdentifier = *b"vesting ";

/// Struct to encode the vesting schedule of an individual account.
#[derive(Encode, Decode, Debug, Copy, Clone, PartialEq, Eq, MaxEncodedLen, TypeInfo)]
pub struct VestingInfo<Balance, VestingTime> {
    /// Locked amount at genesis.
    locked: Balance,
    /// Cliff.
    cliff: VestingTime,
    /// Duration.
    duration: VestingTime,
}

#[frame_support::pallet]
pub mod pallet {
    use super::*;
    use frame_support::{
        pallet_prelude::*, storage::with_storage_layer, traits::ExistenceRequirement,
    };
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

        /// The vesting time representation.
        type VestingTime: Parameter + MaxEncodedLen;

        /// Weight information for extrinsics in this pallet.
        type WeightInfo: WeightInfo;

        /// Reasons that determine under which conditions the balance may drop below
        /// the unvested amount.
        type UnvestedFundsAllowedWithdrawReasons: Get<WithdrawReasons>;
    }

    #[pallet::pallet]
    #[pallet::generate_store(pub(super) trait Store)]
    pub struct Pallet<T>(_);

    /// Information regarding the vesting of a given account.
    #[pallet::storage]
    #[pallet::getter(fn vesting)]
    pub type Vestings<T: Config> =
        StorageMap<_, Blake2_128Concat, T::AccountId, VestingInfo<BalanceOf<T>, T::VestingTime>>;

    #[pallet::event]
    #[pallet::generate_deposit(pub(super) fn deposit_event)]
    pub enum Event<T: Config> {
        /// Full unlocked.
        VestingCompleted { account: T::AccountId },
        /// The vesting was removed by sudo account.
        VestingRemoved { account: T::AccountId },
    }

    /// Error for the vesting pallet.
    #[pallet::error]
    pub enum Error<T> {
        /// The account given is not vesting.
        NotVesting,
        /// Amount being transferred is too low to create a vesting schedule.
        AmountLow,
        /// The account used to create vesting is already used.
        AccountAlreadyHasVesting,
        /// There is no vesting.
        NoVesting,
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

            with_storage_layer(|| {
                // TODO: Add calculation to check the part of locked balance that should be unlocked.
                let _vest_info = <Vestings<T>>::get(&who).ok_or(<Error<T>>::NoVesting)?;

                T::Currency::remove_lock(VESTING_ID, &who);

                <Vestings<T>>::remove(&who);

                Self::deposit_event(Event::<T>::VestingCompleted { account: who });

                Ok(())
            })
        }

        /// Unlock any vested funds of a `target` account.
        ///
        /// The dispatch origin for this call must be _Signed_.
        ///
        /// - `target`: The account whose vested funds should be unlocked. Must have funds still
        /// locked under this pallet.
        #[pallet::weight(T::WeightInfo::vest_locked())]
        pub fn vest_other(origin: OriginFor<T>, target: T::AccountId) -> DispatchResult {
            ensure_signed(origin)?;

            with_storage_layer(|| {
                // TODO: Add calculation to check the part of locked balance that should be unlocked.
                let _vest_info = <Vestings<T>>::get(&target).ok_or(<Error<T>>::NoVesting)?;

                T::Currency::remove_lock(VESTING_ID, &target);

                <Vestings<T>>::remove(&target);

                Self::deposit_event(Event::<T>::VestingCompleted { account: target });

                Ok(())
            })
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
            target: T::AccountId,
            schedule: VestingInfo<BalanceOf<T>, T::VestingTime>,
        ) -> DispatchResult {
            let transactor = ensure_signed(origin)?;

            ensure!(
                schedule.locked >= T::MinVestedTransfer::get(),
                Error::<T>::AmountLow
            );

            if <Vestings<T>>::contains_key(&target) {
                return Err(<Error<T>>::AccountAlreadyHasVesting.into());
            }

            with_storage_layer(move || {
                T::Currency::transfer(
                    &transactor,
                    &target,
                    schedule.locked,
                    ExistenceRequirement::AllowDeath,
                )?;

                let reasons =
                    WithdrawReasons::except(T::UnvestedFundsAllowedWithdrawReasons::get());

                T::Currency::set_lock(VESTING_ID, &target, schedule.locked, reasons);

                <Vestings<T>>::insert(target, schedule);

                Ok(())
            })
        }

        /// Remove vesting.
        #[pallet::weight(T::WeightInfo::vest_locked())]
        pub fn remove_vest(
            origin: OriginFor<T>,
            target: T::AccountId,
            send_to: T::AccountId,
        ) -> DispatchResult {
            ensure_root(origin)?;

            let vest_info = <Vestings<T>>::get(&target).ok_or(<Error<T>>::NoVesting)?;

            with_storage_layer(move || {
                T::Currency::remove_lock(VESTING_ID, &target);

                // TODO. Send only the rest part.
                T::Currency::transfer(
                    &target,
                    &send_to,
                    vest_info.locked,
                    ExistenceRequirement::AllowDeath,
                )?;

                <Vestings<T>>::remove(&target);

                Self::deposit_event(Event::<T>::VestingRemoved { account: target });

                Ok(())
            })
        }
    }
}
