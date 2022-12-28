use frame_support::assert_ok;

use crate::{mock::*, *};

const NEW_VESTING_ACCOUNT: u64 = 10;

#[test]
fn vested_transfer_works() {
    new_test_ext().execute_with(|| {
        assert!(!<Vestings<Test>>::contains_key(NEW_VESTING_ACCOUNT));
        assert_eq!(Balances::free_balance(42), 1000);

        let new_vesting_info = VestingInfo {
            locked: 100,
            cliff: 10,
            duration: 10,
        };

        assert_ok!(Vesting::vested_transfer(
            Origin::signed(42),
            NEW_VESTING_ACCOUNT,
            new_vesting_info
        ));

        assert_eq!(Balances::free_balance(&42), 900);
        assert_eq!(Balances::free_balance(&NEW_VESTING_ACCOUNT), 100);
        assert_eq!(Balances::usable_balance(&NEW_VESTING_ACCOUNT), 0);
        assert_eq!(
            <Vestings<Test>>::get(&NEW_VESTING_ACCOUNT).unwrap(),
            new_vesting_info
        );
    });
}
