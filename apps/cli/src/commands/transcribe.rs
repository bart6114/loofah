use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{Error, Result, output};
use hypr_listener2_core::{
    BatchEvent, BatchParams, BatchProvider, BatchRuntime, run_batch, transcript,
};
use hypr_vault_write::SessionStore;
use owhisper_interface::batch_stream::BatchStreamEvent;

const ACTION: &str = "transcribe audio";

/// The desktop's fixed local user id (`DEFAULT_USER_ID` in `shared/utils.ts`);
/// the owner concept died with the workspaces removal, so every transcript
/// carries this id.
const DEFAULT_USER_ID: &str = "00000000-0000-0000-0000-000000000000";

/// Recording names the readers look for, in `fs-sync-core`'s `AUDIO_FORMATS`
/// order.
const AUDIO_FILE_NAMES: [&str; 3] = ["audio.mp3", "audio.wav", "audio.ogg"];

pub async fn run(vault: &Path, id: &str, json: bool) -> Result<()> {
    let store = SessionStore::new(vault.to_path_buf());
    let outcome = transcribe_session(vault, &store, id).await?;

    let rendered = if json {
        output::json(
            "transcribe",
            &serde_json::json!({
                "id": id,
                "transcript": {
                    "id": outcome.transcript_id,
                    "words": outcome.words,
                },
            }),
            None,
        )?
    } else {
        outcome.transcript_id.clone()
    };
    output::emit(&rendered);
    Ok(())
}

pub(crate) struct TranscribeOutcome {
    pub transcript_id: String,
    pub words: usize,
}

/// Transcribes the session's audio with the configured on-device model and
/// replaces the session's transcript set, exactly as the desktop's batch path
/// does. All preconditions (session, audio, provider config, model download)
/// fail cleanly before any engine work starts.
pub(crate) async fn transcribe_session(
    vault: &Path,
    store: &SessionStore,
    session_id: &str,
) -> Result<TranscribeOutcome> {
    let meta = store
        .read_meta(session_id)
        .await
        .map_err(|error| Error::operation(ACTION, error.to_string()))?;
    if meta.is_none() {
        return Err(Error::NotFound(format!("meeting '{session_id}'")));
    }

    // Resolve the session's physical directory once: the basename may be a
    // readable name rather than the id, so audio lookup
    // must go through the store's catalog, never `sessions/<id>` directly.
    let session_dir = vault.join(
        store
            .session_dir(session_id)
            .await
            .map_err(|error| Error::operation(ACTION, error.to_string()))?,
    );
    let audio_path = find_session_audio(&session_dir)
        .ok_or_else(|| Error::NotFound(format!("audio recording for meeting '{session_id}'")))?;

    let config = read_vault_config(vault)?;
    let model = resolve_soniqo_model(&config)?;
    ensure_soniqo_model_ready(&model)?;

    let params = BatchParams {
        session_id: session_id.to_string(),
        provider: BatchProvider::Soniqo,
        file_path: audio_path.to_string_lossy().into_owned(),
        model: Some(model),
        base_url: hypr_transcribe_soniqo::LOCAL_BASE_URL.to_string(),
        api_key: String::new(),
        languages: Vec::new(),
        keywords: Vec::new(),
        num_speakers: None,
        min_speakers: None,
        max_speakers: None,
    };

    let output = run_batch(Arc::new(CliBatchRuntime), params)
        .await
        .map_err(|error| Error::operation(ACTION, error.to_string()))?;

    let mut new_id = || uuid::Uuid::new_v4().to_string();
    let transcript = transcript::transcript_from_batch_response(
        &output.response,
        transcript::BatchTranscriptMeta {
            session_id,
            user_id: DEFAULT_USER_ID,
            provider: "soniqo",
            created_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            started_at_ms: chrono::Utc::now().timestamp_millis() as f64,
            // The desktop snapshots the note into the transcript as a
            // prosemirror-JSON string; the CLI has no markdown-to-prosemirror
            // converter, so it leaves the snapshot empty even when a note exists.
            memo_md: String::new(),
        },
        &mut new_id,
    )
    .ok_or_else(|| Error::operation(ACTION, "No speech was detected in the audio."))?;

    let outcome = TranscribeOutcome {
        transcript_id: transcript.id.clone(),
        words: transcript.words.len(),
    };

    store
        .replace_session_transcripts(session_id, transcript)
        .await
        .map_err(|error| Error::operation(ACTION, error.to_string()))?;

    Ok(outcome)
}

