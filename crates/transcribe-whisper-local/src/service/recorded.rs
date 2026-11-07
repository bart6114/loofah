use std::{
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

use owhisper_interface::Word2;
use owhisper_interface::{ListenParams, batch, batch_sse::BatchSseMessage};

/// Runs on the caller's blocking thread with trusted host paths. Keep model, audio and admission
/// owners alive until this returns; cancellation requests do not mean native work has stopped.
/// Load the model with `LoadedWhisper::builder().use_gpu(false)` for explicit CPU execution.
/// Progress and segment events are synchronous; the terminal response is returned separately.
/// An emitter error requests cancellation. A backpressured emitter must also observe cancellation
/// and receiver closure so a disconnected consumer cannot indefinitely retain the blocking owner.
pub fn transcribe_batch_file(
    loaded_model: &hypr_whisper_local::LoadedWhisper,
    model_path: &Path,
    audio_path: &Path,
    params: &ListenParams,
    cancelled: Arc<AtomicBool>,
    emit: impl FnMut(BatchSseMessage) -> Result<(), crate::Error>,
) -> Result<batch::Response, crate::Error> {
    super::batch::transcribe_source(
        audio_path,
        params,
        loaded_model,
        model_path,
        cancelled,
        emit,
    )
}

pub fn process_recorded(
    model_path: impl AsRef<std::path::Path>,
    audio_path: impl AsRef<std::path::Path>,
) -> Result<Vec<Word2>, crate::Error> {
    let model = super::load_model(model_path.as_ref())?;
    super::batch::transcribe_recorded_file(&model, model_path.as_ref(), audio_path.as_ref())
}

#[cfg(test)]
mod tests;
