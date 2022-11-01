use crate::DataSourceId;
use pallet_requests::Request;
use pallet_workers_scheduler::traits::IsDataSourceSuit as IsDataSourceSuitT;

pub struct IsDataSourceSuit;

impl IsDataSourceSuitT for IsDataSourceSuit {
    type DataSourceId = DataSourceId;
    type Request = Request;

    fn is_suit(
        _data_source_id: &Self::DataSourceId,
        _data_range: &pallet_data_source::DataRange,
        _request: &Self::Request,
    ) -> bool {
        true
    }
}