/// The flat `config.json` keys the CLI needs; deliberately not the settings
/// plugin's full `AppConfig` (that crate is tauri-bound).
#[derive(Debug, Default, serde::Deserialize)]
struct VaultConfig {
    #[serde(default)]
    current_stt_provider: Option<String>,
    #[serde(default)]
    current_stt_model: Option<String>,
}

fn read_vault_config(vault: &Path) -> Result<VaultConfig> {
    let path = vault.join("config.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(VaultConfig::default());
        }
        Err(error) => {
            return Err(Error::operation(
                ACTION,
                format!("failed to read {}: {error}", path.display()),
            ));
        }
    };

    serde_json::from_str(&raw).map_err(|error| {
        Error::operation(
            ACTION,
            format!("failed to parse {}: {error}", path.display()),
        )
    })
}

/// Mirror of the desktop's `getBatchProvider` (useRunBatch.ts): provider must
/// be "fmtr", and a supported Soniqo model routes to the in-process CoreML engine.
fn resolve_soniqo_model(config: &VaultConfig) -> Result<String> {
    let provider = config
        .current_stt_provider
        .as_deref()
        .filter(|provider| !provider.is_empty());
    let model = config
        .current_stt_model
        .as_deref()
        .filter(|model| !model.is_empty());

    let (Some(provider), Some(model)) = (provider, model) else {
        return Err(Error::operation(
            ACTION,
            "no speech-to-text model is configured; open the desktop app and set one up under Settings → Transcription",
        ));
    };

    if provider != "fmtr" {
        return Err(Error::operation(
            ACTION,
            format!(
                "speech-to-text provider '{provider}' is not supported by the CLI; open the desktop app to update Settings → Transcription"
            ),
        ));
    }

    if model
        .parse::<hypr_transcribe_soniqo::SoniqoModel>()
        .is_err()
    {
        return Err(Error::operation(
            ACTION,
            format!(
                "speech-to-text model '{model}' is not supported by the CLI yet; open the desktop app to select a supported Soniqo model under Settings → Transcription"
            ),
        ));
    }

    Ok(model.to_string())
}

/// Fails before any engine work when the platform can't run the model or the
/// model isn't in the Soniqo cache. The CLI never downloads models — that
/// stays a desktop-app concern.
fn ensure_soniqo_model_ready(model: &str) -> Result<()> {
    let parsed: hypr_transcribe_soniqo::SoniqoModel =
        model
            .parse()
            .map_err(|error: hypr_transcribe_soniqo::Error| {
                Error::operation(ACTION, error.to_string())
            })?;
    // Streaming models transcribe files with their batch sibling (the same
    // mapping `run_soniqo_batch` applies), so check that model's cache.
    let batch_model = parsed.batch_model();

    let downloaded = hypr_transcribe_soniqo::is_model_downloaded(batch_model)
        .map_err(|error| Error::operation(ACTION, error.to_string()))?;
    if !downloaded {
        return Err(Error::operation(
            ACTION,
            format!(
                "the {} model is not downloaded; open the desktop app once to download it",
                batch_model.display_name()
            ),
        ));
    }

    Ok(())
}

pub(crate) fn find_session_audio(session_dir: &Path) -> Option<PathBuf> {
    AUDIO_FILE_NAMES
        .iter()
        .map(|name| session_dir.join(name))
        .find(|path| path.is_file())
}

