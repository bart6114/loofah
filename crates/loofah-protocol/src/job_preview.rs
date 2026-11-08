use serde::{Deserialize, Serialize};

use crate::ProcessingJobSnapshot;

pub const MAX_PROCESSING_PREVIEW_BYTES: usize = 64 * 1024;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct ProcessingPreviewSnapshot {
    pub cursor: String,
    pub markdown: String,
    pub truncated: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, specta::Type)]
#[serde(deny_unknown_fields)]
pub struct ProcessingJobPreview {
    pub job: ProcessingJobSnapshot,
    pub preview: Option<ProcessingPreviewSnapshot>,
}

impl std::fmt::Debug for ProcessingPreviewSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessingPreviewSnapshot")
            .field("bytes", &self.markdown.len())
            .field("truncated", &self.truncated)
            .finish_non_exhaustive()
    }
}
impl std::fmt::Debug for ProcessingJobPreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProcessingJobPreview")
            .field("state", &self.job.state)
            .field("available", &self.preview.is_some())
            .finish_non_exhaustive()
    }
}
