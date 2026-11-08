use hypr_listener2_core::{BatchEvent, BatchParams, BatchProvider, BatchRuntime};
use hypr_loofah_core::{
    local_cli::{LocalCliArtifact, LocalMediaFuture, LocalMediaHost},
    settings::AppConfig,
};
use hypr_loofah_protocol::{ApplicationError, ErrorCode, LocalCliProgress, TranscriptWithData};
use owhisper_interface::batch_stream::BatchStreamEvent;
use std::{
    io::{Read, Seek, Write},
    path::PathBuf,
    sync::Arc,
};

#[derive(Default)]
pub struct LocalMediaEngine;
impl LocalMediaEngine {
    pub fn new() -> Self {
        Self
    }
}
impl LocalMediaHost for LocalMediaEngine {
    fn normalize(
        &self,
        source: LocalCliArtifact,
        filename: String,
        temporary: PathBuf,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalCliArtifact> {
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let (directory, path) = copy_input(source, &filename, temporary)?;
                report(&progress, "importing audio", 0);
                let output = directory.path().join("audio.mp3");
                hypr_fs_sync_core::audio::import_audio(
                    &path,
                    &directory.path().join("encode.tmp"),
                    &output,
                )
                .map_err(failure)?;
                let file = std::fs::File::open(output).map_err(failure)?;
                let metadata = hypr_fs_sync_core::media::metadata_from_reader(
                    "audio.mp3",
                    "audio/mpeg",
                    &file,
                )
                .map_err(failure)?;
                let mut file = file;
                file.rewind().map_err(failure)?;
                report(&progress, "imported audio", 100);
                Ok(LocalCliArtifact::new(
                    file,
                    metadata.size_bytes,
                    metadata.sha256,
                    None,
                    Box::new(directory),
                ))
            })
            .await
            .map_err(failure)?
        })
    }

    fn transcribe(
        &self,
        source: LocalCliArtifact,
        filename: String,
        session_id: String,
        audio: hypr_vault_read::SessionAudio,
        config: AppConfig,
        temporary: PathBuf,
        progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<TranscriptWithData> {
        Box::pin(async move {
            let model = model(&config)?;
            let parsed = model
                .parse::<hypr_transcribe_soniqo::SoniqoModel>()
                .map_err(failure)?;
            if !hypr_transcribe_soniqo::is_model_downloaded(parsed).map_err(failure)? {
                return Err(failure(format!(
                    "the {} model is not downloaded; open the desktop app once to download it",
                    parsed.display_name()
                )));
            }
            let languages = config
                .meeting_languages
                .unwrap_or_default()
                .iter()
                .map(|language| language.parse())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|cause| failure(format!("invalid meeting language: {cause}")))?;
            let (directory, path) =
                tokio::task::spawn_blocking(move || copy_input(source, &filename, temporary))
                    .await
                    .map_err(failure)??;
            let runtime = Arc::new(Runtime {
                directory,
                progress,
            });
            let output = hypr_listener2_core::run_batch(
                runtime.clone(),
                BatchParams {
                    audio,
                    session_id: session_id.clone(),
                    provider: BatchProvider::Soniqo,
                    file_path: path.to_string_lossy().into_owned(),
                    model: Some(model),
                    base_url: hypr_transcribe_soniqo::LOCAL_BASE_URL.into(),
                    api_key: String::new(),
                    languages,
                    keywords: Vec::new(),
                    num_speakers: None,
                    min_speakers: None,
                    max_speakers: None,
                },
            )
            .await
            .map_err(failure)?;
            let mut new_id = || uuid::Uuid::new_v4().to_string();
            hypr_listener2_core::transcript::transcript_from_batch_response(
                &output.response,
                hypr_listener2_core::transcript::BatchTranscriptMeta {
                    session_id: &session_id,
                    user_id: "00000000-0000-0000-0000-000000000000",
                    provider: "soniqo",
                    created_at: chrono::Utc::now()
                        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                    started_at_ms: chrono::Utc::now().timestamp_millis() as f64,
                    memo_md: String::new(),
                },
                &mut new_id,
            )
            .ok_or_else(|| failure("No speech was detected in the audio."))
        })
    }
}

fn model(config: &AppConfig) -> Result<String, ApplicationError> {
    let provider = config
        .current_stt_provider
        .as_deref()
        .filter(|value| !value.is_empty());
    let model = config
        .current_stt_model
        .as_deref()
        .filter(|value| !value.is_empty());
    let (Some(provider), Some(model)) = (provider, model) else {
        return Err(failure(
            "no speech-to-text model is configured; open the desktop app and set one up under Settings → Transcription",
        ));
    };
    if provider != "fmtr" {
        return Err(failure(format!(
            "speech-to-text provider '{provider}' is not supported by the CLI; open the desktop app to update Settings → Transcription"
        )));
    }
    model.parse::<hypr_transcribe_soniqo::SoniqoModel>().map_err(|_| failure(format!("speech-to-text model '{model}' is not supported by the CLI yet; open the desktop app to select a supported on-device Parakeet model under Settings → Transcription")))?;
    Ok(model.into())
}

