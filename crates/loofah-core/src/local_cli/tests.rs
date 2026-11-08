use super::*;
use crate::VaultRuntime;
use hypr_storage::vault::{LeaseAttempt, VaultLease};
use std::time::Duration;

struct Fixture {
    _root: tempfile::TempDir,
    runtime: Option<VaultRuntime>,
    cli: Arc<LocalCli>,
}
impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let vault = root.path().join("vault");
        std::fs::create_dir(&vault).unwrap();
        let lease = match VaultLease::try_acquire_existing(&vault).unwrap() {
            LeaseAttempt::Acquired(lease) => lease,
            _ => panic!("unexpected owner"),
        };
        let runtime = VaultRuntime::open_leased(
            "local:vault".into(),
            lease,
            &root.path().join("cache"),
            &root.path().join("operations"),
            hypr_vault_write::ContentLimits::unlimited(),
        )
        .await
        .unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            runtime.context.startup.wait_until_ready(),
        )
        .await
        .unwrap()
        .unwrap();
        let cli = Arc::new(LocalCli::new(runtime.context.clone()).unwrap());
        Self {
            _root: root,
            runtime: Some(runtime),
            cli,
        }
    }
    async fn execute(&self, op: LocalCliOperation) -> LocalCliOutput {
        self.cli
            .execute(uuid::Uuid::new_v4().to_string(), op, None, Arc::new(|_| {}))
            .await
            .unwrap()
    }
    async fn create(&self) -> String {
        let result = self
            .execute(LocalCliOperation::CreateSession {
                title: "Original".into(),
                markdown: Some("alpha".into()),
                metadata: LocalInitialSessionMetadata::default(),
            })
            .await;
        let LocalCliReply::SessionCreated { meta } = result.reply else {
            panic!("wrong reply")
        };
        meta.id
    }
    async fn stop(mut self) {
        drop(self.cli);
        self.runtime.take().unwrap().shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn shared_cli_routes_rich_creation_metadata_append_and_tag_deltas_through_cas() {
    let fixture = Fixture::new().await;
    let result = fixture
        .execute(LocalCliOperation::CreateSession {
            title: "Original".into(),
            markdown: Some("alpha".into()),
            metadata: LocalInitialSessionMetadata {
                created_at: Some("2026-11-08T10:00:00+01:00".into()),
                started_at: Some("2026-11-08T09:00:00Z".into()),
                ended_at: None,
                tags: vec!["#RawTag".into()],
                author: Some("agent".into()),
                skill: Some("meeting".into()),
            },
        })
        .await;
    let LocalCliReply::SessionCreated { meta } = result.reply else {
        panic!("wrong reply")
    };
    assert_eq!(meta.created_at, "2026-11-08T09:00:00.000Z");
    assert_eq!(meta.tags, ["#RawTag"]);
    assert_eq!(meta.skill.as_deref(), Some("meeting"));
    let id = meta.id;
    let context = &fixture.cli.context;
    let note = context.read_note(&id).await.unwrap();
    context
        .save_note(
            &id,
            SaveNoteRequest {
                markdown: "newer".into(),
                title: None,
                expected_revision: note.revision.clone(),
            },
        )
        .await
        .unwrap();
    let result = fixture
        .execute(LocalCliOperation::AppendNote {
            session_id: id.clone(),
            markdown: "do not overwrite".into(),
            expected_revision: note.revision,
        })
        .await;
    assert!(matches!(
        result.reply,
        LocalCliReply::NoteSaved {
            outcome: NoteSaveOutcome::Conflict(_)
        }
    ));
    let current = context.read_note(&id).await.unwrap();
    let result = fixture
        .execute(LocalCliOperation::AppendNote {
            session_id: id.clone(),
            markdown: "appended".into(),
            expected_revision: current.revision,
        })
        .await;
    assert!(
        matches!(result.reply,LocalCliReply::NoteSaved{outcome:NoteSaveOutcome::Saved(NoteSnapshot{markdown:Some(ref text),..})} if text=="newer\nappended")
    );
    let metadata = context.read_metadata(&id).await.unwrap();
    let result = fixture
        .execute(LocalCliOperation::TagDelta {
            session_id: id.clone(),
            tags: vec!["#ADDED".into()],
            remove: false,
            expected_revision: metadata.revision.clone(),
        })
        .await;
    assert!(
        matches!(result.reply,LocalCliReply::TagDelta{outcome:MetadataSaveOutcome::Saved(_),ref changed,ref tags} if changed==&["added"]&&tags==&["added","rawtag"])
    );
    let result = fixture
        .execute(LocalCliOperation::TagDelta {
            session_id: id.clone(),
            tags: vec!["rawtag".into()],
            remove: true,
            expected_revision: metadata.revision,
        })
        .await;
    assert!(matches!(
        result.reply,
        LocalCliReply::TagDelta {
            outcome: MetadataSaveOutcome::Conflict(_),
            ..
        }
    ));
    assert_eq!(
        context.read_metadata(&id).await.unwrap().meta.tags,
        ["added", "rawtag"]
    );
    fixture.stop().await;
}

#[tokio::test]
async fn shared_cli_deletion_preserves_unknown_attachments_and_replays_historical_details_after_restore()
 {
    let fixture = Fixture::new().await;
    let id = fixture.create().await;
    let context = &fixture.cli.context;
    let source = context.store.vault_base().join("sessions").join(&id);
    std::fs::write(source.join("user-attachment.bin"), b"preserve").unwrap();
    let snapshot = context.read_session_lifecycle(&id).await.unwrap();
    let input = DeleteSessionRequest {
        session_id: id.clone(),
        expected_revision: snapshot.revision,
    };
    let request = uuid::Uuid::new_v4().to_string();
    let op = LocalCliOperation::DeleteSession {
        session_id: id.clone(),
        expected_revision: input.expected_revision.clone(),
    };
    let result = fixture
        .cli
        .execute(request.clone(), op.clone(), None, Arc::new(|_| {}))
        .await
        .unwrap();
    let LocalCliReply::SessionDeleted {
        path,
        trash_path,
        deleted_at,
        ..
    } = result.reply
    else {
        panic!("wrong reply")
    };
    assert_eq!(path, source);
    assert_eq!(
        std::fs::read(trash_path.join("user-attachment.bin")).unwrap(),
        b"preserve"
    );
    let DeleteSessionOutcome::Trashed {
        trash_token,
        revision,
    } = context
        .delete_session(LOCAL_CLI_NAMESPACE.into(), request.clone(), input)
        .await
        .unwrap()
    else {
        panic!("missing receipt")
    };
    let restored = context
        .restore_session(
            LOCAL_CLI_NAMESPACE.into(),
            uuid::Uuid::new_v4().to_string(),
            RestoreSessionRequest {
                trash_token,
                expected_revision: revision,
            },
        )
        .await
        .unwrap();
    assert!(matches!(restored, RestoreSessionOutcome::Restored { .. }));
    let result = fixture
        .cli
        .execute(request, op, None, Arc::new(|_| {}))
        .await
        .unwrap();
    assert!(
        matches!(result.reply,LocalCliReply::SessionDeleted{deleted_at:ref at,..} if at==&deleted_at)
    );
    assert_eq!(
        std::fs::read(source.join("user-attachment.bin")).unwrap(),
        b"preserve"
    );
    assert_eq!(
        context.read_note(&id).await.unwrap().markdown.as_deref(),
        Some("alpha")
    );
    fixture.stop().await;
}

#[tokio::test]
async fn unchecked_local_import_and_transcription_are_not_advertised_or_dispatched() {
    let fixture = Fixture::new().await;
    for op in [
        LocalCliOperation::Import {
            filename: "meeting.mp3".into(),
            title: Some("retained".into()),
            into: None,
            transcribe: true,
            metadata: LocalInitialSessionMetadata::default(),
        },
        LocalCliOperation::Transcribe {
            session_id: "missing".into(),
        },
    ] {
        assert!(!fixture.cli.operations().contains(&op.kind()));
        let cause = fixture
            .cli
            .execute(uuid::Uuid::new_v4().to_string(), op, None, Arc::new(|_| {}))
            .await
            .err()
            .unwrap();
        assert_eq!(cause.code, ErrorCode::Unavailable);
        assert_eq!(cause.mutation_outcome, Some(MutationOutcome::NotCommitted));
    }
    assert!(fixture.cli.context.store.session_list().is_empty());
    fixture.stop().await;
}

#[tokio::test]
async fn cancelling_cli_waiter_keeps_owned_append_until_the_actual_store_transaction_finishes() {
    let fixture = Fixture::new().await;
    let id = fixture.create().await;
    let before = fixture.cli.context.read_note(&id).await.unwrap();
    let snapshot = fixture.cli.context.store.lock_snapshot().await;
    let dispatcher = fixture.cli.clone();
    let requested = id.clone();
    let waiter = tokio::spawn(async move {
        dispatcher
            .execute(
                uuid::Uuid::new_v4().to_string(),
                LocalCliOperation::AppendNote {
                    session_id: requested,
                    markdown: "retained".into(),
                    expected_revision: before.revision,
                },
                None,
                Arc::new(|_| {}),
            )
            .await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while Arc::strong_count(&fixture.cli) < 3 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    waiter.abort();
    let _ = waiter.await;
    drop(snapshot);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if fixture
                .cli
                .context
                .read_note(&id)
                .await
                .unwrap()
                .markdown
                .as_deref()
                == Some("alpha\nretained")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    fixture.stop().await;
}

#[tokio::test]
async fn local_tag_registry_failure_reports_the_confirmed_metadata_write() {
    let fixture = Fixture::new().await;
    let id = fixture.create().await;
    let context = &fixture.cli.context;
    let before = context.read_metadata(&id).await.unwrap();
    std::fs::write(
        context.store.vault_base().join("tags.json"),
        b"{invalid registry",
    )
    .unwrap();
    let error = fixture
        .cli
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::TagDelta {
                session_id: id.clone(),
                tags: vec!["new-tag".into()],
                remove: false,
                expected_revision: before.revision,
            },
            None,
            Arc::new(|_| {}),
        )
        .await
        .err()
        .expect("registry failure");
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::Persisted));
    assert!(!error.retryable);
    let after = context.read_metadata(&id).await.unwrap();
    assert_eq!(after.meta.tags, ["new-tag"]);
    assert_eq!(error.current_revision, Some(after.revision));
    assert!(error.message.contains(&id));
    assert_eq!(
        std::fs::read(context.store.vault_base().join("tags.json")).unwrap(),
        b"{invalid registry"
    );
    fixture.stop().await;
}

