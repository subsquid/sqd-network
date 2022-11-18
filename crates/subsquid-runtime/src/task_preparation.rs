use crate::{
    requests::Request,
    workers::{Command, DockerImage, ExecTask, Task},
};
use pallet_workers_scheduler::traits::PrepareTask;

pub struct TaskPreparation;

impl PrepareTask for TaskPreparation {
    type Request = Request;
    type Task = Task;

    fn prepare_task(_request: &Self::Request) -> Task {
        Task::Execute(ExecTask {
            docker_image: DockerImage::SubstrateWorker,
            command: Command::Parse,
        })
    }
}
