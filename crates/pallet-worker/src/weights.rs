use frame_support::weights::Weight;

/// Weight functions needed.
pub trait WeightInfo {
    /// Weight for `register` call.
    fn register() -> Weight;
    /// Weight for `unregister` call.
    fn unregister() -> Weight;
    /// Weight for `update_spec` call.
    fn update_spec() -> Weight;
    /// Weight for `go_online` call.
    fn go_online() -> Weight;
    /// Weight for `go_offline` call.
    fn go_offline() -> Weight;
    /// Weight for `done` call.
    fn done() -> Weight;
    /// Weight for `submit_task` call.
    fn submit_task() -> Weight;
    /// Weight for `force_run_task` call.
    fn force_run_task() -> Weight;
}

impl WeightInfo for () {
    fn register() -> Weight {
        Weight::zero()
    }

    fn unregister() -> Weight {
        Weight::zero()
    }

    fn update_spec() -> Weight {
        Weight::zero()
    }

    fn go_online() -> Weight {
        Weight::zero()
    }

    fn go_offline() -> Weight {
        Weight::zero()
    }

    fn done() -> Weight {
        Weight::zero()
    }

    fn submit_task() -> Weight {
        Weight::zero()
    }

    fn force_run_task() -> Weight {
        Weight::zero()
    }
}