fn copy_input(
    mut source: LocalCliArtifact,
    filename: &str,
    temporary: PathBuf,
) -> Result<(tempfile::TempDir, PathBuf), ApplicationError> {
    if std::path::Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        != Some(filename)
    {
        return Err(failure("Invalid input filename"));
    }
    for path in temporary.ancestors() {
        match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
                return Err(failure("Unsafe local media temporary directory"));
            }
            Ok(_) => {}
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => return Err(failure(cause)),
        }
    }
    std::fs::create_dir_all(&temporary).map_err(failure)?;
    let directory = tempfile::Builder::new()
        .prefix("local-media-")
        .tempdir_in(temporary)
        .map_err(failure)?;
    let path = directory.path().join(filename);
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(failure)?;
    let source_size = source.size_bytes;
    source.rewind().map_err(failure)?;
    std::io::copy(
        &mut (&mut source).take(source_size.saturating_add(1)),
        &mut output,
    )
    .map_err(failure)?;
    output.flush().map_err(failure)?;
    drop(output);
    let copied = hypr_fs_sync_core::media::metadata_from_reader(
        filename,
        "application/octet-stream",
        std::fs::File::open(&path).map_err(failure)?,
    )
    .map_err(failure)?;
    if copied.sha256 != source.sha256 || copied.size_bytes != source.size_bytes {
        return Err(failure("Recording input changed before processing"));
    }
    Ok((directory, path))
}

struct Runtime {
    directory: tempfile::TempDir,
    progress: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
}
impl BatchRuntime for Runtime {
    fn emit(&self, event: BatchEvent) {
        let _input_owner = &self.directory;
        match event {
            BatchEvent::BatchStarted { .. } => report(&self.progress, "transcribing audio", 0),
            BatchEvent::BatchResponseStreamed {
                event: BatchStreamEvent::Progress { percentage, .. },
                ..
            } => report(
                &self.progress,
                "transcribing audio",
                (percentage * 100.0) as usize,
            ),
            _ => {}
        }
    }
}
fn report(progress: &Arc<dyn Fn(LocalCliProgress) + Send + Sync>, phase: &str, processed: usize) {
    progress(LocalCliProgress {
        schema_version: "2".into(),
        phase: phase.into(),
        processed,
        total: Some(100),
        added: 0,
        updated: 0,
        deleted: 0,
        elapsed_ms: 0,
    });
}
fn failure(cause: impl std::fmt::Display) -> ApplicationError {
    ApplicationError {
        code: ErrorCode::Validation,
        message: cause.to_string(),
        request_id: None,
        retryable: false,
        current_revision: None,
        mutation_outcome: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config(provider: Option<&str>, model: Option<&str>) -> AppConfig {
        AppConfig {
            current_stt_provider: provider.map(str::to_owned),
            current_stt_model: model.map(str::to_owned),
            ..Default::default()
        }
    }
    #[test]
    fn vault_config_preserves_selected_meeting_languages() {
        let config: AppConfig = serde_json::from_value(serde_json::json!({
            "current_stt_provider": "fmtr", "current_stt_model": "soniqo-nemotron-3.5-streaming",
            "meeting_languages": ["en-GB", "nl-BE"],
        }))
        .unwrap();
        assert_eq!(
            config.meeting_languages,
            Some(vec!["en-GB".into(), "nl-BE".into()])
        );
        assert!(AppConfig::default().meeting_languages.is_none());
    }

    #[test]
    fn missing_provider_or_model_asks_for_desktop_setup() {
        for config in [
            config(None, None),
            config(Some("fmtr"), None),
            config(None, Some("soniqo-parakeet-batch")),
            config(Some(""), Some("")),
        ] {
            let error = model(&config).unwrap_err();
            assert_eq!(error.code, ErrorCode::Validation);
            assert!(
                error
                    .to_string()
                    .contains("no speech-to-text model is configured"),
                "unexpected message: {error}"
            );
        }
    }

    #[test]
    fn non_fmtr_provider_is_rejected_as_unsupported() {
        let error = model(&config(Some("deepgram"), Some("nova-3"))).unwrap_err();
        assert!(error.to_string().contains("provider 'deepgram'"));
        assert!(error.to_string().contains("not supported by the CLI"));
    }

    #[test]
    fn non_soniqo_models_are_rejected_as_unsupported() {
        for selected in [
            "am-parakeet-v3",
            "soniqo-qwen3-small",
            "soniqo-qwen3-large",
            "aufklarer/Qwen3-ASR-0.6B-MLX-4bit",
            "aufklarer/Qwen3-ASR-1.7B-MLX-8bit",
            "QuantizedSmallEn",
            "whisper-large-v3",
        ] {
            let error = model(&config(Some("fmtr"), Some(selected))).unwrap_err();
            assert!(error.to_string().contains(selected));
            assert!(error.to_string().contains("not supported by the CLI"));
        }
    }

    #[test]
    fn soniqo_models_resolve_to_their_configured_id() {
        let selected = model(&config(Some("fmtr"), Some("soniqo-nemotron-3.5-streaming"))).unwrap();
        assert_eq!(selected, "soniqo-nemotron-3.5-streaming");
        let selected = model(&config(Some("fmtr"), Some("soniqo-parakeet-batch"))).unwrap();
        assert_eq!(selected, "soniqo-parakeet-batch");

        // Streaming models pass through unchanged; the engine maps them to
        // their batch sibling, like the desktop passing conn.model along.
        let selected = model(&config(Some("fmtr"), Some("soniqo-parakeet-streaming"))).unwrap();
        assert_eq!(selected, "soniqo-parakeet-streaming");
    }
}
