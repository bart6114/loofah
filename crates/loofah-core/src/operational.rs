use serde::Serialize;

use super::VaultContext;

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum Sample<T> {
    Available(T),
    Disabled,
    Unavailable,
    Incomplete,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct JobCounts {
    pub queued: u64,
    pub running: u64,
    pub terminal: u64,
    pub failed: u64,
    pub interrupted: u64,
    pub invalid_timestamps: u64,
    pub latency_ms: LatencyHistogram,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct LatencyHistogram {
    pub le_100: u64,
    pub le_1000: u64,
    pub le_10000: u64,
    pub le_60000: u64,
    pub le_300000: u64,
    pub le_1800000: u64,
    pub total: u64,
}
impl LatencyHistogram {
    pub fn observe(&mut self, milliseconds: u64) {
        self.total = self.total.saturating_add(1);
        for (bound, count) in [
            (100, &mut self.le_100),
            (1000, &mut self.le_1000),
            (10000, &mut self.le_10000),
            (60000, &mut self.le_60000),
            (300000, &mut self.le_300000),
            (1800000, &mut self.le_1800000),
        ] {
            if milliseconds <= bound {
                *count = count.saturating_add(1);
            }
        }
    }
}
impl JobCounts {
    pub(crate) fn terminal_latency(&mut self, created: &str, updated: &str) {
        let duration = chrono::DateTime::parse_from_rfc3339(created)
            .ok()
            .zip(chrono::DateTime::parse_from_rfc3339(updated).ok())
            .and_then(|(created, updated)| {
                u64::try_from((updated - created).num_milliseconds()).ok()
            });
        if let Some(duration) = duration {
            self.latency_ms.observe(duration);
        } else {
            self.invalid_timestamps += 1;
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MediaUploadCounts {
    pub receiving: u64,
    pub ready: u64,
    pub dispatched: u64,
    pub terminal: u64,
    pub review: u64,
    pub declared_bytes: u64,
}

#[derive(Clone, Debug, Serialize)]
pub struct OperationalSnapshot {
    pub media_uploads: Sample<MediaUploadCounts>,
    pub capture_uploads: Sample<hypr_capture_store::capture_spool::OperationalCounts>,
    pub summary_jobs: Sample<JobCounts>,
    pub transcription_jobs: Sample<JobCounts>,
}
impl VaultContext {
    pub async fn sample_operational_media(
        self: &std::sync::Arc<Self>,
    ) -> Sample<MediaUploadCounts> {
        let pin = self.clone();
        tokio::task::spawn_blocking(move || crate::local_cli::sample_media_uploads(&pin))
            .await
            .unwrap_or(Sample::Unavailable)
    }
    /// In-memory, bounded, nonblocking observations. No content, model work or disk traversal.
    pub fn operational_snapshot(&self) -> OperationalSnapshot {
        OperationalSnapshot {
            media_uploads: Sample::Unavailable,
            capture_uploads: self
                .capture_ingestion
                .get()
                .map_or(Sample::Disabled, |capture| capture.operational_sample()),
            summary_jobs: self.processing.get().map_or(Sample::Disabled, |service| {
                service.journal.operational_sample()
            }),
            transcription_jobs: self
                .transcription
                .get()
                .map_or(Sample::Disabled, |service| service.operational_sample()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn job_latency_is_fixed_bounded_and_invalid_clock_data_is_explicit() {
        let mut counts = JobCounts::default();
        counts.terminal_latency("2026-11-08T10:00:00Z", "2026-11-08T10:00:01Z");
        counts.terminal_latency("2026-11-08T10:00:01Z", "2026-11-08T10:00:00Z");
        counts.terminal_latency("PRIVATE-NOTE-CANARY", "SECRET-PATH-CANARY");
        assert_eq!(counts.latency_ms.total, 1);
        assert_eq!(counts.latency_ms.le_100, 0);
        assert_eq!(counts.latency_ms.le_1000, 1);
        assert_eq!(counts.invalid_timestamps, 2);
        assert!(!serde_json::to_string(&counts).unwrap().contains("CANARY"));
    }
}
