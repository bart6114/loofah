use super::*;

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct OperationalCounts {
    pub receiving: u64,
    pub sealed_unpublished: u64,
    pub published_retained: u64,
    pub retired: u64,
    pub retained_bytes: u64,
    pub quota_bytes: u64,
}
impl CaptureSpool {
    pub fn operational_counts(&self) -> Option<OperationalCounts> {
        let state = self.state.try_lock().ok()?;
        store::operational_counts(&state, self.limits.max_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn counts_report_unavailable_under_io_lock_without_reading_source_bytes() {
        let directory = tempfile::tempdir().unwrap();
        let spool = CaptureSpool::open(
            directory.path().canonicalize().unwrap().join("captures"),
            CaptureSpoolLimits::default(),
        )
        .await
        .unwrap();
        let initial = spool.operational_counts().unwrap();
        assert_eq!(initial.receiving, 0);
        assert_eq!(initial.retained_bytes, 0);
        assert!(initial.quota_bytes > 0);
        let locked = spool.state.lock().unwrap();
        assert!(spool.operational_counts().is_none());
        drop(locked);
        assert!(spool.operational_counts().is_some());
    }
}
