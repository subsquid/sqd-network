use crate::{DataSourceId, WorkerId};
use pallet_requests::Request;
use pallet_workers_scheduler::traits::PrepareTask;
use primitives_worker::Task;

pub struct TaskPreparation;

impl PrepareTask for TaskPreparation {
    type WorkerId = WorkerId;
    type DataSourceId = DataSourceId;
    type Request = Request;

    fn prepare_task(
        _worker_id: &Self::WorkerId,
        _data_source_id: &Self::DataSourceId,
        _request: &Self::Request,
    ) -> Task {
        Task {
            docker_image: primitives_worker::DockerImage::SubstrateWorker,
            command: primitives_worker::Command::Run,
            result_storage: primitives_worker::ResultStorage::IPFS,
        }
    }
}
