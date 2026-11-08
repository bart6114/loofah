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
