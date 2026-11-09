use super::*;
use hypr_vault_write::transcript_publication::TranscriptPublicationOutcome;

impl LocalCaptureIntents {
    /// Reconciles retained output for the original intent. An unknown dispatch
    /// without output never starts a second model call.
    pub async fn recover(
        self: &Arc<Self>,
        request: String,
    ) -> Result<LocalCaptureStatus, ApplicationError> {
        uuid(&request)?;
        let this = self.clone();
        tokio::spawn(async move {
            if this.stop.is_cancelled() {
                return Err(review("Local capture owner is stopping"));
            }
            let record = this.read_original(&request).await?.record;
            if record.cancel_requested {
                return Err(review(
                    "Cancelled work cannot dispatch again; unresolved ownership requires review",
                ));
            }
            if !unresolved(&record) {
                return Ok(record.status);
            }
            if !this
                .control_status(&request)
                .await?
                .actions
                .contains(&LocalCaptureAction::Recover)
            {
                return Err(review(
                    "This original intent has no recoverable processing step",
                ));
            }
            if record.physical != Some(Ok(())) || !matches!(record.persistence, Some(Ok(_))) {
                return Err(review(
                    "Original producer/persistence completion cannot be reconstructed",
                ));
            }
            if record.inference_dispatched && record.result_sha256.is_none() {
                return Err(review(
                    "Inference outcome unknown; retained output is required",
                ));
            }
            let admission = this
                .context
                .admit_session_work(&record.status.declaration.session_id)
                .await?;
            let mut state = this.state.lock().await;
            let entry = state.get_mut(&request).unwrap();
            if this.stop.is_cancelled() || entry.running || entry.admission.is_some() {
                return Err(review("Original Local intent is still active"));
            }
            let mut next = entry.record.clone();
            next.version = 2;
            next.status.state = LocalCaptureState::WaitingForPersistence;
            next.processing_drained = false;
            this.persist(&next, false).await?;
            entry.record = next;
            entry.admission = Some(Arc::new(admission));
            entry.running = true;
            entry.cancel = this.stop.child_token();
            drop(state);
            this.clone().start(request.clone());
            this.status(&request).await
        })
        .await
        .map_err(review)?
    }
    pub(super) async fn recover_publication(
        &self,
        request: &str,
    ) -> Result<Option<Record>, ApplicationError> {
        let root = self
            .root
            .parent()
            .ok_or_else(|| review("Operation storage missing"))?
            .join("transcript-publications");
        let Some(plan) = self
            .context
            .store
            .prepare_transcript_publication_status(&root, self.publication_identity(request)?)
            .await
            .map_err(|e| storage_error(e, true))?
        else {
            return Ok(None);
        };
        let plan = plan.recheck().await.map_err(|e| storage_error(e, true))?;
        #[cfg(test)]
        self.pause_publication(tests::PublicationPoint::RecoveryBeforeCommit)
            .await;
        let (_, dispatch) = self.processing_control(request).await?;
        let commit_guard = dispatch.lock().await;
        self.check_processing(request).await?;
        let outcome = plan.commit().await.map_err(|e| storage_error(e, true))?;
        let result = match outcome {
            TranscriptPublicationOutcome::Published {
                revision,
                transcript_id,
            }
            | TranscriptPublicationOutcome::AlreadyCommitted {
                revision,
                transcript_id,
            } => self
                .update(request, |record| {
                    record.published_revision = Some(revision.clone());
                    record.inference_drained = true;
                    record.status.transcript_id = Some(transcript_id);
                    if let Some(source) = &mut record.source {
                        source.transcript_revision = revision;
                        source.transcript_present = true;
                    }
                })
                .await
                .map(Some),
            _ => Err(review(format!(
                "Original transcript publication remains unresolved: {outcome:?}"
            ))),
        };
        #[cfg(test)]
        self.pause_publication(tests::PublicationPoint::RecoveryAfterCommit)
            .await;
        drop(commit_guard);
        result
    }
}
