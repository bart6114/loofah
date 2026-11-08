use super::*;

pub struct BatchAdmission(pub(super) Admission);
pub struct LiveAdmission(pub(super) Admission);

#[derive(Clone, Copy)]
pub(super) enum Kind {
    Batch,
    Live(u8),
}

pub(super) struct Admission {
    pub inner: Arc<Inner>,
    pub cancellation: Arc<AtomicBool>,
    kind: Kind,
}

impl Drop for Admission {
    fn drop(&mut self) {
        {
            let mut state = self.inner.lock();
            match self.kind {
                Kind::Batch => state.batch_groups -= 1,
                Kind::Live(channels) => {
                    state.live_groups -= 1;
                    state.live_channels -= usize::from(channels);
                }
            }
            self.inner.publish(&state);
        }
        self.inner.maybe_release();
    }
}

impl WhisperResources {
    pub fn try_batch(&self) -> Result<BatchAdmission, ResourceError> {
        self.admit(Kind::Batch).map(BatchAdmission)
    }

    /// Reserves a workload group, not proof that a live engine has started or drained.
    /// A future live owner must retain this reservation through its actual native completion.
    pub fn try_live(&self, channels: u8) -> Result<LiveAdmission, ResourceError> {
        if !(1..=2).contains(&channels) {
            return Err(ResourceError::Invalid("live channel count"));
        }
        self.admit(Kind::Live(channels)).map(LiveAdmission)
    }

    fn admit(&self, kind: Kind) -> Result<Admission, ResourceError> {
        let mut state = self.inner.lock();
        if state.stopping {
            return Err(ResourceError::Stopping);
        }
        match &state.phase {
            ResourcePhase::Ready => {}
            ResourcePhase::Loading => return Err(ResourceError::Loading),
            _ => return Err(ResourceError::Unavailable),
        }
        if state.batch_groups + state.live_groups >= self.inner.limits.workload_groups {
            return Err(ResourceError::Busy);
        }
        let cancellation = Arc::new(AtomicBool::new(false));
        state.cancellations.retain(|value| value.strong_count() > 0);
        state.cancellations.push(Arc::downgrade(&cancellation));
        match kind {
            Kind::Batch => state.batch_groups += 1,
            Kind::Live(channels) => {
                state.live_groups += 1;
                state.live_channels += usize::from(channels);
            }
        }
        self.inner.publish(&state);
        Ok(Admission {
            inner: self.inner.clone(),
            cancellation,
            kind,
        })
    }
}

impl LiveAdmission {
    pub fn is_cancelled(&self) -> bool {
        self.0.cancellation.load(Ordering::Acquire)
    }
}
