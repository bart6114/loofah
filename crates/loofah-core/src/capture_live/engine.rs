use std::sync::Arc;

use hypr_loofah_protocol::{
    CaptureFormat, CaptureLiveCaption, CaptureLiveFailure, CaptureLiveRequest, TranscriptionFailure,
};
use tokio::sync::{mpsc, watch};

use crate::transcription_jobs::TranscriptionFuture;

#[derive(Clone)]
pub struct CaptureLiveWork {
    pub principal: String,
    pub session_id: String,
    pub capture_id: String,
    pub generation_id: String,
}

pub trait CaptureLiveAuthorizer: Send + Sync + 'static {
    fn observe<'a>(
        &'a self,
        work: &'a CaptureLiveWork,
    ) -> TranscriptionFuture<
        'a,
        Result<crate::transcription_jobs::TranscriptionAccess, TranscriptionFailure>,
    >;

    fn authorize<'a>(
        &'a self,
        work: &'a CaptureLiveWork,
    ) -> TranscriptionFuture<'a, Result<Box<dyn Send>, TranscriptionFailure>>;
}

pub struct CaptureLiveEngineSpec {
    pub generation_id: String,
    pub format: CaptureFormat,
    pub request: CaptureLiveRequest,
}

pub trait CaptureLiveEngine: Send + Sync + 'static {
    /// The returned session owns native admission. Source ownership must last
    /// until actual native completion even if this future or its caller drops.
    fn start(
        &self,
        spec: CaptureLiveEngineSpec,
        source_owner: Box<dyn Send>,
    ) -> TranscriptionFuture<'_, Result<CaptureLiveSession, CaptureLiveFailure>>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureLiveIngress {
    Accepted,
    Duplicate,
    Busy,
}

pub trait CaptureLiveControl: Send + Sync + 'static {
    fn try_accept(
        &self,
        first_frame: u64,
        pcm: &Arc<[f32]>,
    ) -> Result<CaptureLiveIngress, CaptureLiveFailure>;
    fn finish(&self, final_frame: u64) -> Result<(), CaptureLiveFailure>;
    fn cancel(&self);
}

#[derive(Debug, Clone)]
pub struct CaptureLiveEngineReceipt {
    pub generation_id: String,
    pub admitted_end_frame: u64,
    pub input_consumed_end_frame: u64,
    pub completed_through_frame: Option<u64>,
    pub caption_count: u64,
    pub failure: Option<CaptureLiveFailure>,
    pub native_joined: bool,
    pub owners_released: bool,
}

pub struct CaptureLiveSession {
    pub control: Arc<dyn CaptureLiveControl>,
    pub captions: mpsc::Receiver<CaptureLiveCaption>,
    /// Closure or Pending never proves native, source or mapper completion.
    pub completion: watch::Receiver<Option<Arc<CaptureLiveEngineReceipt>>>,
}
