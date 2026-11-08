use hypr_loofah_protocol::{ProcessingTitleResult, ProcessingTitleStatus};

use super::{title_stage::TitleStage, *};
use crate::generated_title::{GeneratedTitleOutcome, GeneratedTitlePreparation};

pub(super) struct TitleExecution<'a> {
    pub context: &'a Arc<crate::VaultContext>,
    pub journal: &'a JobJournal,
    pub authorizer: &'a dyn JobAuthorizer,
    pub stop: &'a tokio_util::sync::CancellationToken,
    pub id: &'a str,
}

impl TitleExecution<'_> {
    pub async fn run(
        &self,
        work: &JobWork,
        saved_revision: &str,
        runner: Option<Box<dyn TitleRunner>>,
    ) {
        let Some(runner) = runner else {
            self.finish(outcome(ProcessingTitleStatus::Unsupported, None))
                .await;
            return;
        };
        if let Some(interruption) = self.interrupted().await {
            self.finish(interruption).await;
            return;
        }
        let grant = match self.authorizer.authorize(work).await {
            Ok(grant) => grant,
            Err(failure) => {
                self.finish(failed(failure)).await;
                return;
            }
        };
        let baseline = self
            .context
            .prepare_generated_title(&work.request.session_id)
            .await;
        let baseline = match baseline {
            Ok(GeneratedTitlePreparation::AlreadyNamed) => {
                drop(grant);
                self.finish(outcome(ProcessingTitleStatus::AlreadyNamed, None))
                    .await;
                return;
            }
            Ok(GeneratedTitlePreparation::Ready(baseline)) => baseline,
            Err(failure) => {
                drop(grant);
                self.finish(application_failure(failure, None)).await;
                return;
            }
        };
        // A newer body may have arrived before preparation captured its baseline.
        // Checking after preparation closes that gap; the opaque baseline guards later edits.
        let current = self
            .context
            .read_document(&work.request.session_id, &work.request.target)
            .await;
        drop(grant);
        let body = match current {
            Ok(snapshot) if snapshot.revision == saved_revision => match snapshot.document {
                Some(document) => document.markdown,
                None => {
                    self.finish(failed(ProcessingJobFailure::SourceUnavailable))
                        .await;
                    return;
                }
            },
            Ok(_) => {
                self.finish(outcome(ProcessingTitleStatus::Conflict, None))
                    .await;
                return;
            }
            Err(failure) => {
                self.finish(application_failure(failure, None)).await;
                return;
            }
        };
        if let Some(interruption) = self.interrupted().await {
            self.finish(interruption).await;
            return;
        }
        if self
            .journal
            .advance_title(self.id.into(), TitleStage::Generating {})
            .await
            .is_err()
        {
            return;
        }
        title_stage::checkpoint("title_generating");
        let candidate = match self.generate(work, runner, body).await {
            Ok(Some(candidate)) => candidate,
            Ok(None) => {
                self.finish(outcome(ProcessingTitleStatus::Empty, None))
                    .await;
                return;
            }
            Err(result) => {
                self.finish(result).await;
                return;
            }
        };
        title_stage::checkpoint("title_generated");
        if self
            .journal
            .advance_title(
                self.id.into(),
                TitleStage::Candidate {
                    candidate: candidate.clone(),
                },
            )
            .await
            .is_err()
        {
            return;
        }
        title_stage::checkpoint("title_candidate");
        if let Some(mut interruption) = self.interrupted().await {
            interruption.candidate = Some(candidate);
            self.finish(interruption).await;
            return;
        }
        let grant = match self.authorizer.authorize(work).await {
            Ok(grant) => grant,
            Err(failure) => {
                let mut result = failed(failure);
                result.candidate = Some(candidate);
                self.finish(result).await;
                return;
            }
        };
        if let Some(mut interruption) = self.interrupted().await {
            interruption.candidate = Some(candidate);
            drop(grant);
            self.finish(interruption).await;
            return;
        }
        if let Err(failure) = self
            .journal
            .advance_title(
                self.id.into(),
                TitleStage::Applying {
                    candidate: candidate.clone(),
                },
            )
            .await
        {
            if failure.code == ErrorCode::Cancelled {
                drop(grant);
                self.finish(outcome(ProcessingTitleStatus::Cancelled, Some(candidate)))
                    .await;
            }
            return;
        }
        title_stage::checkpoint("title_applying");
        // Once admitted, await actual canonical I/O with the grant still held.
        let applied = self
            .context
            .apply_generated_title(baseline, candidate.clone())
            .await;
        title_stage::checkpoint("title_applied");
        drop(grant);
        let result = match applied {
            Ok(GeneratedTitleOutcome::Applied { .. }) => {
                outcome(ProcessingTitleStatus::Applied, Some(candidate))
            }
            Ok(GeneratedTitleOutcome::AlreadyNamed) => {
                outcome(ProcessingTitleStatus::AlreadyNamed, Some(candidate))
            }
            Ok(GeneratedTitleOutcome::Conflict) => {
                outcome(ProcessingTitleStatus::Conflict, Some(candidate))
            }
            Ok(GeneratedTitleOutcome::Empty) => outcome(ProcessingTitleStatus::Empty, None),
            Err(failure) => application_failure(failure, Some(candidate)),
        };
        self.finish(result).await;
        title_stage::checkpoint("title_finished");
    }

    async fn interrupted(&self) -> Option<ProcessingTitleResult> {
        if self.stop.is_cancelled() {
            return Some(outcome(ProcessingTitleStatus::Interrupted, None));
        }
        match self.journal.get(self.id.into()).await {
            Ok(Some(job)) if job.state == ProcessingJobState::Cancelling => {
                Some(outcome(ProcessingTitleStatus::Cancelled, None))
            }
            Ok(Some(_)) => None,
            _ => Some(failed(ProcessingJobFailure::Storage)),
        }
    }

    async fn generate(
        &self,
        work: &JobWork,
        runner: Box<dyn TitleRunner>,
        body: String,
    ) -> Result<Option<String>, ProcessingTitleResult> {
        let cancel = tokio_util::sync::CancellationToken::new();
        let mut generation = runner.generate(body, cancel.clone());
        let mut changes = self.journal.0.events.subscribe(None);
        let mut checks = tokio::time::interval(std::time::Duration::from_secs(1));
        checks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let result = loop {
            tokio::select! {
                result = &mut generation => break Ok(result),
                _ = self.stop.cancelled() => break Err(outcome(ProcessingTitleStatus::Interrupted, None)),
                _ = checks.tick() => {},
                _ = changes.recv() => {},
            }
            if let Some(interruption) = self.interrupted().await {
                break Err(interruption);
            }
            if let Err(failure) = self.authorizer.authorize(work).await {
                break Err(failed(failure));
            }
        };
        match result {
            Ok(Ok(Some(candidate))) => {
                title_generation::normalize_candidate(&candidate).map_err(failed)
            }
            Ok(Ok(None)) => Ok(None),
            Ok(Err(failure)) => Err(self.interrupted().await.unwrap_or_else(|| failed(failure))),
            Err(mut interruption) => {
                cancel.cancel();
                if let Ok(Some(candidate)) = generation.await
                    && let Ok(Some(candidate)) = title_generation::normalize_candidate(&candidate)
                {
                    interruption.candidate = Some(candidate);
                }
                Err(interruption)
            }
        }
    }

    async fn finish(&self, result: ProcessingTitleResult) {
        if self
            .journal
            .advance_title(self.id.into(), TitleStage::finished(result))
            .await
            .is_err()
        {
            tracing::warn!(
                job_id = self.id,
                "Optional title status could not be confirmed"
            );
        }
    }
}

