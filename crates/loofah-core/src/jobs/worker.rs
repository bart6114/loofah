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
    finished: CancellationToken,
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
        let finished = CancellationToken::new();
        let finished_guard = finished.clone().drop_guard();
        let task = tokio::spawn(async move {
            let _finished_guard = finished_guard;
            let _finished = stopping.clone().drop_guard();
            let _permit = permit;
            run(context, journal, generator, authorizer, stopping).await
        });
        Ok(Self {
            stop,
            finished,
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

    pub(crate) fn finished(&self) -> CancellationToken {
        self.finished.clone()
    }

    pub fn request_shutdown(&self) {
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

fn local_parent_refusal(failure: &ApplicationError) -> JobCompletion {
    if failure.code == ErrorCode::Cancelled {
        JobCompletion::Cancelled
    } else {
        JobCompletion::Interrupted
    }
}

fn failure_completion(failure: ProcessingJobFailure) -> JobCompletion {
    match failure {
        ProcessingJobFailure::Interrupted => JobCompletion::Interrupted,
        other => JobCompletion::Failed(other),
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
    let _work = context.admit_session_work(&work.request.session_id).await?;
    let (request_id, origin) = journal.capture_request_id(id.into()).await?;
    let local_origin = journal.local_capture_origin(id.into()).await?;
    let manual_origin = journal.manual_media_origin(id.into()).await?;
    if context
        .guard_manual_media_summary(&work.principal, &request_id, manual_origin.as_ref())
        .await
        .is_err()
    {
        return Ok(JobCompletion::Interrupted);
    }

    if context
        .guard_local_capture_summary(&work.principal, &request_id, local_origin.as_ref())
        .await
        .is_err()
    {
        return Ok(JobCompletion::Interrupted);
    }
    if context
        .capture_child_admission(&work.principal, &request_id, origin.as_ref())
        .await
        .is_err()
    {
        return Ok(JobCompletion::Interrupted);
    }
    #[cfg(test)]
    local_capture_tests::checkpoint(&work.request.session_id, "summary_source_ready").await;
    let grant = match authorizer.authorize(&work).await {
        Ok(grant) => grant,
        Err(failure) => return Ok(failure_completion(failure)),
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
        Ok(snapshot) if conditional_target_filled(&work.request, &snapshot) => {
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
    let parent_cancel = match context
        .local_capture_cancellation(&work.principal, &request_id, local_origin.as_ref())
        .await
    {
        Ok(token) => token.unwrap_or_default(),
        Err(_) => return Ok(JobCompletion::Interrupted),
    };
    #[cfg(test)]
    local_capture_tests::checkpoint(&work.request.session_id, "summary_generate").await;
    let parent_admission = match context
        .local_capture_dispatch_guard(&work.principal, &request_id, local_origin.as_ref())
        .await
    {
        Ok(guard) => guard,
        Err(failure) => return Ok(local_parent_refusal(&failure)),
    };
    let cancel = parent_cancel.child_token();
    let (preview, mut preview_changes) = GenerationPreview::channel();
    let mut preview_cursor = None;
    let mut generation = generator.generate_with_preview(
        work.clone(),
        cancel.clone(),
        preview.clone(),
        work.request.title_policy == TitlePolicy::GenerateIfEmpty,
    );
    // Cancellation and the first model poll share the original parent gate.
    // Release it before waiting for model work or its physical drain.
    let first =
        std::future::poll_fn(|cx| std::task::Poll::Ready(generation.as_mut().poll(cx))).await;
    drop(parent_admission);
    let mut generation = Box::pin(async move {
        match first {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => generation.await,
        }
    });
    let mut changes = journal.0.events.subscribe(None);
    let mut checks = tokio::time::interval(std::time::Duration::from_secs(1));
    checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut preview_tick = tokio::time::interval(std::time::Duration::from_millis(500));
    preview_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let outcome = loop {
        tokio::select! {
            output = &mut generation => break Ok(output),
            _ = preview_tick.tick() => { preview.publish_pending(false); },
            _ = preview_changes.changed() => { flush_preview(journal, id, &preview, &mut preview_cursor).await; },
            _ = stop.cancelled() => break Err(JobCompletion::Interrupted),
            _ = parent_cancel.cancelled() => {
                let completion = context.local_capture_dispatch_guard(&work.principal, &request_id, local_origin.as_ref())
                    .await.err().map_or(JobCompletion::Interrupted, |failure| local_parent_refusal(&failure));
                break Err(completion);
            },
            _ = checks.tick() => {},
            _ = changes.recv() => {},
        }
        match cancelled(journal, id).await {
            Ok(true) => break Err(JobCompletion::Cancelled),
            Ok(false) => {}
            Err(failure) => {
                cancel.cancel();
                let _ = generation.await;
                flush_preview(journal, id, &preview, &mut preview_cursor).await;
                return Err(failure);
            }
        }
        if let Err(failure) = authorizer.authorize(&work).await {
            break Err(failure_completion(failure));
        }
    };
    let output = match outcome {
        Ok(output) => output,
        Err(completion) => {
            cancel.cancel();
            // Keep the worker permit and owned I/O alive until the generator has actually stopped.
            if let Ok(prepared) = generation.await
                && result::validate_output(&prepared.document).is_ok()
            {
                journal.retain_result(id.into(), prepared.document).await?;
            }
            flush_preview(journal, id, &preview, &mut preview_cursor).await;
            return Ok(completion);
        }
    };
    flush_preview(journal, id, &preview, &mut preview_cursor).await;
    let PreparedGeneration {
        document: output,
        title,
    } = match output {
        Ok(output) => output,
        Err(failure) => {
            return Ok(if cancelled(journal, id).await? {
                JobCompletion::Cancelled
            } else {
                failure_completion(failure)
            });
        }
    };
    if result::validate_output(&output).is_err() {
        return Ok(JobCompletion::Failed(ProcessingJobFailure::InvalidOutput));
    }
    journal.retain_result(id.into(), output.clone()).await?;
    if stop.is_cancelled() {
        return Ok(JobCompletion::Interrupted);
    }
    let grant = match authorizer.authorize(&work).await {
        Ok(grant) => grant,
        Err(failure) => return Ok(failure_completion(failure)),
    };
    if stop.is_cancelled() {
        return Ok(JobCompletion::Interrupted);
    }
    if !journal.begin_apply(id.into()).await? {
        return Ok(JobCompletion::Cancelled);
    }
    let manual_source_guard = match context
        .guard_manual_media_summary(&work.principal, &request_id, manual_origin.as_ref())
        .await
    {
        Ok(guard) => guard,
        Err(_) => return Ok(JobCompletion::Conflict),
    };
    let local_source_guard = match context
        .guard_local_capture_summary(&work.principal, &request_id, local_origin.as_ref())
        .await
    {
        Ok(guard) => guard,
        Err(_) => return Ok(JobCompletion::Conflict),
    };
    let source_guard = match context
        .guard_capture_summary_source(&work.principal, &request_id, origin.as_ref())
        .await
    {
        Ok(guard) => guard,
        Err(_) => return Ok(JobCompletion::Conflict),
    };
    #[cfg(test)]
    local_capture_tests::checkpoint(&work.request.session_id, "summary_apply").await;
    let parent_admission = match context
        .local_capture_dispatch_guard(&work.principal, &request_id, local_origin.as_ref())
        .await
    {
        Ok(guard) => guard,
        Err(failure) => return Ok(local_parent_refusal(&failure)),
    };
    #[cfg(test)]
    local_capture_tests::checkpoint(&work.request.session_id, "summary_apply_admitted").await;
    let input = hypr_loofah_protocol::SaveDocumentRequest {
        markdown: output.markdown,
        expected_revision: work.request.expected_revision.clone(),
    };
    let applied = match &work.request.target {
        DocumentTarget::Summary => {
            context
                .save_generated_summary(&work.request.session_id, input, output.suggested_tags)
                .await
        }
        DocumentTarget::TemplateOutput { document_id } => {
            context
                .save_generated_template_output(
                    &work.request.session_id,
                    document_id,
                    input,
                    output.suggested_tags,
                )
                .await
        }
    };
    // Once admitted, the owned canonical write finishes before parent cancellation.
    drop(parent_admission);
    #[cfg(test)]
    local_capture_tests::checkpoint(&work.request.session_id, "summary_written").await;
    drop(source_guard);
    drop(local_source_guard);
    drop(manual_source_guard);
    if work.request.title_policy == TitlePolicy::GenerateIfEmpty
        && let Ok(hypr_loofah_protocol::DocumentSaveOutcome::Saved(snapshot)) = &applied
    {
        title_stage::checkpoint("body_saved");
        let revision = snapshot.revision.clone();
        journal
            .mark_body_applied(id.into(), revision.clone())
            .await?;
        drop(applied);
        drop(grant);
        title_stage::checkpoint("body_confirmed");
        title_worker::TitleExecution {
            context,
            journal,
            authorizer,
            stop,
            id,
        }
        .run(&work, &revision, title)
        .await;
        return Ok(JobCompletion::Succeeded);
    }
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

async fn flush_preview(
    journal: &JobJournal,
    id: &str,
    preview: &GenerationPreview,
    cursor: &mut Option<String>,
) {
    let Some(snapshot) = preview.current() else {
        return;
    };
    if cursor.as_deref() == Some(&snapshot.cursor) {
        return;
    }
    *cursor = Some(snapshot.cursor.clone());
    if journal
        .checkpoint_preview(id.into(), snapshot)
        .await
        .is_err()
    {
        tracing::warn!("Processing preview checkpoint unavailable");
    }
}

#[cfg(test)]
pub(super) mod local_capture_tests;
