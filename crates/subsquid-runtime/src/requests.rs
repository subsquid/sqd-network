use codec::{Decode, Encode, MaxEncodedLen};
use pallet_substrate_native_requests::{traits::RequestIdGenerator, Request};
use scale_info::TypeInfo;

#[derive(PartialEq, Copy, Eq, Clone, Encode, Decode, Hash, Debug, TypeInfo, MaxEncodedLen)]
pub enum Status {
    Scheduling,
    Scheduled,
    Done,
}

pub struct IdGenerator;

impl RequestIdGenerator for IdGenerator {
    type Id = [u8; 32];
    type Data = Request;

    fn generate_id(request: Self::Data) -> Self::Id {
        request.call
    }
}

// 1. Финиширую PoC.
// 2. Иметь возможность выкатывать.
// 3. Для Озгура сделать легкого клиента.
// 4. Показать и определить симуляция worker и data source с Озгуром.
