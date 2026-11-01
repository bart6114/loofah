use crate::{InferenceProgress, batch, batch_stream, common_derives, stream};

pub const EVENT_NAME: &str = "batch";
pub const KEEP_ALIVE_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10);
pub const IDLE_TIMEOUT: std::time::Duration = KEEP_ALIVE_INTERVAL.saturating_mul(3);

common_derives! {
    #[serde(tag = "type", rename_all = "snake_case")]
    pub enum BatchSseMessage {
        Progress { progress: InferenceProgress },
        Segment { response: stream::StreamResponse },
        Result { response: batch::Response },
        Error { error: String, detail: String },
    }
}

impl From<BatchSseMessage> for batch_stream::BatchStreamEvent {
    fn from(value: BatchSseMessage) -> Self {
        match value {
            BatchSseMessage::Progress { progress } => batch_stream::BatchStreamEvent::Progress {
                percentage: progress.percentage,
                partial_text: progress.partial_text,
            },
            BatchSseMessage::Segment { response } => batch_stream::BatchStreamEvent::Segment {
                percentage: 0.0,
                response,
            },
            BatchSseMessage::Result { response } => {
                batch_stream::BatchStreamEvent::Result { response }
            }
            BatchSseMessage::Error { error, detail } => batch_stream::BatchStreamEvent::Error {
                error_code: None,
                error_message: detail,
                provider: error,
            },
        }
    }
}
