use crate::{Error, Result, local_owner::LocalGateway, output};
use hypr_loofah_protocol::{
    ApplicationError, ErrorCode, LocalCliOperation, LocalCliProgress, LocalCliReply,
};
use std::{path::Path, sync::Arc};

pub async fn run(vault: &Path, id: &str, json: bool) -> Result<()> {
    let response = LocalGateway::new(vault.to_owned(), crate::vault::cache_for_vault(vault)?)
        .execute_with_upload(
            LocalCliOperation::Transcribe {
                session_id: id.into(),
            },
            None,
            progress(),
        )
        .await?;
    let LocalCliReply::Transcribed {
        session_id,
        transcript_id,
        word_count,
    } = response
        .response
        .result
        .map_err(|cause| media_error("transcribe audio", cause))?
    else {
        return Err(crate::profiles::unexpected());
    };
    output::track_write(&session_id)?;
    output::emit(&if json {
        output::json(
            "transcribe",
            &serde_json::json!({ "id": session_id, "transcript": { "id": transcript_id, "words": word_count } }),
            None,
        )?
    } else {
        transcript_id
    });
    Ok(())
}

pub(super) fn progress() -> Arc<dyn Fn(LocalCliProgress) + Send + Sync> {
    Arc::new(|event| {
        if event.total == Some(100) {
            eprintln!("{}... {}%", event.phase, event.processed);
        }
    })
}
pub(super) fn media_error(action: &'static str, cause: ApplicationError) -> Error {
    match cause.code {
        ErrorCode::NotFound => Error::NotFound(cause.message.trim_end_matches(" not found").into()),
        ErrorCode::Validation => Error::operation(action, cause.message),
        _ => Error::from(cause),
    }
}

#[cfg(test)]
mod compatibility {
    use super::*;
    use hypr_vault_write::SessionStore;
    use std::path::PathBuf;
    const ACTION: &str = "transcribe audio";
    const AUDIO_FILE_NAMES: [&str; 3] = ["audio.mp3", "audio.wav", "audio.ogg"];
    /// The flat `config.json` keys the CLI needs; deliberately not the settings
    /// plugin's full `AppConfig` (that crate is tauri-bound).
    #[derive(Debug, Default, serde::Deserialize)]
    struct VaultConfig {
        #[serde(default)]
        current_stt_provider: Option<String>,
        #[serde(default)]
        current_stt_model: Option<String>,
        #[serde(default)]
        meeting_languages: Vec<String>,
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
                    "speech-to-text model '{model}' is not supported by the CLI yet; open the desktop app to select a supported on-device Parakeet model under Settings → Transcription"
                ),
            ));
        }

        Ok(model.to_string())
    }

    pub(crate) fn find_session_audio(session_dir: &Path) -> Option<PathBuf> {
        AUDIO_FILE_NAMES
            .iter()
            .map(|name| session_dir.join(name))
            .find(|path| path.is_file())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn config(provider: Option<&str>, model: Option<&str>) -> VaultConfig {
            VaultConfig {
                current_stt_provider: provider.map(str::to_string),
                current_stt_model: model.map(str::to_string),
                ..Default::default()
            }
        }

        #[test]
        fn vault_config_preserves_selected_meeting_languages() {
            let config: VaultConfig = serde_json::from_value(serde_json::json!({
            "current_stt_provider": "fmtr", "current_stt_model": "soniqo-nemotron-3.5-streaming",
            "meeting_languages": ["en-GB", "nl-BE"],
        }))
        .unwrap();
            assert_eq!(config.meeting_languages, ["en-GB", "nl-BE"]);
            assert!(super::VaultConfig::default().meeting_languages.is_empty());
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
            let error =
                resolve_soniqo_model(&config(Some("deepgram"), Some("nova-3"))).unwrap_err();
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
                resolve_soniqo_model(&config(Some("fmtr"), Some("soniqo-nemotron-3.5-streaming")))
                    .unwrap();
            assert_eq!(model, "soniqo-nemotron-3.5-streaming");
            let model =
                resolve_soniqo_model(&config(Some("fmtr"), Some("soniqo-parakeet-batch"))).unwrap();
            assert_eq!(model, "soniqo-parakeet-batch");

            // Streaming models pass through unchanged; the engine maps them to
            // their batch sibling, like the desktop passing conn.model along.
            let model =
                resolve_soniqo_model(&config(Some("fmtr"), Some("soniqo-parakeet-streaming")))
                    .unwrap();
            assert_eq!(model, "soniqo-parakeet-streaming");
        }

        #[tokio::test]
        async fn historical_transcripts_keep_provenance_and_speakers_when_read_and_exported() {
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
            std::fs::write(
            session_dir.join("_meta.json"),
            r#"{"id":"s1","title":"Historical meeting","created_at":"2026-03-20T00:00:00Z","tags":[]}"#,
        ).unwrap();
            let exported = hypr_agent_access::get_meeting_export(dir.path(), "s1".into())
                .await
                .unwrap();
            let exported = serde_json::to_value(exported).unwrap();
            let exported_word = &exported["transcripts"][0]["words"][0];
            assert_eq!(exported_word["text"], "Historical words");
            assert_eq!(exported_word["speaker"], "Alice");
            assert_eq!(exported_word["metadata"]["model"], "am-parakeet-v3");
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
                let error =
                    resolve_soniqo_model(&read_vault_config(dir.path()).unwrap()).unwrap_err();
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
        async fn audio_resolution_uses_the_canonical_session_directory() {
            let dir = tempfile::tempdir().unwrap();
            let id = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
            let session_dir = dir
                .path()
                .join("sessions/6ba7b810-9dad-11d1-80b4-00c04fd430c8");
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

            let store = SessionStore::new(dir.path().to_path_buf());
            let resolved = dir.path().join(store.session_dir(id).await.unwrap());
            assert_eq!(
                find_session_audio(&resolved),
                Some(session_dir.join("audio.mp3"))
            );
        }
    }
}