/// Headless stand-in for the desktop's Tauri event forwarding: progress goes
/// to stderr (stdout stays reserved for the command's result, `--json` or
/// not); the final response is consumed from `run_batch`'s return value.
struct CliBatchRuntime;

impl BatchRuntime for CliBatchRuntime {
    fn emit(&self, event: BatchEvent) {
        match event {
            BatchEvent::BatchStarted { .. } => eprintln!("transcribing audio..."),
            BatchEvent::BatchResponseStreamed {
                event: BatchStreamEvent::Progress { percentage, .. },
                ..
            } => eprintln!("transcribing audio... {:.0}%", percentage * 100.0),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(provider: Option<&str>, model: Option<&str>) -> VaultConfig {
        VaultConfig {
            current_stt_provider: provider.map(str::to_string),
            current_stt_model: model.map(str::to_string),
        }
    }

    #[test]
    fn missing_provider_or_model_asks_for_desktop_setup() {
        for config in [
            config(None, None),
            config(Some("fmtr"), None),
            config(None, Some("soniqo-parakeet-batch")),
            config(Some(""), Some("")),
        ] {
            let error = resolve_soniqo_model(&config).unwrap_err();
            assert_eq!(error.code(), "operation_failed");
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
        let error = resolve_soniqo_model(&config(Some("deepgram"), Some("nova-3"))).unwrap_err();
        assert!(error.to_string().contains("provider 'deepgram'"));
        assert!(error.to_string().contains("not supported by the CLI"));
    }

    #[test]
    fn non_soniqo_models_are_rejected_as_unsupported() {
        for model in [
            "am-parakeet-v3",
            "soniqo-qwen3-small",
            "soniqo-qwen3-large",
            "aufklarer/Qwen3-ASR-0.6B-MLX-4bit",
            "aufklarer/Qwen3-ASR-1.7B-MLX-8bit",
            "QuantizedSmallEn",
            "whisper-large-v3",
        ] {
            let error = resolve_soniqo_model(&config(Some("fmtr"), Some(model))).unwrap_err();
            assert!(error.to_string().contains(model));
            assert!(error.to_string().contains("not supported by the CLI"));
        }
    }

    #[test]
    fn soniqo_models_resolve_to_their_configured_id() {
        let model =
            resolve_soniqo_model(&config(Some("fmtr"), Some("soniqo-parakeet-batch"))).unwrap();
        assert_eq!(model, "soniqo-parakeet-batch");

        // Streaming models pass through unchanged; the engine maps them to
        // their batch sibling, like the desktop passing conn.model along.
        let model =
            resolve_soniqo_model(&config(Some("fmtr"), Some("soniqo-parakeet-streaming"))).unwrap();
        assert_eq!(model, "soniqo-parakeet-streaming");
    }

    #[test]
    fn historical_transcripts_keep_provenance_and_speakers_when_read_and_exported() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("sessions/s1");
        std::fs::create_dir_all(&session_dir).unwrap();
        let path = session_dir.join("transcript.json");
        let original = serde_json::json!({"transcripts": [{
            "id": "t1", "session_id": "s1", "words": [{
                "text": "Historical words", "start_ms": 0.0, "end_ms": 1000.0, "channel": 0.0,
                "speaker": "Alice", "metadata": {"model": "am-parakeet-v3", "arch": "ArgmaxSDK"}
            }]
        }]})
        .to_string();
        std::fs::write(&path, &original).unwrap();
        let file = hypr_vault_read::transcript::read_transcript_json(dir.path(), "s1").unwrap();
        let word = &file.transcripts[0].words[0];
        assert_eq!(word.metadata.as_ref().unwrap()["model"], "am-parakeet-v3");
        let export = dir.path().join("export.vtt");
        hypr_listener2_core::export_words_to_vtt_file(
            vec![hypr_listener2_core::VttWord {
                text: word.text.clone(),
                start_ms: word.start_ms as u64,
                end_ms: word.end_ms as u64,
                speaker: word.speaker.clone(),
            }],
            &export,
        )
        .unwrap();
        let content = std::fs::read_to_string(export).unwrap();
        assert!(content.contains("Historical words"));
        assert!(content.contains("Alice"));
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
    }

    #[test]
    fn retired_vault_selection_is_read_only_and_actionable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        for (provider, model) in [
            ("am", "am-parakeet-v3"),
            ("fmtr", "soniqo-qwen3-small"),
            ("fmtr", "aufklarer/Qwen3-ASR-1.7B-MLX-8bit"),
        ] {
            let original = serde_json::json!({"current_stt_provider": provider, "current_stt_model": model, "future": 42}).to_string();
            std::fs::write(&path, &original).unwrap();
            let error = resolve_soniqo_model(&read_vault_config(dir.path()).unwrap()).unwrap_err();
            assert!(error.to_string().contains("open the desktop app"));
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        }
    }

    #[test]
    fn vault_config_reads_the_flat_keys_and_defaults_when_absent() {
        let dir = tempfile::tempdir().unwrap();

        // Missing file is "not configured", not an error.
        let config = read_vault_config(dir.path()).unwrap();
        assert_eq!(config.current_stt_provider, None);
        assert_eq!(config.current_stt_model, None);

        std::fs::write(
            dir.path().join("config.json"),
            serde_json::json!({
                "current_stt_provider": "fmtr",
                "current_stt_model": "soniqo-parakeet-batch",
                "audio_retention": "none",
                "spoken_languages": ["en"],
                "unrelated_key": { "nested": true },
            })
            .to_string(),
        )
        .unwrap();
        let config = read_vault_config(dir.path()).unwrap();
        assert_eq!(config.current_stt_provider.as_deref(), Some("fmtr"));
        assert_eq!(
            config.current_stt_model.as_deref(),
            Some("soniqo-parakeet-batch")
        );

        std::fs::write(dir.path().join("config.json"), "{ not json").unwrap();
        let error = read_vault_config(dir.path()).unwrap_err();
        assert_eq!(error.code(), "operation_failed");
    }

    #[test]
    fn find_session_audio_prefers_the_reader_order() {
        let dir = tempfile::tempdir().unwrap();
        let session_dir = dir.path().join("sessions").join("s1");
        std::fs::create_dir_all(&session_dir).unwrap();

        assert_eq!(find_session_audio(&session_dir), None);

        std::fs::write(session_dir.join("audio.wav"), b"wav").unwrap();
        assert_eq!(
            find_session_audio(&session_dir),
            Some(session_dir.join("audio.wav"))
        );

        std::fs::write(session_dir.join("audio.mp3"), b"mp3").unwrap();
        assert_eq!(
            find_session_audio(&session_dir),
            Some(session_dir.join("audio.mp3"))
        );
    }

    #[tokio::test]
    async fn audio_resolution_follows_a_readable_session_directory() {
        let dir = tempfile::tempdir().unwrap();
        let id = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
        let session_dir = dir.path().join("sessions/2026-03-20 — Planning — 6ba7b8");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("_meta.json"),
            serde_json::json!({
                "id": id,
                "title": "Planning",
                "started_at": null,
                "ended_at": null,
                "created_at": "2026-03-20T10:00:00.000Z",
                "tags": [],
            })
            .to_string(),
        )
        .unwrap();
        std::fs::write(session_dir.join("audio.mp3"), b"mp3").unwrap();

        // The exact resolution import/transcribe perform: the store maps the
        // id to the readable directory, and audio lookup runs inside it.
        let store = SessionStore::new(dir.path().to_path_buf());
        let resolved = dir.path().join(store.session_dir(id).await.unwrap());
        assert_eq!(
            find_session_audio(&resolved),
            Some(session_dir.join("audio.mp3"))
        );
    }
}
