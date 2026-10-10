use std::sync::{LazyLock, Mutex};

use hypr_transcribe_onnx::{
    BatchSession, LiveSession,
    models::{self, Model},
};

use super::*;

static MODEL_OPERATIONS: Mutex<()> = Mutex::new(());

static LIVE: LazyLock<Mutex<Option<[LiveSession; 2]>>> = LazyLock::new(Mutex::default);
static BATCH: LazyLock<Mutex<Option<BatchSession>>> = LazyLock::new(Mutex::default);

fn convert_error(error: impl std::fmt::Display) -> Error {
    Error::Bridge(error.to_string())
}

fn backend_model(model: SoniqoModel) -> Result<Model> {
    match model {
        SoniqoModel::ParakeetStreaming | SoniqoModel::OnnxParakeetStreaming => Ok(Model::Streaming),
        SoniqoModel::ParakeetBatch | SoniqoModel::OnnxParakeetBatch => Ok(Model::Batch),
        _ => Err(Error::UnsupportedModel(model.to_string())),
    }
}

pub(super) fn model_cache_dir(model: SoniqoModel) -> Result<PathBuf> {
    backend_model(model)?.cache_dir().map_err(convert_error)
}

pub(super) fn model_download_state(model: SoniqoModel) -> Result<ModelDownloadState> {
    let state = models::state(backend_model(model)?).map_err(convert_error)?;
    Ok(ModelDownloadState {
        status: state.status,
        current_file: state.current_file,
        progress_percent: state.progress_percent,
        local_path: state.local_path,
        error: state.error,
    })
}

pub(super) fn start_model_download(model: SoniqoModel) -> Result<()> {
    let _operation = MODEL_OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    models::start(backend_model(model)?).map_err(convert_error)
}

pub(super) fn delete_model(model: SoniqoModel) -> Result<()> {
    let _operation = MODEL_OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    reset_inner(model)?;
    let path = model_cache_dir(model)?;
    if path.exists() {
        std::fs::remove_dir_all(path).map_err(Error::Delete)?;
    }
    Ok(())
}

pub(super) fn reset_model(model: SoniqoModel) -> Result<()> {
    let _operation = MODEL_OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    reset_inner(model)
}

fn reset_inner(model: SoniqoModel) -> Result<()> {
    let model = backend_model(model)?;
    match model {
        Model::Streaming => {
            LIVE.lock().unwrap_or_else(|e| e.into_inner()).take();
        }
        Model::Diarizer => return Err(Error::UnsupportedModel(model.id().into())),
        Model::Batch => {
            BATCH.lock().unwrap_or_else(|e| e.into_inner()).take();
        }
    }
    models::reset(model);
    Ok(())
}

pub(super) fn transcribe_file(model: SoniqoModel, path: &Path, _: &str) -> Result<FileTranscript> {
    let _operation = MODEL_OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    if backend_model(model)? != Model::Batch {
        return Err(Error::UnsupportedModel(model.to_string()));
    }
    let mut batch = BATCH.lock().unwrap_or_else(|e| e.into_inner());
    if batch.is_none() {
        *batch = Some(BatchSession::load().map_err(convert_error)?);
    }
    let (duration, segments) = batch
        .as_mut()
        .unwrap()
        .transcribe_file(path)
        .map_err(convert_error)?;
    Ok(FileTranscript::from_chunks(
        segments
            .into_iter()
            .map(|segment| FileTranscriptChunk {
                text: segment.text,
                start_seconds: segment.start,
                duration_seconds: (segment.end - segment.start).max(0.0),
                speech_spans: vec![SpeechSpan {
                    start_seconds: segment.start,
                    end_seconds: segment.end,
                }],
            })
            .collect(),
        duration,
    ))
}

pub(super) fn live_start(model: SoniqoModel) -> Result<()> {
    let _operation = MODEL_OPERATIONS.lock().unwrap_or_else(|e| e.into_inner());
    if backend_model(model)? != Model::Streaming {
        return Err(Error::UnsupportedModel(model.to_string()));
    }
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    if live.is_some() {
        return Err(Error::Bridge(
            "A live speech session is already running".into(),
        ));
    }
    *live = Some(LiveSession::load_pair().map_err(convert_error)?);
    Ok(())
}

pub(super) fn live_append(source: TranscriptSource, samples: &[f32]) -> Result<Vec<LivePartial>> {
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let sessions = live
        .as_mut()
        .ok_or_else(|| Error::Bridge("No live speech session".into()))?;
    let partials = sessions[source.channel_index() as usize]
        .append(samples)
        .map_err(convert_error)?;
    Ok(partials
        .into_iter()
        .map(|partial| LivePartial {
            source: source.as_str().into(),
            text: partial.text,
            is_final: partial.is_final,
        })
        .collect())
}

pub(super) fn live_finalize(source: TranscriptSource) -> Result<Vec<LivePartial>> {
    let mut live = LIVE.lock().unwrap_or_else(|e| e.into_inner());
    let Some(sessions) = live.as_mut() else {
        return Ok(Vec::new());
    };
    let partials = sessions[source.channel_index() as usize]
        .finalize()
        .map_err(convert_error)?;
    Ok(partials
        .into_iter()
        .map(|partial| LivePartial {
            source: source.as_str().into(),
            text: partial.text,
            is_final: partial.is_final,
        })
        .collect())
}

pub(super) fn live_stop() -> Result<()> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).take();
    Ok(())
}
