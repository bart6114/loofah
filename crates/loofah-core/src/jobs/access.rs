use hypr_loofah_protocol::{
    ListProcessingJobsInput, ProcessingJobPage, ProcessingJobResult, ProcessingTagSuggestion,
};

use super::*;
use crate::VaultContext;

impl State {
    fn owned(&self, namespace: &str, id: &str) -> Result<&Record, ApplicationError> {
        self.ids
            .get(id)
            .and_then(|key| self.records.get(key))
            .filter(|record| record.namespace == namespace)
            .ok_or_else(|| rejected(ErrorCode::NotFound, "Processing job not found"))
    }
}

impl JobJournal {
    pub async fn owned_job(
        &self,
        namespace: String,
        id: String,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        validate_namespace(&namespace)?;
        let id = parse_uuid(&id)?;
        self.run(move |state, _| Ok(state.owned(&namespace, &id)?.snapshot.clone()))
            .await
    }

    pub async fn owned_page(
        &self,
        namespace: String,
        input: ListProcessingJobsInput,
    ) -> Result<ProcessingJobPage, ApplicationError> {
        validate_namespace(&namespace)?;
        let limit = input.limit.unwrap_or(50) as usize;
        if !(1..=200).contains(&limit) {
            return Err(rejected(
                ErrorCode::Validation,
                "Invalid processing job page size",
            ));
        }
        let after = input.after.map(|id| parse_uuid(&id)).transpose()?;
        if let Some(session) = &input.session_id {
            hypr_vault_read::paths::validate_session_id(session)
                .map_err(|_| rejected(ErrorCode::Validation, "Invalid job session"))?;
            if session.len() > 512 {
                return Err(rejected(ErrorCode::Validation, "Invalid job session"));
            }
        }
        self.run(move |state, _| {
            let after = after
                .as_ref()
                .map(|id| {
                    state
                        .owned(&namespace, id)
                        .map(|record| (&record.snapshot.created_at, &record.snapshot.id))
                })
                .transpose()?;
            let mut items: Vec<_> = state
                .records
                .values()
                .filter(|record| record.namespace == namespace)
                .map(|record| &record.snapshot)
                .filter(|job| {
                    input
                        .session_id
                        .as_ref()
                        .is_none_or(|session| &job.session_id == session)
                        && (!input.active_only || !job.state.is_terminal())
                        && after.is_none_or(|after| (&job.created_at, &job.id) < after)
                })
                .collect();
            items.sort_unstable_by(|a, b| (&b.created_at, &b.id).cmp(&(&a.created_at, &a.id)));
            let more = items.len() > limit;
            items.truncate(limit);
            let next_cursor = more.then(|| items.last().unwrap().id.clone());
            Ok(ProcessingJobPage {
                items: items.into_iter().cloned().collect(),
                next_cursor,
            })
        })
        .await
    }

    pub async fn cancel_owned(
        &self,
        namespace: String,
        id: String,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        validate_namespace(&namespace)?;
        let id = parse_uuid(&id)?;
        self.run(move |state, events| {
            let record = state.owned(&namespace, &id)?.clone();
            state.cancel_record(record, events)
        })
        .await
    }

    pub async fn owned_result(
        &self,
        namespace: String,
        id: String,
    ) -> Result<Option<ProcessingJobResult>, ApplicationError> {
        validate_namespace(&namespace)?;
        let id = parse_uuid(&id)?;
        self.run(move |state, _| {
            let record = state.owned(&namespace, &id)?;
            let output = super::result::read_result(state, record)?;
            Ok(output.map(|output| ProcessingJobResult {
                vault_id: state.vault_id.clone(),
                job_id: id,
                session_id: record.request.session_id.clone(),
                target: record.request.target.clone(),
                markdown: output.markdown,
                title: super::title_stage::project(state, record),
                suggested_tags: output.suggested_tags.map(|tags| {
                    tags.into_iter()
                        .map(|tag| ProcessingTagSuggestion {
                            name: tag.name,
                            confidence: tag.confidence,
                        })
                        .collect()
                }),
            }))
        })
        .await
    }
}

impl VaultContext {
    async fn readable_jobs(&self) -> Result<&JobJournal, ApplicationError> {
        self.store.ensure_ready().map_err(|_| {
            rejected(
                ErrorCode::Unavailable,
                "Processing is not available for this vault",
            )
        })?;
        let service = self.processing.get().ok_or_else(|| {
            rejected(
                ErrorCode::Unavailable,
                "Processing is not available for this vault",
            )
        })?;
        // An admission stop can precede actual worker drain. Only reconcile after
        // the task has exited, when no worker can still apply or retain output.
        if service.finished.is_cancelled() {
            service
                .journal
                .run(|state, events| state.interrupt_active(events))
                .await?;
        }
        Ok(&service.journal)
    }

