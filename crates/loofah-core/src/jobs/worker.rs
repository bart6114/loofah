use std::{future::Future, pin::Pin};

use tokio_util::sync::CancellationToken;

use super::*;
use crate::VaultContext;

pub type JobFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct JobWork {
    pub principal: String,
    pub request: SummaryJobRequest,
}

/// The generator prepares a body; the worker owns canonical application.
/// Its future retains underlying processing resources until their I/O stops.
pub trait JobGenerator: Send + Sync + 'static {
    fn generate(
        &self,
        work: JobWork,
        cancel: CancellationToken,
    ) -> JobFuture<'_, Result<GeneratedDocument, ProcessingJobFailure>>;
}

pub trait JobAuthorizer: Send + Sync + 'static {
    // A host grant holds current vault access through the short canonical commit.
    fn authorize<'a>(
        &'a self,
        work: &'a JobWork,
    ) -> JobFuture<'a, Result<Box<dyn Send>, ProcessingJobFailure>>;
}

/// Owned by the vault runtime; dropping this handle requests worker shutdown.
pub struct JobWorker {
    stop: CancellationToken,
    task: Option<tokio::task::JoinHandle<Result<(), ApplicationError>>>,
}
impl Drop for JobWorker {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
impl JobWorker {
    pub async fn start(
        context: Arc<VaultContext>,
        journal: JobJournal,
        generator: Arc<dyn JobGenerator>,
        authorizer: Arc<dyn JobAuthorizer>,
    ) -> Result<Self, ApplicationError> {
        if context.events.vault_id() != journal.0.events.vault_id() {
            return Err(rejected(
                ErrorCode::Validation,
                "Processing worker vault mismatch",
            ));
        }
        let permit = journal.0.worker.clone().try_acquire_owned().map_err(|_| {
            rejected(
                ErrorCode::Conflict,
                "A processing worker already owns this vault",
            )
        })?;
        journal
            .run(|state, events| state.interrupt_active(events))
            .await?;
        let stop = CancellationToken::new();
        let stopping = stop.clone();
        let task = tokio::spawn(async move {
            let _finished = stopping.clone().drop_guard();
            let _permit = permit;
            run(context, journal, generator, authorizer, stopping).await
        });
        Ok(Self {
            stop,
            task: Some(task),
        })
    }
    pub async fn shutdown(mut self) -> Result<(), ApplicationError> {
        self.stop.cancel();
        self.task.take().unwrap().await.map_err(|_| storage())?
    }

    pub(crate) fn lifetime(&self) -> CancellationToken {
        self.stop.clone()
    }

