use frame_support::weights::Weight;
use sp_runtime::traits::Zero;

/// Weight functions needed.
pub trait WeightInfo {
    /// Weight for `register` call.
    fn register() -> Weight;
    /// Weight for `done` call.
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
