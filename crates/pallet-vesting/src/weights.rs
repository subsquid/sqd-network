use frame_support::weights::Weight;
use sp_runtime::traits::Zero;

/// Weight functions needed.
pub trait WeightInfo {
    /// Weight for `vest_locked` call.
    fn vest_locked() -> Weight;
}

impl WeightInfo for () {
    fn vest_locked() -> Weight {
        Weight::zero()
    }
}
