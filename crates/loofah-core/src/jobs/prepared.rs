use hypr_loofah_protocol::ProcessingJobFailure;
use tokio_util::sync::CancellationToken;

use super::{GeneratedDocument, JobFuture, JobWork};

pub struct PreparedGeneration {
    pub document: GeneratedDocument,
    pub title: Option<Box<dyn TitleRunner>>,
}

/// A one-use continuation whose resources remain owned through actual title drain.
pub trait TitleRunner: Send + 'static {
    fn generate(
        self: Box<Self>,
        enhanced_body: String,
        cancel: CancellationToken,
    ) -> JobFuture<'static, Result<Option<String>, ProcessingJobFailure>>;
}

/// The generator prepares a body; the worker owns canonical application.
/// Its future retains underlying processing resources until their I/O stops.
pub trait JobGenerator: Send + Sync + 'static {
    fn generate(
        &self,
        work: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>>;

    fn generate_prepared(
        &self,
        work: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<PreparedGeneration, ProcessingJobFailure>> {
        Box::pin(async move {
            let document = self.generate(work, cancel).await?;
            Ok(PreparedGeneration {
                document,
                title: None,
            })
        })
    }
}