fn outcome(status: ProcessingTitleStatus, candidate: Option<String>) -> ProcessingTitleResult {
    ProcessingTitleResult {
        status,
        candidate,
        failure: None,
    }
}

fn failed(failure: ProcessingJobFailure) -> ProcessingTitleResult {
    if failure == ProcessingJobFailure::Interrupted {
        outcome(ProcessingTitleStatus::Interrupted, None)
    } else {
        ProcessingTitleResult {
            status: ProcessingTitleStatus::Failed,
            candidate: None,
            failure: Some(failure),
        }
    }
}

fn application_failure(
    failure: ApplicationError,
    candidate: Option<String>,
) -> ProcessingTitleResult {
    if matches!(
        failure.mutation_outcome,
        Some(MutationOutcome::Unknown | MutationOutcome::Persisted)
    ) {
        return outcome(ProcessingTitleStatus::Unconfirmed, candidate);
    }
    if failure.code == ErrorCode::Conflict {
        return outcome(ProcessingTitleStatus::Conflict, candidate);
    }
    let mut result = failed(match failure.code {
        ErrorCode::Forbidden | ErrorCode::Unauthenticated => ProcessingJobFailure::AccessDenied,
        ErrorCode::NotFound | ErrorCode::Unavailable => ProcessingJobFailure::SourceUnavailable,
        ErrorCode::Validation => ProcessingJobFailure::InvalidOutput,
        ErrorCode::ResourceLimit => ProcessingJobFailure::ResourceLimit,
        _ => ProcessingJobFailure::Storage,
    });
    result.candidate = candidate;
    result
}
