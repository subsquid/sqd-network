use frame_support::weights::Weight;
use sp_runtime::traits::Zero;

/// Weight functions needed.
pub trait WeightInfo {
    /// Weight for `register` call.
    fn register() -> Weight;
    /// Weight for `update_info` call.
    fn update_info() -> Weight;
}

impl WeightInfo for () {
    fn register() -> Weight {
        Weight::zero()
    }

    fn update_info() -> Weight {
        Weight::zero()
    }
}
