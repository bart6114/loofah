use crate::BatchEvent;

pub type BatchCheckpointFuture<'a> =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send + 'a>>;

pub trait BatchRuntime: Send + Sync + 'static {
    fn emit(&self, event: BatchEvent);
    fn checkpoint<'a>(
        &'a self,
        _output: &'a mut crate::BatchRunOutput,
    ) -> BatchCheckpointFuture<'a> {
        Box::pin(async { Ok(()) })
    }
    fn is_cancelled(&self) -> bool {
        false
    }
}
