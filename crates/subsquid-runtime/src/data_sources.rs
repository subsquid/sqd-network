use pallet_substrate_native_requests::Request;
use pallet_workers_scheduler::traits::IsDataSourceSuit as IsDataSourceSuitT;

pub struct IsDataSourceSuit;

impl IsDataSourceSuitT for IsDataSourceSuit {
    type Request = Request;

    fn is_suit(data_source: &pallet_data_source::Data, request: &Request) -> bool {
        true
    }
}
