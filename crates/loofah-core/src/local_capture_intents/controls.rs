use super::*;
use std::{future::Future, task::Poll};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(rename_all = "snake_case")]
pub enum LocalCaptureAction {
    Cancel,
    Recover,
    KeepCurrentContent,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct LocalCaptureControlSnapshot {
    pub status: LocalCaptureStatus,
    pub revision: String,
    pub actions: Vec<LocalCaptureAction>,
    pub resolution: Option<LocalCaptureResolution>,
}
impl LocalCaptureIntents {
    pub async fn controls(
        &self,
        session: Option<&str>,
    ) -> Result<Vec<LocalCaptureControlSnapshot>, ApplicationError> {
        let snapshots: Vec<_> = self
            .state
            .lock()
            .await
            .values()
            .filter(|entry| {
                session.is_none_or(|session| entry.record.status.declaration.session_id == session)
            })
            .map(|entry| {
                (
                    entry.record.clone(),
                    entry.running,
                    entry.admission.is_some(),
                )
            })
            .collect();
        let mut result = Vec::with_capacity(snapshots.len());
        for (record, running, admitted) in snapshots {
            result.push(self.control_snapshot(record, running, admitted).await?);
        }
        Ok(result)
    }
    pub async fn control_status(
        &self,
        request: &str,
    ) -> Result<LocalCaptureControlSnapshot, ApplicationError> {
        let (record, running, admitted) = {
            let state = self.state.lock().await;
            let entry = state
                .get(request)
                .ok_or_else(|| review("Original intent missing"))?;
            (
                entry.record.clone(),
                entry.running,
                entry.admission.is_some(),
            )
        };
        self.control_snapshot(record, running, admitted).await
    }
    async fn control_snapshot(
        &self,
        record: Record,
        running: bool,
        admitted: bool,
    ) -> Result<LocalCaptureControlSnapshot, ApplicationError> {
        let mut actions = Vec::new();
        if !self.stop.is_cancelled() {
            if resolution::closable(&record) && !running && !admitted {
                actions.push(LocalCaptureAction::KeepCurrentContent);
            }
            if cancellable(&record, running) {
                actions.push(LocalCaptureAction::Cancel);
            }
            if recoverable(&record) && !running && !admitted {
                let child_can_progress =
                    if let (Some(summary), Some(source)) = (&record.summary, &record.source) {
                        match self.context.processing_jobs() {
                            Ok(journal) => journal
                                .replay_local_capture(
                                    record.status.declaration.request_id.clone(),
                                    summary.clone(),
                                    LocalCaptureChildOrigin {
                                        request_id: record.status.declaration.request_id.clone(),
                                        session_id: record.status.declaration.session_id.clone(),
                                        source: source.clone(),
                                    },
                                )
                                .await
                                .is_ok_and(|child| {
                                    child.is_none_or(|child| {
                                        !child.state.is_terminal()
                                        || child.state
                                            == hypr_loofah_protocol::ProcessingJobState::Succeeded
                                    })
                                }),
                            Err(_) => false,
                        }
                    } else {
                        true
                    };
                if child_can_progress {
                    actions.push(LocalCaptureAction::Recover);
                }
            }
        }
        Ok(LocalCaptureControlSnapshot {
            revision: control_revision(&record)?,
            resolution: record.resolution,
            status: record.status,
            actions,
        })
    }
    pub async fn cancel(
        self: &Arc<Self>,
        request: String,
    ) -> Result<LocalCaptureControlSnapshot, ApplicationError> {
        uuid(&request)?;
        let service = self.clone();
        tokio::spawn(async move { service.cancel_owned(request).await })
            .await
            .map_err(review)?
    }
    #[cfg(test)]
    pub(crate) async fn cancel_for_test(
        &self,
        request: String,
    ) -> Result<LocalCaptureControlSnapshot, ApplicationError> {
        self.cancel_owned(request).await
    }
    pub(super) async fn cancel_owned(
        &self,
        request: String,
    ) -> Result<LocalCaptureControlSnapshot, ApplicationError> {
        let (_, dispatch) = self.processing_control(&request).await?;
        let _dispatch = dispatch.lock().await;
        let mut state = self.state.lock().await;
        let entry = state
            .get_mut(&request)
            .ok_or_else(|| review("Original intent missing"))?;
        if !entry.record.cancel_requested {
            if !cancellable(&entry.record, entry.running) {
                return Err(review(
                    "Only active post-capture processing can be cancelled",
                ));
            }
            let mut record = entry.record.clone();
            record.version = 2;
            record.cancel_requested = true;
            record.processing_drained = false;
            record.status.state = LocalCaptureState::Cancelling;
            record.status.message =
                Some("Cancellation requested; waiting for original processing resources".into());
            self.persist(&record, false).await?;
            entry.record = record;
        }
        entry.cancel.cancel();
        drop(state);
        drop(_dispatch);
        self.changed.notify_waiters();
        self.control_status(&request).await
    }
    pub(super) async fn processing_control(
        &self,
        request: &str,
    ) -> Result<(CancellationToken, Arc<Mutex<()>>), ApplicationError> {
        let state = self.state.lock().await;
        let entry = state
            .get(request)
            .ok_or_else(|| review("Original intent missing"))?;
        Ok((entry.cancel.clone(), entry.dispatch.clone()))
    }
    pub(super) async fn check_processing(&self, request: &str) -> Result<(), ApplicationError> {
        let state = self.state.lock().await;
        let entry = state
            .get(request)
            .ok_or_else(|| review("Original intent missing"))?;
        if self.stop.is_cancelled() || entry.record.cancel_requested || entry.cancel.is_cancelled()
        {
            return Err(review("Original processing was cancelled before admission"));
        }
        Ok(())
    }
    // The request is frozen before the first poll, not through response/model I/O.
    pub(super) async fn dispatch_processing<T>(
        &self,
        request: &str,
        change: impl FnOnce(&mut Record),
        future: impl Future<Output = Result<T, ApplicationError>>,
    ) -> Result<T, ApplicationError> {
        let (_, dispatch) = self.processing_control(request).await?;
        let gate = dispatch.lock().await;
        self.check_processing(request).await?;
        self.update(request, change).await?;
        tokio::pin!(future);
        let first = std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await;
        drop(gate);
        match first {
            Poll::Ready(value) => value,
            Poll::Pending => future.await,
        }
    }
}
fn cancellable(record: &Record, running: bool) -> bool {
    running
        && !record.cancel_requested
        && record.physical == Some(Ok(()))
        && matches!(record.persistence, Some(Ok(_)))
        && matches!(
            record.status.state,
            LocalCaptureState::WaitingForPersistence
                | LocalCaptureState::Transcribing
                | LocalCaptureState::Summarizing
        )
}
pub(super) fn recoverable(record: &Record) -> bool {
    record.status.state == LocalCaptureState::Review
        && !record.cancel_requested
        && record.physical == Some(Ok(()))
        && matches!(record.persistence, Some(Ok(_)))
        && (!record.inference_dispatched || record.result_sha256.is_some())
}

pub(super) fn control_revision(record: &Record) -> Result<String, ApplicationError> {
    Ok(ledger::hash(&ledger::encode_bounded_result(
        record,
        128 * 1024,
    )?))
}
