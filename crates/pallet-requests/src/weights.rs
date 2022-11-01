use frame_support::weights::Weight;
use sp_runtime::traits::Zero;

/// Weight functions needed for pallet.
pub trait WeightInfo {
    fn request() -> Weight;
}

impl WeightInfo for () {
    fn request() -> Weight {
        Weight::zero()
    }
}
