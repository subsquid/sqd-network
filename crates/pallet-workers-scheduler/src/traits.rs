pub trait PrepareTask {
    type Request;
    type Task;

    fn prepare_task(request: &Self::Request) -> Self::Task;
}