#[tokio::test]
async fn local_read_owner_recovers_recording_gates_without_config_or_global_startup() {
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    let session = vault.join("sessions/readable");
    std::fs::create_dir_all(&session).unwrap();
    let meta = br#"{"id":"readable","title":"Original","created_at":"2026-11-05T00:00:00Z","tags":[],"custom":{"retained":true}}"#;
    std::fs::write(session.join("_meta.json"), meta).unwrap();
    std::fs::write(session.join("notes.md"), "Canonical note").unwrap();
    std::fs::write(session.join("unknown-user-file"), "User bytes").unwrap();
    std::fs::write(
        vault.join("config.json"),
        br#"{"current_stt_provider":"parakeet","current_stt_model":"retired"}"#,
    )
    .unwrap();
    let config = std::fs::read(vault.join("config.json")).unwrap();
    let mut wav = hound::WavWriter::create(
        session.join("audio.wav"),
        hound::WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        },
    )
    .unwrap();
    wav.write_sample(0i16).unwrap();
    wav.finalize().unwrap();
    let LeaseAttempt::Acquired(lease) = VaultLease::try_acquire_existing(&vault).unwrap() else {
        panic!("owner");
    };
    let reader = LocalCli::open_read_owner(lease, root.path().join("operations"))
        .await
        .unwrap();
    assert!(
        reader
            .context
            .audio_metadata("readable")
            .await
            .unwrap()
            .is_some()
    );
    let output = reader
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::Note {
                session_id: "readable".into(),
            },
            None,
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert!(
        matches!(output.reply, LocalCliReply::Note { snapshot: NoteSnapshot { markdown:Some(ref body), .. } } if body == "Canonical note")
    );
    assert!(
        !reader
            .operations()
            .contains(&LocalCliOperationKind::SaveNote)
    );
    assert!(reader.context.search.cache().await.is_err());
    assert_eq!(std::fs::read(vault.join("config.json")).unwrap(), config);
    assert_eq!(std::fs::read(session.join("_meta.json")).unwrap(), meta);
    assert_eq!(
        std::fs::read_to_string(session.join("unknown-user-file")).unwrap(),
        "User bytes"
    );
    drop(reader);
    assert!(matches!(
        VaultLease::try_acquire_existing(&vault).unwrap(),
        LeaseAttempt::Acquired(_)
    ));
}

