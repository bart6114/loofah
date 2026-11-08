use super::*;
use owhisper_interface::stream::StreamResponse;
use std::collections::VecDeque;

const MAX_CAPTIONS: usize = 128;
const MAX_WORDS: usize = 16_384;
const MAX_TEXT: usize = 512 * 1024;
const MAX_CAPTION_TEXT: usize = 16 * 1024;
const MAX_CAPTION_WORDS: usize = 2048;

#[derive(Clone, Debug)]
pub struct LiveCaption {
    pub sequence: u64,
    pub physical_channel: u8,
    pub role: ChannelRole,
    /// Physical indexing is preserved here. Core owns canonical channel projection.
    pub response: Arc<StreamResponse>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LiveStatus {
    Starting,
    Open,
    Finishing,
    Interrupted(LiveFailure),
    Finished,
}

#[derive(Clone, Debug)]
pub struct LiveSnapshot {
    pub generation: [u8; 16],
    pub start_frame: u64,
    pub admitted_end_frame: u64,
    pub input_consumed_end_frame: u64,
    pub completed_through_frame: Option<u64>,
    pub status: LiveStatus,
    pub first_sequence: u64,
    pub next_sequence: u64,
    pub captions: Vec<Arc<LiveCaption>>,
}

impl LiveSnapshot {
    pub fn requires_reset(&self, generation: [u8; 16], next_sequence: u64) -> bool {
        generation != self.generation
            || next_sequence < self.first_sequence
            || next_sequence > self.next_sequence
    }
}

#[derive(Clone)]
pub struct LiveObserver(pub(super) watch::Receiver<Arc<LiveSnapshot>>);

impl LiveObserver {
    pub fn snapshot(&self) -> Arc<LiveSnapshot> {
        self.0.borrow().clone()
    }
    pub async fn changed(&mut self) -> Option<Arc<LiveSnapshot>> {
        self.0.changed().await.ok()?;
        Some(self.snapshot())
    }
}

#[derive(Default)]
pub(super) struct Window {
    captions: VecDeque<(Arc<LiveCaption>, usize, usize)>,
    words: usize,
    bytes: usize,
    pub next_sequence: u64,
    pub completed: Option<u64>,
}

impl Shared {
    pub(super) fn publish(&self, state: &Ingress) {
        let status = if let Some(error) = &state.failure {
            LiveStatus::Interrupted(error.clone())
        } else {
            match state.phase {
                Phase::Starting => LiveStatus::Starting,
                Phase::Open => LiveStatus::Open,
                Phase::Finishing => LiveStatus::Finishing,
                Phase::Stopping => LiveStatus::Interrupted(LiveFailure::Cancelled),
                Phase::Terminal => LiveStatus::Finished,
            }
        };
        self.status.send_replace(Arc::new(LiveSnapshot {
            generation: self.spec.generation,
            start_frame: self.spec.start_frame,
            admitted_end_frame: state.admitted_end,
            input_consumed_end_frame: state.consumed_end,
            completed_through_frame: state.window.completed,
            status,
            first_sequence: state
                .window
                .captions
                .front()
                .map_or(state.window.next_sequence, |item| item.0.sequence),
            next_sequence: state.window.next_sequence,
            captions: state
                .window
                .captions
                .iter()
                .map(|item| item.0.clone())
                .collect(),
        }));
    }

    pub(super) fn caption(
        &self,
        channel: usize,
        segment: &crate::service::Segment,
        finalized: bool,
        metadata: &owhisper_interface::stream::Metadata,
        results: &mpsc::Sender<LiveCaption>,
    ) -> Result<(), LiveFailure> {
        let role = self
            .spec
            .channels
            .role(channel)
            .ok_or(LiveFailure::InvalidOutput)?;
        let words = segment.text.split_whitespace().count();
        let bytes = segment.text.len();
        if words > MAX_CAPTION_WORDS
            || bytes > MAX_CAPTION_TEXT
            || !segment.start.is_finite()
            || !segment.duration.is_finite()
            || !segment.confidence.is_finite()
            || segment.start < 0.0
            || segment.duration < 0.0
        {
            return Err(LiveFailure::InvalidOutput);
        }
        let response = Arc::new(crate::service::live_response(
            segment,
            finalized,
            metadata,
            channel,
            self.spec.channels.count(),
        ));
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if state.failure.is_some() {
            return Err(state.failure.clone().unwrap());
        }
        if segment.start < self.spec.start_frame as f64 / 16000.0
            || segment.start + segment.duration
                > state.admitted_end as f64 / 16000.0 + 1.0 / 16000.0
        {
            return Err(LiveFailure::InvalidOutput);
        }
        let caption = LiveCaption {
            sequence: state.window.next_sequence,
            physical_channel: channel as u8,
            role,
            response,
        };
        // There is no full terminal transcript to recover dropped live words from.
        results
            .try_send(caption.clone())
            .map_err(|_| LiveFailure::OutputLost)?;
        state.window.next_sequence = state
            .window
            .next_sequence
            .checked_add(1)
            .ok_or(LiveFailure::InvalidOutput)?;
        state.window.words += words;
        state.window.bytes += bytes;
        state
            .window
            .captions
            .push_back((Arc::new(caption), words, bytes));
        while state.window.captions.len() > MAX_CAPTIONS
            || state.window.words > MAX_WORDS
            || state.window.bytes > MAX_TEXT
        {
            let (_, words, bytes) = state.window.captions.pop_front().unwrap();
            state.window.words -= words;
            state.window.bytes -= bytes;
        }
        self.publish(&state);
        Ok(())
    }
}
