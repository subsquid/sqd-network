pub trait IsDataSourceSuit {
    type DataSourceId;
    type Request;

    fn is_suit(
        data_source_id: &Self::DataSourceId,
        data_range: &pallet_data_source::DataRange,
        request: &Self::Request,
    ) -> bool;
}

pub trait PrepareTask {
    type WorkerId;
    type DataSourceId;
    type Request;

    fn prepare_task(
        worker_id: &Self::WorkerId,
        data_source_id: &Self::DataSourceId,
        request: &Self::Request,
    ) -> primitives_worker::Task;
}