    pub(crate) async fn submit_local_capture_summary(
        self: &std::sync::Arc<Self>,
        request_id: String,
        input: SummaryJobRequest,
        origin: crate::local_capture_intents::LocalCaptureChildOrigin,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        let context = self.clone();
        tokio::spawn(async move {
            if let Some(snapshot) = context
                .processing_jobs()?
                .replay_local_capture(request_id.clone(), input.clone(), origin.clone())
                .await?
            {
                return Ok(snapshot);
            }
            let _admission = context.admit_session_work(&input.session_id).await?;
            drop(
                context
                    .guard_local_capture_summary(
                        crate::local_capture_intents::SUMMARY_OWNER,
                        &request_id,
                        Some(&origin),
                    )
                    .await?,
            );
            let target = context
                .read_document(&input.session_id, &input.target)
                .await?;
            if target.revision != input.expected_revision
                || conditional_target_filled(&input, &target)
            {
                return Err(rejected(
                    ErrorCode::Conflict,
                    "Automatic summary target changed during capture",
                ));
            }
            context
                .processing_jobs()?
                .submit_with_origins(
                    crate::local_capture_intents::SUMMARY_OWNER.into(),
                    request_id,
                    input,
                    None,
                    Some(origin),
                )
                .await
        })
        .await
        .map_err(|_| storage())?
    }
    pub async fn submit_summary_job(
        self: &std::sync::Arc<Self>,
        namespace: String,
        request_id: String,
        input: SummaryJobRequest,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        self.submit_summary_inner(namespace, request_id, input, None)
            .await
    }
    pub(crate) async fn submit_capture_summary_job(
        self: &std::sync::Arc<Self>,
        namespace: String,
        request_id: String,
        input: SummaryJobRequest,
        origin: crate::capture_processing::CaptureChildOrigin,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        self.submit_summary_inner(namespace, request_id, input, Some(origin))
            .await
    }
    async fn submit_summary_inner(
        self: &std::sync::Arc<Self>,
        namespace: String,
        request_id: String,
        input: SummaryJobRequest,
        origin: Option<crate::capture_processing::CaptureChildOrigin>,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        let context = self.clone();
        tokio::spawn(async move {
            let journal = context.processing_jobs()?;
            if let Some(snapshot) = journal
                .replay_with_origin(
                    namespace.clone(),
                    request_id.clone(),
                    input.clone(),
                    origin.clone(),
                )
                .await?
            {
                return Ok(snapshot);
            }
            let _admission = context.admit_session_work(&input.session_id).await?;
            let exists = context
                .store
                .read_meta(&input.session_id)
                .await
                .map_err(|cause| crate::vault::storage_error(cause, false))?
                .is_some_and(|meta| meta.id == input.session_id);
            if !exists {
                return Err(rejected(ErrorCode::NotFound, "Session not found"));
            }
            if input.only_if_empty {
                let snapshot = context
                    .read_document(&input.session_id, &input.target)
                    .await?;
                if snapshot.revision != input.expected_revision
                    || conditional_target_filled(&input, &snapshot)
                {
                    return Err(rejected(
                        ErrorCode::Conflict,
                        "The document no longer matches the automatic generation request",
                    ));
                }
            }
            journal
                .submit_with_origin(namespace, request_id, input, origin)
                .await
        })
        .await
        .map_err(|_| {
            rejected(
                ErrorCode::Internal,
                "Processing admission could not be confirmed",
            )
        })?
    }

    pub async fn processing_job(
        &self,
        namespace: String,
        id: String,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        self.readable_jobs().await?.owned_job(namespace, id).await
    }

    pub async fn list_processing_jobs(
        &self,
        namespace: String,
        input: ListProcessingJobsInput,
    ) -> Result<ProcessingJobPage, ApplicationError> {
        self.readable_jobs()
            .await?
            .owned_page(namespace, input)
            .await
    }

    pub async fn cancel_processing_job(
        &self,
        namespace: String,
        id: String,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        self.readable_jobs()
            .await?
            .cancel_owned(namespace, id)
            .await
    }

    pub async fn processing_job_result(
        &self,
        namespace: String,
        id: String,
    ) -> Result<Option<ProcessingJobResult>, ApplicationError> {
        self.readable_jobs()
            .await?
            .owned_result(namespace, id)
            .await
    }
}

#[cfg(test)]
mod tests;