    pub(crate) fn request_shutdown(&self) {
        self.stop.cancel();
    }
}

async fn run(
    context: Arc<VaultContext>,
    journal: JobJournal,
    generator: Arc<dyn JobGenerator>,
    authorizer: Arc<dyn JobAuthorizer>,
    stop: CancellationToken,
) -> Result<(), ApplicationError> {
    let mut changes = journal.0.events.subscribe(None);
    loop {
        if stop.is_cancelled() {
            return Ok(());
        }
        let queued = journal.queued().await?;
        if queued.is_empty() {
            tokio::select! {
                _ = stop.cancelled() => return Ok(()),
                change = changes.recv() => if change.is_none() { return Ok(()); },
            }
            continue;
        }
        for job in queued {
            if stop.is_cancelled() {
                return Ok(());
            }
            if journal.claim(job.id.clone()).await?.is_none() {
                continue;
            }
            let completion = execute(
                &context,
                &journal,
                generator.as_ref(),
                authorizer.as_ref(),
                &stop,
                &job.id,
            )
            .await?;
            journal.complete(job.id, completion).await?;
        }
    }
}

async fn execute(
    context: &Arc<VaultContext>,
    journal: &JobJournal,
    generator: &dyn JobGenerator,
    authorizer: &dyn JobAuthorizer,
    stop: &CancellationToken,
    id: &str,
) -> Result<JobCompletion, ApplicationError> {
    let work = journal.work(id.into()).await?;
    let grant = match authorizer.authorize(&work).await {
        Ok(grant) => grant,
        Err(failure) => return Ok(JobCompletion::Failed(failure)),
    };
    let baseline = context
        .read_document(&work.request.session_id, &work.request.target)
        .await;
    drop(grant);
    match baseline {
        Ok(snapshot)
            if matches!(work.request.target, DocumentTarget::TemplateOutput { .. })
                && snapshot.document.is_none() =>
        {
            return Ok(JobCompletion::Failed(
                ProcessingJobFailure::SourceUnavailable,
            ));
        }
        Ok(snapshot) if snapshot.revision != work.request.expected_revision => {
            return Ok(JobCompletion::Conflict);
        }
        Err(_) if stop.is_cancelled() => return Ok(JobCompletion::Interrupted),
        Err(_) => {
            return Ok(JobCompletion::Failed(
                ProcessingJobFailure::SourceUnavailable,
            ));
        }
        _ => {}
    }
    if stop.is_cancelled() {
        return Ok(JobCompletion::Interrupted);
    }
    if cancelled(journal, id).await? {
        return Ok(JobCompletion::Cancelled);
    }
    journal
        .progress(id.into(), ProcessingPhase::Generating)
        .await?;
    let cancel = CancellationToken::new();
    let mut generation = generator.generate(work.clone(), cancel.clone());
    let mut changes = journal.0.events.subscribe(None);
    let mut checks = tokio::time::interval(std::time::Duration::from_secs(1));
    checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let outcome = loop {
        tokio::select! {
            output = &mut generation => break Ok(output),
            _ = stop.cancelled() => break Err(JobCompletion::Interrupted),
            _ = checks.tick() => {},
            _ = changes.recv() => {},
        }
        match cancelled(journal, id).await {
            Ok(true) => break Err(JobCompletion::Cancelled),
            Ok(false) => {}
            Err(failure) => {
                cancel.cancel();
                let _ = generation.await;
                return Err(failure);
            }
        }
        if let Err(failure) = authorizer.authorize(&work).await {
            break Err(JobCompletion::Failed(failure));
        }
    };
    let output = match outcome {
        Ok(output) => output,
        Err(completion) => {
            cancel.cancel();
            // Keep the worker permit and owned I/O alive until the generator has actually stopped.
            if let Ok(output) = generation.await
                && !output.markdown.trim().is_empty()
                && output.markdown.len() <= 4 * 1024 * 1024
            {
                journal.retain_result(id.into(), output).await?;
            }
            return Ok(completion);
        }
    };
    let output = match output {
        Ok(output) => output,
        Err(failure) => {
            return Ok(if cancelled(journal, id).await? {
                JobCompletion::Cancelled
            } else {
                JobCompletion::Failed(failure)
            });
        }
    };
    if output.markdown.trim().is_empty() || output.markdown.len() > 4 * 1024 * 1024 {
        return Ok(JobCompletion::Failed(ProcessingJobFailure::InvalidOutput));
    }
    journal.retain_result(id.into(), output.clone()).await?;
    if stop.is_cancelled() {
        return Ok(JobCompletion::Interrupted);
    }
    let grant = match authorizer.authorize(&work).await {
        Ok(grant) => grant,
        Err(failure) => return Ok(JobCompletion::Failed(failure)),
    };
    if stop.is_cancelled() {
        return Ok(JobCompletion::Interrupted);
    }
    if !journal.begin_apply(id.into()).await? {
        return Ok(JobCompletion::Cancelled);
    }
    let applied = context
        .save_generated_document_body(
            &work.request.session_id,
            &work.request.target,
            hypr_loofah_protocol::SaveDocumentRequest {
                markdown: output.markdown,
                expected_revision: work.request.expected_revision,
            },
        )
        .await;
    drop(grant);
    Ok(completion_from_apply(applied))
}

fn completion_from_apply(
    applied: Result<hypr_loofah_protocol::DocumentSaveOutcome, ApplicationError>,
) -> JobCompletion {
    match applied {
        Ok(hypr_loofah_protocol::DocumentSaveOutcome::Saved(_)) => JobCompletion::Succeeded,
        Ok(hypr_loofah_protocol::DocumentSaveOutcome::Conflict(_)) => JobCompletion::Conflict,
        Err(failure)
            if matches!(
                failure.mutation_outcome,
                Some(MutationOutcome::Unknown | MutationOutcome::Persisted)
            ) =>
        {
            JobCompletion::Interrupted
        }
        Err(failure) if failure.code == ErrorCode::Unavailable => JobCompletion::Interrupted,
        Err(failure) if failure.code == ErrorCode::NotFound => {
            JobCompletion::Failed(ProcessingJobFailure::SourceUnavailable)
        }
        Err(failure)
            if matches!(
                failure.code,
                ErrorCode::Validation | ErrorCode::ResourceLimit
            ) =>
        {
            JobCompletion::Failed(ProcessingJobFailure::InvalidOutput)
        }
        Err(_) => JobCompletion::Failed(ProcessingJobFailure::Storage),
    }
}

async fn cancelled(journal: &JobJournal, id: &str) -> Result<bool, ApplicationError> {
    Ok(journal
        .get(id.into())
        .await?
        .is_some_and(|job| job.state == ProcessingJobState::Cancelling))
}

#[cfg(test)]
mod tests;
