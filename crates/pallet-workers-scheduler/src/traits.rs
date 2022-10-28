pub trait IsDataSourceSuit {
    type Request;

    fn is_suit(data_source: &pallet_data_source::Data, request: &Self::Request) -> bool;
}
