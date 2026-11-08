use super::*;
use std::collections::VecDeque;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

const RECENT_BLOCKS: usize = 64;
pub(super) const INPUT_BLOCKS: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveIngressError {
    Busy,
    Invalid,
    Closed,
    Gap { expected_frame: u64 },
    ChangedReplay,
    Overlap,
    UnknownReplay,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameReceipt {
    Accepted { admitted_end_frame: u64 },
    Duplicate { admitted_end_frame: u64 },
}

struct Recent {
    first: u64,
    frames: u64,
    hash: [u8; 32],
}

pub(super) struct Frame {
    pub first: u64,
    pub samples: Arc<[f32]>,
    pub permits: Vec<OwnedSemaphorePermit>,
}

pub(super) struct Ingress {
    recent: VecDeque<Recent>,
    pub phase: Phase,
    pub admitted_end: u64,
    pub consumed_end: u64,
    pub finish_end: Option<u64>,
    pub failure: Option<LiveFailure>,
    pub window: super::snapshot::Window,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Starting,
    Open,
    Finishing,
    Stopping,
    Terminal,
}

impl Ingress {
    pub fn new(start: u64) -> Self {
        Self {
            recent: VecDeque::new(),
            phase: Phase::Starting,
            admitted_end: start,
            consumed_end: start,
            finish_end: None,
            failure: None,
            window: Default::default(),
        }
    }
}

impl LiveController {
    /// Bounded CPU validation/hash only: no waiting, filesystem or network work.
    /// The caller retains its Arc on every outcome, including Busy.
    pub fn try_accept_frame(
        &self,
        first_frame: u64,
        samples: &Arc<[f32]>,
    ) -> Result<FrameReceipt, LiveIngressError> {
        use sha2::{Digest, Sha256};
        let count = self.shared.spec.channels.count();
        if samples.is_empty()
            || !samples.len().is_multiple_of(count)
            || samples.len() / count > crate::service::live_pipeline::MAX_MESSAGE_SAMPLES
        {
            return Err(LiveIngressError::Invalid);
        }
        let frames = (samples.len() / count) as u64;
        let end = first_frame
            .checked_add(frames)
            .filter(|end| *end <= MAX_FRAME)
            .ok_or(LiveIngressError::Invalid)?;
        let mut hash = Sha256::new();
        for sample in samples.iter() {
            if !sample.is_finite() {
                return Err(LiveIngressError::Invalid);
            }
            hash.update(sample.to_bits().to_le_bytes());
        }
        let hash: [u8; 32] = hash.finalize().into();
        let mut state = self
            .shared
            .state
            .try_lock()
            .map_err(|_| LiveIngressError::Busy)?;
        if state.phase == Phase::Starting {
            return Err(LiveIngressError::Busy);
        }
        if !matches!(state.phase, Phase::Open | Phase::Finishing) {
            return Err(LiveIngressError::Closed);
        }
        if first_frame < state.admitted_end {
            if let Some(prior) = state.recent.iter().find(|item| item.first == first_frame) {
                return if prior.frames == frames && prior.hash == hash {
                    Ok(FrameReceipt::Duplicate {
                        admitted_end_frame: state.admitted_end,
                    })
                } else {
                    Err(LiveIngressError::ChangedReplay)
                };
            }
            if state
                .recent
                .front()
                .is_some_and(|item| first_frame < item.first)
            {
                return Err(LiveIngressError::UnknownReplay);
            }
            return Err(LiveIngressError::Overlap);
        }
        if state.phase != Phase::Open {
            return Err(LiveIngressError::Closed);
        }
        if first_frame != state.admitted_end {
            return Err(LiveIngressError::Gap {
                expected_frame: state.admitted_end,
            });
        }
        let queue = self.frames.try_reserve().map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => LiveIngressError::Busy,
            mpsc::error::TrySendError::Closed(_) => LiveIngressError::Closed,
        })?;
        // One reservation covers every physical channel before any cursor changes.
        let permits = self
            .shared
            .budgets
            .iter()
            .map(|budget| {
                budget
                    .clone()
                    .try_acquire_many_owned(frames as u32)
                    .map_err(|_| LiveIngressError::Busy)
            })
            .collect::<Result<Vec<_>, _>>()?;
        state.recent.push_back(Recent {
            first: first_frame,
            frames,
            hash,
        });
        if state.recent.len() > RECENT_BLOCKS {
            state.recent.pop_front();
        }
        state.admitted_end = end;
        queue.send(Frame {
            first: first_frame,
            samples: samples.clone(),
            permits,
        });
        self.shared.publish(&state);
        Ok(FrameReceipt::Accepted {
            admitted_end_frame: end,
        })
    }

    pub fn finish(&self, expected_end_frame: u64) -> Result<(), LiveIngressError> {
        let mut state = self
            .shared
            .state
            .try_lock()
            .map_err(|_| LiveIngressError::Busy)?;
        if state.finish_end == Some(expected_end_frame) && state.phase == Phase::Finishing {
            return Ok(());
        }
        if !matches!(state.phase, Phase::Starting | Phase::Open) {
            return Err(LiveIngressError::Closed);
        }
        if expected_end_frame != state.admitted_end {
            return Err(LiveIngressError::Gap {
                expected_frame: state.admitted_end,
            });
        }
        state.finish_end = Some(expected_end_frame);
        state.phase = Phase::Finishing;
        self.shared.publish(&state);
        self.shared.control.notify_one();
        Ok(())
    }
}

impl OwnedLive {
    pub fn try_accept_frame(
        &self,
        first_frame: u64,
        samples: &Arc<[f32]>,
    ) -> Result<FrameReceipt, LiveIngressError> {
        self.controller().try_accept_frame(first_frame, samples)
    }

    pub fn finish(&self, expected_end_frame: u64) -> Result<(), LiveIngressError> {
        self.controller().finish(expected_end_frame)
    }
}

pub(super) fn budgets(channels: usize) -> Vec<Arc<Semaphore>> {
    (0..channels)
        .map(|_| Arc::new(Semaphore::new(crate::service::live_pipeline::AUDIO_BUDGET)))
        .collect()
}