#[tokio::test]
async fn local_read_owner_preserves_uncertain_recording_gate_while_text_remains_readable() {
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    let session = vault.join("sessions/readable");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(
        session.join("_meta.json"),
        br#"{"id":"readable","title":"Original","created_at":"2026-11-05T00:00:00Z","tags":[]}"#,
    )
    .unwrap();
    std::fs::write(session.join("notes.md"), "Text survives").unwrap();
    let operations = root.path().join("operations");
    let publications = operations.join("recording-publications");
    std::fs::create_dir_all(&publications).unwrap();
    let issue = publications.join("recording-publication-invalid.json");
    std::fs::write(&issue, b"unrecognized receipt must survive").unwrap();
    let LeaseAttempt::Acquired(lease) = VaultLease::try_acquire_existing(&vault).unwrap() else {
        panic!("owner");
    };
    let reader = LocalCli::open_read_owner(lease, operations).await.unwrap();
    assert_eq!(
        reader
            .context
            .audio_metadata("readable")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    assert_eq!(
        reader
            .context
            .read_note("readable")
            .await
            .unwrap()
            .markdown
            .as_deref(),
        Some("Text survives")
    );
    assert_eq!(
        std::fs::read(&issue).unwrap(),
        b"unrecognized receipt must survive"
    );
}

#[tokio::test]
async fn local_tag_list_keeps_strict_registry_errors_instead_of_silent_empty_success() {
    let fixture = Fixture::new().await;
    fixture
        .cli
        .context
        .search
        .wait_until_initialized()
        .await
        .unwrap();
    let path = fixture.cli.context.store.vault_base().join("tags.json");
    std::fs::write(&path, b"{unreadable registry").unwrap();
    let error = fixture
        .cli
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::Tags {},
            None,
            Arc::new(|_| {}),
        )
        .await
        .err()
        .expect("corrupt registry must fail");
    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(std::fs::read(&path).unwrap(), b"{unreadable registry");
    fixture.stop().await;
}

