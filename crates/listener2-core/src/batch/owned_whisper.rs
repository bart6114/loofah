use std::sync::Arc;

use hypr_transcribe_whisper_local::resources::{
    BatchSnapshot, TrustedBatchSource, WhisperResources,
};

use super::{BatchParams, BatchRunMode, BatchRunOutput, audio, build_listen_params, diarize};
use crate::{BatchEvent, BatchRuntime};

#[cfg(test)]
mod tests;

/// Runs the same preparation, channel semantics and diarization as the UI batch
/// kernel, with a native owner instead of an HTTP server that another caller can replace.
pub async fn run_owned_whisper_batch(
    runtime: Arc<dyn BatchRuntime>,
    params: BatchParams,
    resources: Arc<WhisperResources>,
) -> crate::Result<BatchRunOutput> {
    let admission = resources.try_batch().map_err(failure)?;
    let prepared = audio::PreparedAudio::prepare(
        params.file_path.clone(),
        params.audio,
        Some(runtime.clone()),
    )
    .await?;
    let runtime: Arc<dyn BatchRuntime> = Arc::new(audio::PreparedRuntime {
        inner: runtime,
        prepared: prepared.clone(),
    });
    let listen = build_listen_params(
        &params,
        prepared.pcm.channels as u8,
        prepared.pcm.sample_rate,
    );
    let source =
        TrustedBatchSource::new(prepared.path.clone(), (prepared.clone(), runtime.clone()))
            .map_err(failure)?;
    let mut work = resources
        .start_batch(admission, source, listen)
        .map_err(failure)?;
    let diarization =
        diarize::SharedDiarization::for_file(Arc::new(diarize::SoniqoDiarizer), prepared.clone());
    let mut completion = work.observer();
    let mut events_open = true;
    let result = loop {
        if runtime.is_cancelled() {
            work.cancel();
        }
        if let BatchSnapshot::Finished(result) = completion.snapshot() {
            break result;
        }
        tokio::select! {
            event = work.events.recv(), if events_open => {
                match event {
                    Some(event @ (owhisper_interface::batch_sse::BatchSseMessage::Progress { .. } | owhisper_interface::batch_sse::BatchSseMessage::Segment { .. })) => {
                        runtime.emit(BatchEvent::BatchResponseStreamed { session_id: params.session_id.clone(), event: event.into() });
                    }
                    Some(_) => {},
                    None => events_open = false,
                }
            }
            next = completion.changed() => {
                if next.is_none() { return Err(failure("Native completion observer closed")); }
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
        }
    };
    // Diarization retains the same source and may still be running after ASR fails.
    let segments = diarization.segments().await?;
    let mut response = result.as_ref().clone().map_err(failure)?;
    if runtime.is_cancelled() {
        return Err(failure("Transcription interrupted"));
    }
    diarize::stamp_batch_response(&mut response, &segments);
    prepared.stamp_response(&mut response);
    Ok(BatchRunOutput {
        session_id: params.session_id,
        mode: BatchRunMode::Direct,
        response,
    })
}

fn failure(_: impl std::fmt::Display) -> crate::Error {
    crate::BatchFailure::DirectRequestFailed {
        provider: "whispercpp".into(),
        message: "On-device transcription failed".into(),
    }
    .into()
}
