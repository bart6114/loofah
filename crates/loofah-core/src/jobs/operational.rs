use super::*;
use crate::operational::{JobCounts, Sample};

impl JobJournal {
    pub(crate) fn operational_sample(&self) -> Sample<JobCounts> {
        let Ok(state) = self.0.state.try_lock() else {
            return Sample::Unavailable;
        };
        if state.failed {
            return Sample::Unavailable;
        }
        if state.records.len() > MAX_JOBS {
            return Sample::Incomplete;
        }
        let mut counts = JobCounts::default();
        for record in state.records.values() {
            let snapshot = &record.snapshot;
            match snapshot.state {
                ProcessingJobState::Queued => counts.queued += 1,
                ProcessingJobState::Running | ProcessingJobState::Cancelling => counts.running += 1,
                terminal => {
                    counts.terminal += 1;
                    counts.failed += u64::from(terminal == ProcessingJobState::Failed);
                    counts.interrupted += u64::from(terminal == ProcessingJobState::Interrupted);
                    counts.terminal_latency(&snapshot.created_at, &snapshot.updated_at);
                }
            }
        }
        Sample::Available(counts)
    }
}
