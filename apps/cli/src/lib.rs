#![forbid(unsafe_code)]

mod cli;
mod commands;
mod error;
mod mcp;
mod output;
mod vault;

pub use cli::Args;
pub use error::{Error, Result};
pub use output::JSON_SCHEMA_VERSION;

pub async fn run(args: Args) -> Result<u8> {
    if let cli::Command::Sync(sync) = &args.command {
        return commands::sync::run(&args, sync).await;
    }
    if matches!(&args.command, cli::Command::Doctor) {
        let ready = commands::doctor::run(&args, args.json)?;
        return Ok(if ready { 0 } else { 1 });
    }

    let vault = vault::open(&args)?;

    let mutates = matches!(
        &args.command,
        cli::Command::Import { .. }
            | cli::Command::Transcribe { .. }
            | cli::Command::Sessions {
                command: cli::MeetingCommand::New { .. }
                    | cli::MeetingCommand::Tag { .. }
                    | cli::MeetingCommand::Attach { .. }
            }
    ) || matches!(&args.command, cli::Command::Sessions { command: cli::MeetingCommand::Note { set, append, .. } } if set.is_some() || append.is_some());
    match args.command {
        cli::Command::Sync(_) => unreachable!("sync is handled before ordinary vault access"),
        cli::Command::Doctor => unreachable!("doctor returns before opening the vault"),
        cli::Command::Sessions { command } => {
            commands::meetings::run(&vault, command, args.json).await?
        }
        cli::Command::Import {
            file,
            title,
            into,
            transcribe,
            created_at,
            started_at,
            ended_at,
            author,
            skill,
        } => {
            let timestamps = commands::NewSessionOptions {
                created_at,
                started_at,
                ended_at,
                tags: Vec::new(),
                author,
                skill,
            };
            let result =
                commands::import::run(&vault, file, title, into, transcribe, timestamps, args.json)
                    .await;
            commands::sync::notify_committed(&vault).await;
            return result;
        }
        cli::Command::Transcribe { id } => {
            commands::transcribe::run(&vault, &id, args.json).await?
        }
        cli::Command::Mcp => mcp::serve(vault.clone()).await?,
        cli::Command::Tags { command } => commands::tags::run(&vault, command, args.json).await?,
    }

    if mutates {
        commands::sync::notify_committed(&vault).await;
    }
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn doctor_returns_nonzero_status_when_vault_is_not_ready() {
        let dir = tempfile::tempdir().unwrap();
        let status = run(Args {
            base: None,
            vault_path: Some(dir.path().join("missing-vault")),
            json: true,
            command: cli::Command::Doctor,
        })
        .await
        .unwrap();

        assert_eq!(status, 1);
    }

    fn write_session(vault: &std::path::Path, id: &str, note: Option<&str>) {
        let session_dir = vault.join("sessions").join(id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("_meta.json"),
            serde_json::json!({
                "id": id,
                "title": "Planning",
                "started_at": null,
                "ended_at": null,
                "created_at": "2026-07-13T00:00:00Z",
                "tags": [],
            })
            .to_string(),
        )
        .unwrap();
        if let Some(note) = note {
            std::fs::write(session_dir.join("notes.md"), note).unwrap();
        }
    }

    fn write_session_with_tags(vault: &std::path::Path, id: &str, tags: &[&str]) {
        let session_dir = vault.join("sessions").join(id);
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("_meta.json"),
            serde_json::json!({
                "id": id,
                "title": "Planning",
                "started_at": null,
                "ended_at": null,
                "created_at": "2026-07-13T00:00:00Z",
                "tags": tags,
            })
            .to_string(),
        )
        .unwrap();
    }

    fn read_meta_tags(vault: &std::path::Path, id: &str) -> Vec<String> {
        let meta: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(vault.join("sessions").join(id).join("_meta.json")).unwrap(),
        )
        .unwrap();
        meta["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tag| tag.as_str().unwrap().to_string())
            .collect()
    }

    fn read_registry_ids(vault: &std::path::Path) -> Vec<String> {
        let raw = match std::fs::read_to_string(vault.join("tags.json")) {
            Ok(raw) => raw,
            Err(_) => return Vec::new(),
        };
        let file: serde_json::Value = serde_json::from_str(&raw).unwrap();
        file["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tag| tag["id"].as_str().unwrap().to_string())
            .collect()
    }

    fn meetings_args(vault: &std::path::Path, command: cli::MeetingCommand) -> Args {
        Args {
            base: None,
            vault_path: Some(vault.to_path_buf()),
            json: false,
            command: cli::Command::Sessions { command },
        }
    }

    #[tokio::test]
    async fn new_command_creates_meta_and_note_readable_via_the_read_path() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let body_path = dir.path().join("body.md");
        std::fs::write(&body_path, "Decide the launch date.\n").unwrap();

        run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: true,
            command: cli::Command::Sessions {
                command: cli::MeetingCommand::New {
                    title: "Kickoff".to_string(),
                    note: Some(body_path),
                    created_at: None,
                    started_at: None,
                    ended_at: None,
                    tag: Vec::new(),
                    author: None,
                    skill: None,
                },
            },
        })
        .await
        .unwrap();

        // The directory is named exactly for its persistent session ID.
        let sessions = std::fs::read_dir(vault.join("sessions"))
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(sessions.len(), 1);
        let meta: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(sessions[0].path().join("_meta.json")).unwrap(),
        )
        .unwrap();
        let id = meta["id"].as_str().unwrap().to_string();
        assert_eq!(sessions[0].file_name().to_str(), Some(id.as_str()));
        // Desktop id format: lowercase hyphenated UUID (crypto.randomUUID()).
        assert_eq!(id.len(), 36);
        assert_eq!(id.matches('-').count(), 4);
        assert_eq!(id, id.to_lowercase());

        assert_eq!(meta["title"], "Kickoff");
        assert_eq!(meta["tags"], serde_json::json!([]));
        // No --author means the vault owner wrote it: the key must be absent, not null.
        assert!(meta.get("author").is_none());
        // Desktop timestamp format: RFC3339 UTC with millisecond precision and Z.
        let created_at = meta["created_at"].as_str().unwrap();
        assert!(
            created_at.ends_with('Z') && created_at.len() == 24,
            "unexpected created_at format: {created_at}"
        );
        assert_eq!(
            std::fs::read_to_string(sessions[0].path().join("notes.md")).unwrap(),
            "Decide the launch date.\n"
        );

        // The existing read path sees the new session.
        run(Args {
            base: None,
            vault_path: Some(vault),
            json: true,
            command: cli::Command::Sessions {
                command: cli::MeetingCommand::Note {
                    id,
                    kind: cli::DocumentKind::Note,
                    set: None,
                    append: None,
                },
            },
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn new_command_records_the_author_and_skill_in_meta() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();

        run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: true,
            command: cli::Command::Sessions {
                command: cli::MeetingCommand::New {
                    title: "Agent note".to_string(),
                    note: None,
                    created_at: None,
                    started_at: None,
                    ended_at: None,
                    tag: Vec::new(),
                    author: Some("claude-code".to_string()),
                    skill: Some("meeting-summarizer".to_string()),
                },
            },
        })
        .await
        .unwrap();

        let sessions = std::fs::read_dir(vault.join("sessions"))
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(sessions.len(), 1);
        let meta: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(sessions[0].path().join("_meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta["author"], "claude-code");
        assert_eq!(meta["skill"], "meeting-summarizer");
    }

    #[tokio::test]
    async fn new_command_creates_nothing_when_the_note_source_is_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();

        let error = run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: false,
            command: cli::Command::Sessions {
                command: cli::MeetingCommand::New {
                    title: "Kickoff".to_string(),
                    note: Some(dir.path().join("missing.md")),
                    created_at: None,
                    started_at: None,
                    ended_at: None,
                    tag: Vec::new(),
                    author: None,
                    skill: None,
                },
            },
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "operation_failed");
        // The body is read before the vault is touched, so a bad --note path
        // must not leave a session behind.
        assert!(!vault.join("sessions").exists());
    }

    fn write_test_wav(path: &std::path::Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).unwrap();
        // Half a second of a quiet tone; enough signal for the MP3 encoder
        // to produce real frames.
        for i in 0..8_000u32 {
            let sample = (f32::sin(i as f32 * 0.05) * 3000.0) as i16;
            writer.write_sample(sample).unwrap();
        }
        writer.finalize().unwrap();
    }

    #[tokio::test]
    async fn import_command_normalizes_audio_into_a_new_meeting() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let audio_path = dir.path().join("standup recording.wav");
        write_test_wav(&audio_path);

        run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: true,
            command: cli::Command::Import {
                file: audio_path,
                title: None,
                into: None,
                transcribe: false,
                created_at: None,
                started_at: None,
                ended_at: None,
                author: None,
                skill: None,
            },
        })
        .await
        .unwrap();

        // The directory is named exactly for its persistent session ID.
        let sessions = std::fs::read_dir(vault.join("sessions"))
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(sessions.len(), 1);
        let meta: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(sessions[0].path().join("_meta.json")).unwrap(),
        )
        .unwrap();
        let id = meta["id"].as_str().unwrap();
        assert_eq!(id.len(), 36);
        // The title defaults to the audio file's stem.
        assert_eq!(meta["title"], "standup recording");
        let created_at = meta["created_at"].as_str().unwrap();
        assert!(
            created_at.ends_with('Z') && created_at.len() == 24,
            "unexpected created_at format: {created_at}"
        );

        // The audio landed as the vault's normalized MP3, with no temp file left.
        let audio = sessions[0].path().join("audio.mp3");
        assert!(std::fs::metadata(&audio).unwrap().len() > 0);
        assert!(!sessions[0].path().join("audio.mp3.tmp").exists());
    }

    #[tokio::test]
    async fn import_into_normalizes_audio_into_the_existing_meeting() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session(&vault, "meeting-1", Some("Agenda: launch date."));
        let audio_path = dir.path().join("standup.wav");
        write_test_wav(&audio_path);

        run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: true,
            command: cli::Command::Import {
                file: audio_path,
                title: None,
                into: Some("meeting-1".to_string()),
                transcribe: false,
                created_at: None,
                started_at: None,
                ended_at: None,
                author: None,
                skill: None,
            },
        })
        .await
        .unwrap();

        // No second session appeared; the audio landed in the target session
        // and the meeting's own metadata and note stayed untouched.
        let sessions = std::fs::read_dir(vault.join("sessions"))
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(sessions.len(), 1);
        let session_dir = vault.join("sessions/meeting-1");
        assert!(
            std::fs::metadata(session_dir.join("audio.mp3"))
                .unwrap()
                .len()
                > 0
        );
        assert!(!session_dir.join("audio.mp3.tmp").exists());
        let meta: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(session_dir.join("_meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta["title"], "Planning");
        assert_eq!(
            std::fs::read_to_string(session_dir.join("notes.md")).unwrap(),
            "Agenda: launch date."
        );
    }

    #[tokio::test]
    async fn import_into_fails_cleanly_when_the_meeting_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join("sessions")).unwrap();
        let audio_path = dir.path().join("standup.wav");
        write_test_wav(&audio_path);

        let error = run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: false,
            command: cli::Command::Import {
                file: audio_path,
                title: None,
                into: Some("missing".to_string()),
                transcribe: false,
                created_at: None,
                started_at: None,
                ended_at: None,
                author: None,
                skill: None,
            },
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "not_found");
        assert_eq!(error.exit_code(), 2);
        assert!(!vault.join("sessions/missing").exists());
    }

    #[tokio::test]
    async fn import_into_refuses_to_replace_an_existing_recording() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session(&vault, "meeting-1", None);
        std::fs::write(vault.join("sessions/meeting-1/audio.mp3"), b"mp3").unwrap();
        let audio_path = dir.path().join("standup.wav");
        write_test_wav(&audio_path);

        let error = run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: false,
            command: cli::Command::Import {
                file: audio_path,
                title: None,
                into: Some("meeting-1".to_string()),
                transcribe: false,
                created_at: None,
                started_at: None,
                ended_at: None,
                author: None,
                skill: None,
            },
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "operation_failed");
        assert!(error.to_string().contains("already has a recording"));
        // The stub recording survives untouched.
        assert_eq!(
            std::fs::read(vault.join("sessions/meeting-1/audio.mp3")).unwrap(),
            b"mp3"
        );
    }

    #[tokio::test]
    async fn import_rejects_unsupported_formats_without_creating_anything() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        let text_path = dir.path().join("notes.txt");
        std::fs::write(&text_path, "not audio").unwrap();

        let error = run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: false,
            command: cli::Command::Import {
                file: text_path,
                title: None,
                into: None,
                transcribe: false,
                created_at: None,
                started_at: None,
                ended_at: None,
                author: None,
                skill: None,
            },
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "operation_failed");
        assert!(error.to_string().contains("unsupported audio format"));
        assert!(!vault.join("sessions").exists());
    }

    #[tokio::test]
    async fn import_fails_cleanly_when_the_audio_file_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();

        let error = run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: false,
            command: cli::Command::Import {
                file: dir.path().join("missing.wav"),
                title: None,
                into: None,
                transcribe: false,
                created_at: None,
                started_at: None,
                ended_at: None,
                author: None,
                skill: None,
            },
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "not_found");
        assert_eq!(error.exit_code(), 2);
        assert!(!vault.join("sessions").exists());
    }

    #[tokio::test]
    async fn transcribe_fails_cleanly_when_the_meeting_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join("sessions")).unwrap();

        let error = run(Args {
            base: None,
            vault_path: Some(vault),
            json: false,
            command: cli::Command::Transcribe {
                id: "missing".to_string(),
            },
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "not_found");
        assert_eq!(error.exit_code(), 2);
    }

    #[tokio::test]
    async fn transcribe_fails_cleanly_when_the_meeting_has_no_audio() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session(&vault, "meeting-1", None);

        let error = run(Args {
            base: None,
            vault_path: Some(vault),
            json: false,
            command: cli::Command::Transcribe {
                id: "meeting-1".to_string(),
            },
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "not_found");
        assert!(error.to_string().contains("audio recording"));
    }

    #[tokio::test]
    async fn transcribe_fails_cleanly_without_a_configured_stt_model() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session(&vault, "meeting-1", None);
        std::fs::write(vault.join("sessions/meeting-1/audio.mp3"), b"mp3").unwrap();

        let error = run(Args {
            base: None,
            vault_path: Some(vault),
            json: false,
            command: cli::Command::Transcribe {
                id: "meeting-1".to_string(),
            },
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "operation_failed");
        assert!(
            error
                .to_string()
                .contains("no speech-to-text model is configured"),
            "unexpected message: {error}"
        );
    }

    #[tokio::test]
    async fn transcribe_rejects_a_provider_the_cli_does_not_support() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session(&vault, "meeting-1", None);
        std::fs::write(vault.join("sessions/meeting-1/audio.mp3"), b"mp3").unwrap();
        std::fs::write(
            vault.join("config.json"),
            serde_json::json!({
                "current_stt_provider": "deepgram",
                "current_stt_model": "nova-3",
            })
            .to_string(),
        )
        .unwrap();

        let error = run(Args {
            base: None,
            vault_path: Some(vault),
            json: false,
            command: cli::Command::Transcribe {
                id: "meeting-1".to_string(),
            },
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "operation_failed");
        assert!(error.to_string().contains("not supported by the CLI"));
    }

    #[tokio::test]
    async fn import_transcribe_reports_the_meeting_id_when_transcription_fails() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();
        // A provider the CLI cannot serve makes the chained transcription fail
        // deterministically without touching any model cache.
        std::fs::write(
            vault.join("config.json"),
            serde_json::json!({
                "current_stt_provider": "deepgram",
                "current_stt_model": "nova-3",
            })
            .to_string(),
        )
        .unwrap();
        let audio_path = dir.path().join("standup.wav");
        write_test_wav(&audio_path);

        let exit_code = run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: true,
            command: cli::Command::Import {
                file: audio_path,
                title: None,
                into: None,
                transcribe: true,
                created_at: None,
                started_at: None,
                ended_at: None,
                author: None,
                skill: None,
            },
        })
        .await
        .unwrap();

        // Partial failure: the import stuck, the exit code did not stay 0.
        assert_eq!(exit_code, 1);
        let sessions = std::fs::read_dir(vault.join("sessions"))
            .unwrap()
            .map(|entry| entry.unwrap())
            .collect::<Vec<_>>();
        assert_eq!(sessions.len(), 1);
        assert!(
            std::fs::metadata(sessions[0].path().join("audio.mp3"))
                .unwrap()
                .len()
                > 0
        );
        assert!(!sessions[0].path().join("transcript.json").exists());
    }

    #[tokio::test]
    async fn note_set_replaces_and_append_concatenates_with_a_newline() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session(&vault, "meeting-1", Some("Original body"));
        let set_path = dir.path().join("set.md");
        std::fs::write(&set_path, "Replaced body").unwrap();
        let append_path = dir.path().join("append.md");
        std::fs::write(&append_path, "Appended line").unwrap();

        run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: false,
            command: cli::Command::Sessions {
                command: cli::MeetingCommand::Note {
                    id: "meeting-1".to_string(),
                    kind: cli::DocumentKind::Note,
                    set: Some(set_path),
                    append: None,
                },
            },
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(vault.join("sessions/meeting-1/notes.md")).unwrap(),
            "Replaced body"
        );

        run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: false,
            command: cli::Command::Sessions {
                command: cli::MeetingCommand::Note {
                    id: "meeting-1".to_string(),
                    kind: cli::DocumentKind::Note,
                    set: None,
                    append: Some(append_path),
                },
            },
        })
        .await
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(vault.join("sessions/meeting-1/notes.md")).unwrap(),
            "Replaced body\nAppended line"
        );
    }

    #[tokio::test]
    async fn note_edit_fails_cleanly_when_the_session_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join("sessions")).unwrap();
        let set_path = dir.path().join("set.md");
        std::fs::write(&set_path, "body").unwrap();

        let error = run(Args {
            base: None,
            vault_path: Some(vault.clone()),
            json: false,
            command: cli::Command::Sessions {
                command: cli::MeetingCommand::Note {
                    id: "missing".to_string(),
                    kind: cli::DocumentKind::Note,
                    set: Some(set_path),
                    append: None,
                },
            },
        })
        .await
        .unwrap_err();

        assert_eq!(error.code(), "not_found");
        assert_eq!(error.exit_code(), 2);
        assert!(!vault.join("sessions/missing").exists());
    }

    #[tokio::test]
    async fn tag_add_normalizes_meta_tags_and_registers_them() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session_with_tags(&vault, "meeting-1", &["Existing"]);

        run(meetings_args(
            &vault,
            cli::MeetingCommand::Tag {
                command: cli::TagCommand::Add {
                    id: "meeting-1".to_string(),
                    tags: vec!["#Hiring".to_string(), "Project-X".to_string()],
                },
            },
        ))
        .await
        .unwrap();

        // The whole tag list is rewritten normalized and sorted, including the
        // pre-existing raw entry.
        assert_eq!(
            read_meta_tags(&vault, "meeting-1"),
            vec!["existing", "hiring", "project-x"]
        );
        // Only the newly added tags are registered; the pre-existing one is not.
        let mut registry = read_registry_ids(&vault);
        registry.sort();
        assert_eq!(registry, vec!["hiring", "project-x"]);
    }

    #[tokio::test]
    async fn tag_add_is_idempotent_and_remove_never_touches_the_registry() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session_with_tags(&vault, "meeting-1", &["hiring"]);

        // Adding an already-present tag is a no-op: no registry write happens.
        run(meetings_args(
            &vault,
            cli::MeetingCommand::Tag {
                command: cli::TagCommand::Add {
                    id: "meeting-1".to_string(),
                    tags: vec!["HIRING".to_string()],
                },
            },
        ))
        .await
        .unwrap();
        assert_eq!(read_meta_tags(&vault, "meeting-1"), vec!["hiring"]);
        assert!(!vault.join("tags.json").exists());

        // Removing an absent tag exits 0 and changes nothing.
        run(meetings_args(
            &vault,
            cli::MeetingCommand::Tag {
                command: cli::TagCommand::Remove {
                    id: "meeting-1".to_string(),
                    tags: vec!["ghost".to_string()],
                },
            },
        ))
        .await
        .unwrap();
        assert_eq!(read_meta_tags(&vault, "meeting-1"), vec!["hiring"]);

        // A real removal updates _meta.json but leaves the append-only registry
        // alone.
        run(meetings_args(
            &vault,
            cli::MeetingCommand::Tag {
                command: cli::TagCommand::Remove {
                    id: "meeting-1".to_string(),
                    tags: vec!["#Hiring".to_string()],
                },
            },
        ))
        .await
        .unwrap();
        assert!(read_meta_tags(&vault, "meeting-1").is_empty());
        assert!(!vault.join("tags.json").exists());
    }

    #[tokio::test]
    async fn tag_edit_fails_cleanly_when_the_meeting_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(vault.join("sessions")).unwrap();

        let error = run(meetings_args(
            &vault,
            cli::MeetingCommand::Tag {
                command: cli::TagCommand::Add {
                    id: "missing".to_string(),
                    tags: vec!["hiring".to_string()],
                },
            },
        ))
        .await
        .unwrap_err();

        assert_eq!(error.code(), "not_found");
        assert_eq!(error.exit_code(), 2);
        assert!(!vault.join("tags.json").exists());
    }

    #[tokio::test]
    async fn tag_edit_rejects_tags_that_normalize_to_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session_with_tags(&vault, "meeting-1", &[]);

        let error = run(meetings_args(
            &vault,
            cli::MeetingCommand::Tag {
                command: cli::TagCommand::Add {
                    id: "meeting-1".to_string(),
                    tags: vec!["#".to_string()],
                },
            },
        ))
        .await
        .unwrap_err();

        assert_eq!(error.code(), "operation_failed");
        assert!(error.to_string().contains("tag name cannot be empty"));
        assert!(read_meta_tags(&vault, "meeting-1").is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn per_id_get_path_and_export_do_not_enumerate_sessions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session(&vault, "target", Some("bounded lookup"));
        let root = vault.join("sessions");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o111)).unwrap();
        assert!(std::fs::read_dir(&root).is_err());
        for id in ["target", "missing"] {
            for command in [
                cli::MeetingCommand::Get { id: id.into() },
                cli::MeetingCommand::Path { id: id.into() },
                cli::MeetingCommand::Export {
                    id: id.into(),
                    format: cli::ExportFormat::Json,
                    output: Some(dir.path().join(format!("{id}.json"))),
                    force: false,
                },
            ] {
                let result = run(meetings_args(&vault, command)).await;
                if id == "target" {
                    result.unwrap();
                } else {
                    assert_eq!(result.unwrap_err().code(), "not_found");
                }
            }
        }
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[tokio::test]
    async fn path_command_resolves_the_session_directory() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        write_session(&vault, "meeting-1", None);

        // The command prints the path; the resolver it uses must agree with the
        // on-disk layout.
        run(meetings_args(
            &vault,
            cli::MeetingCommand::Path {
                id: "meeting-1".to_string(),
            },
        ))
        .await
        .unwrap();

        let error = run(meetings_args(
            &vault,
            cli::MeetingCommand::Path {
                id: "missing".to_string(),
            },
        ))
        .await
        .unwrap_err();
        assert_eq!(error.code(), "not_found");
        assert_eq!(error.exit_code(), 2);
    }

    #[tokio::test]
    async fn tags_list_reflects_tags_registered_at_meeting_creation() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        std::fs::create_dir_all(&vault).unwrap();

        run(meetings_args(
            &vault,
            cli::MeetingCommand::New {
                title: "Kickoff".to_string(),
                note: None,
                created_at: None,
                started_at: None,
                ended_at: None,
                tag: vec!["#Hiring".to_string(), "project-x".to_string()],
                author: None,
                skill: None,
            },
        ))
        .await
        .unwrap();

        let mut registry = read_registry_ids(&vault);
        registry.sort();
        assert_eq!(registry, vec!["hiring", "project-x"]);

        run(Args {
            base: None,
            vault_path: Some(vault),
            json: true,
            command: cli::Command::Tags {
                command: cli::TagsCommand::List,
            },
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn export_command_reads_an_existing_vault_without_writing_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let vault = dir.path().join("vault");
        let output_path = dir.path().join("meeting.md");
        let session_dir = vault.join("sessions/meeting-1");
        std::fs::create_dir_all(&session_dir).unwrap();
        std::fs::write(
            session_dir.join("_meta.json"),
            serde_json::json!({
                "id": "meeting-1",
                "title": "Planning",
                "started_at": "2026-07-13",
                "ended_at": null,
                "created_at": "2026-07-13T00:00:00Z",
                "tags": [],
            })
            .to_string(),
        )
        .unwrap();
        // Legacy note name on purpose: export must still read `_memo.md` vaults.
        std::fs::write(session_dir.join("_memo.md"), "Decide the launch date.").unwrap();

        run(Args {
            base: None,
            vault_path: Some(vault),
            json: false,
            command: cli::Command::Sessions {
                command: cli::MeetingCommand::Export {
                    id: "meeting-1".to_string(),
                    format: cli::ExportFormat::Markdown,
                    output: Some(output_path.clone()),
                    force: false,
                },
            },
        })
        .await
        .unwrap();

        let exported = std::fs::read_to_string(output_path).unwrap();
        assert!(exported.contains("# Planning"));
        assert!(exported.contains("Decide the launch date."));
    }
}
