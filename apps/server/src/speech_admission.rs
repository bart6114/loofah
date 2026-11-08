use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum AdmissionError {
    Busy,
    Cancelled,
    Stopped,
}

pub(crate) struct SpeechAdmission {
    total: u32,
    slots: Arc<Semaphore>,
    batch: Option<Arc<Semaphore>>,
    live: Option<Arc<Semaphore>>,
    waiting: Arc<Semaphore>,
    stopped: CancellationToken,
}

// The permit follows the actual native/source owner, never just its async waiter.
pub(crate) struct SpeechPermit {
    admission: Arc<SpeechAdmission>,
    _total: OwnedSemaphorePermit,
    _class: Option<OwnedSemaphorePermit>,
}
impl Drop for SpeechPermit {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.admission.stop();
        }
    }
}

impl SpeechAdmission {
    pub(crate) fn new(
        total: usize,
        reserved_live: usize,
        waiting: usize,
    ) -> Result<Arc<Self>, &'static str> {
        if !(1..=8).contains(&total) || reserved_live >= total || waiting > 16 {
            return Err("Invalid CPU speech admission bounds");
        }
        Ok(Arc::new(Self {
            total: total as u32,
            slots: Arc::new(Semaphore::new(total)),
            batch: (reserved_live > 0).then(|| Arc::new(Semaphore::new(total - reserved_live))),
            live: (reserved_live > 0).then(|| Arc::new(Semaphore::new(reserved_live))),
            waiting: Arc::new(Semaphore::new(waiting)),
            stopped: CancellationToken::new(),
        }))
    }
    pub(crate) fn total(&self) -> usize {
        self.total as usize
    }
    pub(crate) fn is_stopped(&self) -> bool {
        self.stopped.is_cancelled()
    }
    pub(crate) fn is_busy(&self) -> bool {
        self.slots.available_permits() == 0
    }
    pub(crate) fn stop(&self) {
        self.stopped.cancel();
    }
    pub(crate) fn reserve_loading(&self) -> OwnedSemaphorePermit {
        self.slots
            .clone()
            .try_acquire_many_owned(self.total)
            .expect("new CPU host capacity")
    }
    pub(crate) async fn drain(&self) -> OwnedSemaphorePermit {
        self.slots
            .clone()
            .acquire_many_owned(self.total)
            .await
            .expect("owned CPU host capacity")
    }
    fn check(&self, cancel: &CancellationToken) -> Result<(), AdmissionError> {
        if self.is_stopped() {
            Err(AdmissionError::Stopped)
        } else if cancel.is_cancelled() {
            Err(AdmissionError::Cancelled)
        } else {
            Ok(())
        }
    }
    pub(crate) async fn batch(
        self: &Arc<Self>,
        cancel: &CancellationToken,
    ) -> Result<SpeechPermit, AdmissionError> {
        self.check(cancel)?;
        // Tokio's semaphore queue is FIFO. The separate waiting permit bounds
        // callers before any source decoding or model preparation is admitted.
        let queue = self.batch.as_ref().unwrap_or(&self.slots).clone();
        let class_or_total = match queue.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(_) => {
                let _waiting = self
                    .waiting
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| AdmissionError::Busy)?;
                tokio::select! {
                    biased;
                    _ = self.stopped.cancelled() => return Err(AdmissionError::Stopped),
                    _ = cancel.cancelled() => return Err(AdmissionError::Cancelled),
                    permit = queue.acquire_owned() => permit.map_err(|_| AdmissionError::Stopped)?,
                }
            }
        };
        let (total, class) = if self.batch.is_some() {
            let total = tokio::select! {
                biased;
                _ = self.stopped.cancelled() => return Err(AdmissionError::Stopped),
                _ = cancel.cancelled() => return Err(AdmissionError::Cancelled),
                permit = self.slots.clone().acquire_owned() => permit.map_err(|_| AdmissionError::Stopped)?,
            };
            (total, Some(class_or_total))
        } else {
            (class_or_total, None)
        };
        self.check(cancel)?;
        Ok(SpeechPermit {
            admission: self.clone(),
            _total: total,
            _class: class,
        })
    }
    pub(crate) fn try_live(self: &Arc<Self>) -> Result<SpeechPermit, AdmissionError> {
        self.check(&CancellationToken::new())?;
        let class = self
            .live
            .as_ref()
            .map(|slots| {
                slots
                    .clone()
                    .try_acquire_owned()
                    .map_err(|_| AdmissionError::Busy)
            })
            .transpose()?;
        let total = self
            .slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| AdmissionError::Busy)?;
        self.check(&CancellationToken::new())?;
        Ok(SpeechPermit {
            admission: self.clone(),
            _total: total,
            _class: class,
        })
    }
}

#[cfg(test)]
mod tests;