struct TestMedia {
    normalizations: Arc<std::sync::atomic::AtomicUsize>,
    transcriptions: Arc<std::sync::atomic::AtomicUsize>,
    edit: Option<std::path::PathBuf>,
}
impl LocalMediaHost for TestMedia {
    fn normalize(
        &self,
        source: LocalCliArtifact,
        _: String,
        _: std::path::PathBuf,
        _: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<LocalCliArtifact> {
        self.normalizations
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async move { Ok(source) })
    }
    fn transcribe(
        &self,
        _: LocalCliArtifact,
        _: String,
        session_id: String,
        _: hypr_vault_read::SessionAudio,
        _: crate::settings::AppConfig,
        _: std::path::PathBuf,
        _: Arc<dyn Fn(LocalCliProgress) + Send + Sync>,
    ) -> LocalMediaFuture<TranscriptWithData> {
        self.transcriptions
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let edit = self.edit.clone();
        Box::pin(async move {
            if let Some(path) = edit {
                std::fs::write(path, br#"{"transcripts":[],"speaker_edit":"preserved"}"#).unwrap();
            }
            Ok(serde_json::from_value(serde_json::json!({
                "id":"generated", "user_id":"owner", "created_at":"2026-11-08T10:00:00Z", "session_id":session_id,
                "started_at":0, "ended_at":1000, "memo_md":"", "words":[{"id":"word", "text":"Hello", "start_ms":0, "end_ms":1000, "channel":0}], "speaker_hints":[]
            })).unwrap())
        })
    }
}
fn input_artifact(root: &std::path::Path) -> LocalCliArtifact {
    use sha2::{Digest, Sha256};
    let path = root.join(format!("input-{}.mp3", uuid::Uuid::new_v4()));
    std::fs::write(&path, b"test normalized MP3").unwrap();
    LocalCliArtifact::new(
        std::fs::File::open(path).unwrap(),
        19,
        format!("{:x}", Sha256::digest(b"test normalized MP3")),
        None,
        Box::new(()),
    )
}

#[tokio::test]
async fn owned_import_and_transcription_replay_the_original_reply_without_duplicate_engines() {
    let mut fixture = Fixture::new().await;
    let normalizations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let transcriptions = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    fixture.cli = Arc::new(
        LocalCli::new(fixture.cli.context.clone())
            .unwrap()
            .with_media_host(Arc::new(TestMedia {
                normalizations: normalizations.clone(),
                transcriptions: transcriptions.clone(),
                edit: None,
            })),
    );
    let request_id = uuid::Uuid::new_v4().to_string();
    let operation = LocalCliOperation::Import {
        filename: "Meeting.mp3".into(),
        title: None,
        into: None,
        transcribe: true,
        metadata: LocalInitialSessionMetadata::default(),
    };
    let first = fixture
        .cli
        .execute(
            request_id.clone(),
            operation.clone(),
            Some(input_artifact(fixture._root.path())),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    let again = fixture
        .cli
        .execute(
            request_id.clone(),
            operation,
            Some(input_artifact(fixture._root.path())),
            Arc::new(|_| {}),
        )
        .await
        .unwrap();
    assert_eq!(
        serde_json::to_value(&first.reply).unwrap(),
        serde_json::to_value(&again.reply).unwrap()
    );
    assert_eq!(normalizations.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(transcriptions.load(std::sync::atomic::Ordering::SeqCst), 1);
    let LocalCliReply::Imported {
        meta,
        audio_path,
        transcription: Some(Ok(outcome)),
    } = first.reply
    else {
        panic!("expected import with transcription")
    };
    assert_eq!(std::fs::read(audio_path).unwrap(), b"test normalized MP3");
    assert_eq!(meta.title, "Meeting");
    assert_eq!(outcome.words, 1);
    let transcript = fixture
        .cli
        .context
        .store
        .vault_base()
        .join("sessions")
        .join(meta.id)
        .join("transcript.json");
    assert!(
        std::fs::read_to_string(transcript)
            .unwrap()
            .contains("Hello")
    );
    fixture.stop().await;
}

#[tokio::test]
async fn owned_transcription_preserves_concurrent_speaker_edit_and_retains_generated_result() {
    let mut fixture = Fixture::new().await;
    let session = fixture.create().await;
    let directory = fixture
        .cli
        .context
        .store
        .vault_base()
        .join("sessions")
        .join(&session);
    std::fs::write(directory.join("audio.mp3"), b"recording").unwrap();
    std::fs::write(directory.join("transcript.json"), br#"{"transcripts":[]}"#).unwrap();
    fixture.cli = Arc::new(
        LocalCli::new(fixture.cli.context.clone())
            .unwrap()
            .with_media_host(Arc::new(TestMedia {
                normalizations: Default::default(),
                transcriptions: Default::default(),
                edit: Some(directory.join("transcript.json")),
            })),
    );
    let error = fixture
        .cli
        .execute(
            uuid::Uuid::new_v4().to_string(),
            LocalCliOperation::Transcribe {
                session_id: session,
            },
            None,
            Arc::new(|_| {}),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::Conflict);
    assert_eq!(
        std::fs::read(directory.join("transcript.json")).unwrap(),
        br#"{"transcripts":[],"speaker_edit":"preserved"}"#
    );
    let root = fixture
        .cli
        .context
        .operations
        .as_ref()
        .unwrap()
        .root
        .join("transcript-publications");
    assert!(std::fs::read_dir(root).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("transcript-result-")
    }));
    fixture.stop().await;
}

#[tokio::test]
async fn trusted_local_metadata_and_agent_queries_do_not_inherit_remote_budgets() {
    let fixture = Fixture::new().await;
    fixture
        .cli
        .context
        .search
        .wait_until_initialized()
        .await
        .unwrap();
    let session = fixture.create().await;
    let current = fixture.cli.context.read_metadata(&session).await.unwrap();
    let input = SaveMetadataRequest {
        expected_revision: current.revision,
        patch: SessionMetaPatch {
            title: Some("t".repeat(4097)),
            tags: Some((0..65).map(|n| format!("tag{n}")).collect()),
            ..Default::default()
        },
    };
    assert_eq!(
        fixture
            .cli
            .context
            .save_metadata(&session, input.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
    let result = fixture
        .execute(LocalCliOperation::SaveMetadata {
            session_id: session,
            input,
        })
        .await;
    assert!(matches!(
        result.reply,
        LocalCliReply::MetadataSaved {
            outcome: MetadataSaveOutcome::Saved(_)
        }
    ));
    let input: SearchMeetingsInput = serde_json::from_value(
        serde_json::json!({"query":"x".repeat(8193), "offset":10001, "limit":1}),
    )
    .unwrap();
    assert_eq!(
        fixture
            .cli
            .context
            .agent_search(input.clone())
            .await
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimit
    );
    fixture
        .execute(LocalCliOperation::AgentSearch { input })
        .await;
    fixture.stop().await;
}
