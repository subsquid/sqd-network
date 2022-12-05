use crate::{
    requests::{Request, RequestIdGenerator},
    workers::{Command, DockerImage, Task, TaskData},
};
use pallet_requests::traits::RequestIdGenerator as RequestIdGeneratorT;
use pallet_workers_scheduler::traits::PrepareTask;

pub struct TaskPreparation;

impl PrepareTask for TaskPreparation {
    type Request = Request;
    type Task = Task;

    fn prepare_task(request: Self::Request) -> Task {
        Task::Execute(TaskData {
            request_id: RequestIdGenerator::generate_id(request),
            docker_image: DockerImage::default(),
            command: Command::default(),
        })
    }
}
