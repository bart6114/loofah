use std::time::SystemTime;

use tokio::sync::watch;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceCaptureCompletionSnapshot {
    pub first_admitted_audio_observed_at: Option<SystemTime>,
    pub stop_requested_observed_at: Option<SystemTime>,
    pub resources_released_observed_at: Option<SystemTime>,
    pub supervisor_ended: bool,
    pub failure: Option<String>,
}

#[derive(Clone)]
pub struct DeviceCaptureCompletion(watch::Receiver<DeviceCaptureCompletionSnapshot>);

#[derive(Clone)]
pub struct DeviceCaptureReporter(watch::Sender<DeviceCaptureCompletionSnapshot>);

impl DeviceCaptureCompletion {
    pub fn channel() -> (DeviceCaptureReporter, Self) {
        let (tx, rx) = watch::channel(DeviceCaptureCompletionSnapshot::default());
        (DeviceCaptureReporter(tx), Self(rx))
    }

    pub fn current(&self) -> DeviceCaptureCompletionSnapshot {
        self.0.borrow().clone()
    }

    pub async fn changed(
        &mut self,
    ) -> Result<DeviceCaptureCompletionSnapshot, watch::error::RecvError> {
        self.0.changed().await?;
        Ok(self.0.borrow_and_update().clone())
    }
}

impl DeviceCaptureReporter {
    pub(crate) fn first_audio(&self, at: SystemTime) {
        self.0.send_modify(|state| {
            state.first_admitted_audio_observed_at.get_or_insert(at);
        });
    }

    pub(crate) fn stop_requested(&self) {
        self.0.send_modify(|state| {
            state
                .stop_requested_observed_at
                .get_or_insert_with(SystemTime::now);
        });
    }

    pub(crate) fn resources_released(&self) {
        self.0.send_modify(|state| {
            state
                .resources_released_observed_at
                .get_or_insert_with(SystemTime::now);
        });
    }

    pub(crate) fn supervisor_ended(&self, failure: Option<String>) {
        self.0.send_modify(|state| {
            state.supervisor_ended = true;
            if state.failure.is_none() {
                state.failure = failure;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn resource_and_supervisor_proofs_are_independent_and_observations_are_write_once() {
        let (reporter, mut completion) = DeviceCaptureCompletion::channel();
        let first = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(7);
        reporter.first_audio(first);
        reporter.first_audio(SystemTime::UNIX_EPOCH);
        reporter.supervisor_ended(Some("interrupted".into()));
        let state = completion.changed().await.unwrap();
        assert_eq!(state.first_admitted_audio_observed_at, Some(first));
        assert!(state.supervisor_ended);
        assert!(state.resources_released_observed_at.is_none());
        reporter.resources_released();
        let state = completion.changed().await.unwrap();
        assert!(state.resources_released_observed_at.is_some());
        assert_eq!(state.failure.as_deref(), Some("interrupted"));
        drop(reporter);
        assert!(completion.changed().await.is_err());
        assert_eq!(completion.current(), state);
    }
}
