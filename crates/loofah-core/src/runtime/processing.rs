use hypr_loofah_protocol::{ApplicationError, ErrorCode};

use super::*;
use crate::{
    jobs::{JobAuthorizer, JobGenerator, JobJournal, JobWorker, ProcessingService},
    vault::error,
};

impl VaultRuntime {
    pub async fn enable_processing(
        &mut self,
        generator: Arc<dyn JobGenerator>,
        authorizer: Arc<dyn JobAuthorizer>,
    ) -> Result<(), ApplicationError> {
        self.context
            .store
            .ensure_ready()
            .map_err(|_| unavailable())?;
        if self.processing.is_some() || self.context.processing.get().is_some() {
            return Err(error(
                ErrorCode::Conflict,
                "Processing is already configured for this vault",
            ));
        }
        let operations = self.context.operations.as_ref().ok_or_else(unavailable)?;
        let journal =
            JobJournal::open(operations.root.join("jobs"), self.context.events.clone()).await?;
        let worker =
            JobWorker::start(self.context.clone(), journal.clone(), generator, authorizer).await?;
        let service = ProcessingService {
            journal,
            lifetime: worker.lifetime(),
            finished: worker.finished(),
        };
        if self.context.processing.set(service).is_err() {
            worker.shutdown().await?;
            return Err(error(
                ErrorCode::Conflict,
                "Processing is already configured for this vault",
            ));
        }
        self.processing = Some(worker);
        Ok(())
    }
}

impl VaultContext {
    pub fn processing_jobs(&self) -> Result<&JobJournal, ApplicationError> {
        self.store.ensure_ready().map_err(|_| unavailable())?;
        self.processing
            .get()
            .filter(|service| !service.lifetime.is_cancelled())
            .map(|service| &service.journal)
            .ok_or_else(unavailable)
    }
}

fn unavailable() -> ApplicationError {
    error(
        ErrorCode::Unavailable,
        "Processing is not available for this vault",
    )
}

#[cfg(test)]
mod tests;
