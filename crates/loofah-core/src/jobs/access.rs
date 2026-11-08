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
                title: None,
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

    pub async fn submit_summary_job(
        &self,
        namespace: String,
        request_id: String,
        input: SummaryJobRequest,
    ) -> Result<ProcessingJobSnapshot, ApplicationError> {
        self.processing_jobs()?
            .submit(namespace, request_id, input)
            .await
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
