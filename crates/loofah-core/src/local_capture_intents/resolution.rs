use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LocalCaptureResolutionDecision {
    KeepCurrentContent,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct LocalCaptureResolution {
    pub resolution_id: String,
    pub expected_revision: String,
    pub decision: LocalCaptureResolutionDecision,
}

impl LocalCaptureIntents {
    pub async fn resolve(
        self: &Arc<Self>,
        request: String,
        resolution: LocalCaptureResolution,
    ) -> Result<LocalCaptureControlSnapshot, ApplicationError> {
        uuid(&request)?;
        validate_resolution(&resolution)?;
        let service = self.clone();
        tokio::spawn(async move { service.resolve_owned(request, resolution).await })
            .await
            .map_err(review)?
    }

    async fn resolve_owned(
        &self,
        request: String,
        resolution: LocalCaptureResolution,
    ) -> Result<LocalCaptureControlSnapshot, ApplicationError> {
        let original = self.read_original(&request).await?;
        if original.archived {
            if !replay(&original.record, &resolution)? {
                return Err(review("Archived processing has no matching resolution"));
            }
            drop(original);
            return self.control_status(&request).await;
        }
        let (_, dispatch) = self.processing_control(&request).await?;
        let _dispatch = dispatch.lock().await;
        drop(original);
        let record = {
            let state = self.state.lock().await;
            let entry = state
                .get(&request)
                .ok_or_else(|| review("Original intent missing"))?;
            if replay(&entry.record, &resolution)? {
                drop(state);
                drop(_dispatch);
                return self.control_status(&request).await;
            }
            eligible(entry, &resolution)?;
            entry.record.clone()
        };
        if self.stop.is_cancelled() {
            return Err(review("Local capture owner is stopping"));
        }
        let session = &record.status.declaration.session_id;
        let _lifecycle = self.context.try_session_lifecycle_change(session)?;
        self.context.ensure_session_jobs_idle(session).await?;
        if self.context.read_metadata(session).await?.meta.created_at != record.session_created_at {
            return Err(review("Original Local session generation changed"));
        }
        let mut state = self.state.lock().await;
        let entry = state
            .get_mut(&request)
            .ok_or_else(|| review("Original intent missing"))?;
        eligible(entry, &resolution)?;
        if self.stop.is_cancelled() {
            return Err(review("Local capture owner is stopping"));
        }
        let mut next = entry.record.clone();
        next.version = 3;
        next.resolution = Some(resolution);
        next.status.state = LocalCaptureState::Closed;
        next.status.message = Some("Processing closed by review; current content, original audio, retained output and journals were preserved".into());
        self.persist(&next, false).await?;
        entry.record = next;
        drop(state);
        drop(_dispatch);
        self.changed.notify_waiters();
        self.control_status(&request).await
    }
}

fn replay(record: &Record, resolution: &LocalCaptureResolution) -> Result<bool, ApplicationError> {
    match &record.resolution {
        Some(original) if original == resolution => Ok(true),
        Some(_) => Err(error(
            ErrorCode::Conflict,
            "Original Local processing resolution changed",
        )),
        None => Ok(false),
    }
}
fn eligible(entry: &Entry, resolution: &LocalCaptureResolution) -> Result<(), ApplicationError> {
    if entry.running || entry.admission.is_some() || !closable(&entry.record) {
        return Err(review(
            "Only confirmed-drained processing conflicts can be closed",
        ));
    }
    let revision = controls::control_revision(&entry.record)?;
    if revision != resolution.expected_revision {
        let mut failure = error(
            ErrorCode::Conflict,
            "Local processing changed; review the current state",
        );
        failure.current_revision = Some(revision);
        failure.mutation_outcome = Some(hypr_loofah_protocol::MutationOutcome::NotCommitted);
        return Err(failure);
    }
    Ok(())
}
pub(super) fn closable(record: &Record) -> bool {
    record.status.state == LocalCaptureState::Conflict
        && record.resolution.is_none()
        && !record.cancel_requested
        && confirmed_drain(record)
}
pub(super) fn confirmed_drain(record: &Record) -> bool {
    record.processing_drained
        && record.physical == Some(Ok(()))
        && matches!(record.persistence, Some(Ok(_)))
        && (!record.inference_dispatched || record.inference_drained)
        && !record.summary_dispatch_pending
        && (record.status.summary_job_id.is_none() || record.summary_drained)
}
pub(super) fn validate_resolution(
    resolution: &LocalCaptureResolution,
) -> Result<(), ApplicationError> {
    uuid(&resolution.resolution_id)?;
    if !hypr_loofah_protocol::is_transcription_sha256(&resolution.expected_revision) {
        return Err(error(
            ErrorCode::Validation,
            "Invalid Local processing control revision",
        ));
    }
    Ok(())
}
