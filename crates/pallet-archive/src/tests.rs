//! Tests.

use crate::{self as pallet_archive, *};
use frame_support::{
    assert_noop, assert_ok,
    traits::{ConstU32, ConstU64},
};
use sp_core::H256;
use sp_runtime::{
    testing::{Header, UintAuthorityId},
    traits::{BlakeTwo256, IdentityLookup},
    BuildStorage,
};

type UncheckedExtrinsic = frame_system::mocking::MockUncheckedExtrinsic<Test>;
type Block = frame_system::mocking::MockBlock<Test>;

const MAX_ARCHIVES: u32 = 100;

frame_support::construct_runtime!(
    pub enum Test where
        Block = Block,
        NodeBlock = Block,
        UncheckedExtrinsic = UncheckedExtrinsic,
    {
        System: frame_system::{Pallet, Call, Config, Storage, Event<T>},
        Archive: pallet_archive::{Pallet, Call, Storage, Config<T>, Event<T>},
    }
);

impl frame_system::Config for Test {
    type BaseCallFilter = frame_support::traits::Everything;
    type BlockWeights = ();
    type BlockLength = ();
    type DbWeight = ();
    type Origin = Origin;
    type Index = u64;
    type BlockNumber = u64;
    type Hash = H256;
    type Call = Call;
    type Hashing = BlakeTwo256;
    type AccountId = u64;
    type Lookup = IdentityLookup<Self::AccountId>;
    type Header = Header;
    type Event = Event;
    type BlockHashCount = ConstU64<250>;
    type Version = ();
    type PalletInfo = PalletInfo;
    type AccountData = ();
    type OnNewAccount = ();
    type OnKilledAccount = ();
    type SystemWeightInfo = ();
    type SS58Prefix = ();
    type OnSetCode = ();
    type MaxConsumers = frame_support::traits::ConstU32<16>;
}

impl pallet_archive::Config for Test {
    type Event = Event;
    type ArchiveId = UintAuthorityId;
    type MaxArchives = ConstU32<MAX_ARCHIVES>;
    type WeightInfo = ();
}

pub fn new_test_ext() -> sp_io::TestExternalities {
    let genesis_config = GenesisConfig::default();
    new_test_ext_with(genesis_config)
}

pub fn new_test_ext_with(genesis_config: GenesisConfig) -> sp_io::TestExternalities {
    let storage = genesis_config.build_storage().unwrap();
    storage.into()
}

#[test]
fn archive_registration_works() {
    new_test_ext().execute_with(|| {
        // Verify test preconditions.
        assert_eq!(Archive::archives(), vec![]);

        // Execute register call.
        assert_ok!(Archive::register(Origin::signed(1), 100.into()));

        // Ensure that the archive has been added.
        assert_eq!(Archive::archives(), vec![100.into()]);
    });
}

#[test]
fn archive_registration_already_registered_error() {
    new_test_ext().execute_with(|| {
        // Prepare test preconditions.
        let archives_before = vec![UintAuthorityId::from(100)];
        <Archives<Test>>::put(
            WeakBoundedVec::<_, ConstU32<MAX_ARCHIVES>>::try_from(archives_before.clone()).unwrap(),
        );

        // Execute register call.
        assert_noop!(
            Archive::register(Origin::signed(1), 100.into()),
            Error::<Test>::ArchiveAlreadyRegistered
        );

        // Ensure that the archive hasn't been added.
        assert_eq!(Archive::archives(), archives_before);
    });
}
