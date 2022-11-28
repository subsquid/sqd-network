use frame_support::weights::Weight;
use sp_runtime::traits::Zero;

/// Weight functions needed for pallet-worker.
pub trait WeightInfo {
    fn register() -> Weight;
    fn done() -> Weight;
}

impl WeightInfo for () {
    fn register() -> Weight {
        Weight::zero()
    }

    fn done() -> Weight {
        Weight::zero()
    }
}
