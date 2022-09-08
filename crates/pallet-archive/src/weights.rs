use frame_support::weights::Weight;

/// Weight functions needed for pallet-archive.
pub trait WeightInfo {
    fn register() -> Weight;
}
