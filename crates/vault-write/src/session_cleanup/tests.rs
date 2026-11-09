use super::*;
use serde_json::json;
use std::sync::Arc;

async fn fixture() -> (
    tempfile::TempDir,
    tempfile::TempDir,
    SessionStore,
    SessionCleanupKey,
) {
    let vault = tempfile::tempdir().unwrap();
    let operations = tempfile::tempdir().unwrap();
    let store = SessionStore::new(vault.path().into());
    let meta = serde_json::from_value(json!({"id":"note","title":"","created_at":"2026-11-07T10:00:00Z","tags":[],"future":{"keep":true}})).unwrap();
    store
        .create_note_session(&meta, "  &nbsp;  ")
        .await
        .unwrap();
    let key = SessionCleanupKey {
        vault_id: "vault-a".into(),
        namespace: "member-a".into(),
        request_id: uuid::Uuid::new_v4(),
    };
    (vault, operations, store, key)
}

fn input(id: &str) -> DeleteEmptySessionRequest {
    DeleteEmptySessionRequest {
        session_id: id.into(),
    }
}
async fn cleanup(
    store: &SessionStore,
    operations: &Path,
    key: &SessionCleanupKey,
) -> Result<DeleteEmptySessionOutcome, SessionCleanupError> {
    store
        .delete_empty_session(
            operations,
            Some(&operations.join("document-saves")),
            key.clone(),
            input("note"),
        )
        .await
}
fn receipt_path(operations: &Path) -> PathBuf {
    std::fs::read_dir(operations)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap()
}
fn receipt_json(operations: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(receipt_path(operations)).unwrap()).unwrap()
}
fn trash(vault: &Path, operations: &Path) -> PathBuf {
    let receipt = receipt_json(operations);
    vault.join(receipt["state"]["trash"].as_str().unwrap())
}

#[tokio::test]
async fn empty_cleanup_trashes_all_bytes_preserves_ignored_entries_and_supports_local_undo() {
    let (vault, operations, store, key) = fixture().await;
    let source = vault.path().join("sessions/note");
    for (name, bytes) in [
        (".hidden", b"hidden".as_slice()),
        ("audio.wav", b"recording"),
        ("audio.peaks.json", b"peaks"),
    ] {
        std::fs::write(source.join(name), bytes).unwrap();
    }
    std::fs::create_dir(source.join("unknown-directory")).unwrap();
    std::fs::write(
        source.join("unknown-directory/user.bin"),
        b"uninterpreted bytes",
    )
    .unwrap();
    store.live.lock().await.insert(
        "note".into(),
        crate::transcript::LiveTranscriptBuffer::default(),
    );
    let events = store.subscribe_index_changes();
    assert_eq!(
        cleanup(&store, operations.path(), &key).await.unwrap(),
        DeleteEmptySessionOutcome::Deleted
    );
    assert!(!source.exists());
    assert!(store.session_get("note").is_none());
    assert!(!store.live.lock().await.contains_key("note"));
    let destination = store.recent_deletions.lock().unwrap()["note"]
        .trash_path
        .clone();
    assert_eq!(
        std::fs::read(destination.join("unknown-directory/user.bin")).unwrap(),
        b"uninterpreted bytes"
    );
    assert_eq!(
        std::fs::read(destination.join("audio.wav")).unwrap(),
        b"recording"
    );
    let mut events = events;
    let mut entities = Vec::new();
    while let Ok(event) = events.try_recv() {
        entities.push(event.entity);
    }
    assert!(entities.contains(&crate::IndexEntity::Sessions));
    assert!(entities.contains(&crate::IndexEntity::SessionHeaders));
    assert!(store.restore_session("note").await.unwrap());
    assert_eq!(
        std::fs::read(source.join("unknown-directory/user.bin")).unwrap(),
        b"uninterpreted bytes"
    );
    store
        .write_note("note", "Restored and edited")
        .await
        .unwrap();
    while events.try_recv().is_ok() {}
    assert_eq!(
        cleanup(&store, operations.path(), &key).await.unwrap(),
        DeleteEmptySessionOutcome::Deleted
    );
    assert_eq!(
        store.read_note("note").await.unwrap().as_deref(),
        Some("Restored and edited")
    );
    assert!(store.session_get("note").is_some());
    assert!(events.try_recv().is_err());
}

#[tokio::test]
async fn canonical_content_protects_notes_even_when_the_index_still_says_empty() {
    for kind in [
        "title",
        "note",
        "legacy_note",
        "tags",
        "transcript",
        "summary",
        "template",
    ] {
        let (vault, operations, store, key) = fixture().await;
        let source = vault.path().join("sessions/note");
        match kind {
            "title" | "tags" => {
                let mut meta: serde_json::Value = serde_json::from_slice(&std::fs::read(source.join("_meta.json")).unwrap()).unwrap();
                if kind == "title" { meta["title"] = json!("Canonical title"); } else { meta["tags"] = json!(["attached"]); }
                std::fs::write(source.join("_meta.json"), meta.to_string()).unwrap();
            }
            "note" => std::fs::write(source.join("notes.md"), "Canonical edit").unwrap(),
            "legacy_note" => { std::fs::remove_file(source.join("notes.md")).unwrap(); std::fs::write(source.join("_memo.md"), "Legacy edit").unwrap(); }
            "transcript" => std::fs::write(source.join("transcript.json"), json!({"transcripts":[{"id":"t","started_at":0.0,"ended_at":null,"words":[],"speaker_hints":[]}]}).to_string()).unwrap(),
            "summary" => std::fs::write(source.join("summary.md"), "").unwrap(),
            "template" => { std::fs::create_dir(source.join("enhanced")).unwrap(); std::fs::write(source.join("enhanced/doc.md"), "").unwrap(); }
            _ => unreachable!(),
        }
        assert!(
            store.session_is_empty("note").await.unwrap(),
            "index fixture unexpectedly changed for {kind}"
        );
        assert_eq!(
            cleanup(&store, operations.path(), &key).await.unwrap(),
            DeleteEmptySessionOutcome::NotEmpty,
            "{kind}"
        );
        assert!(source.join("_meta.json").is_file());
        assert!(!vault.path().join(".trash").exists());
    }
}

#[tokio::test]
async fn unknown_loose_and_managed_attachment_entries_are_never_removed() {
    for managed in [false, true] {
        let (vault, operations, store, key) = fixture().await;
        let source = vault.path().join("sessions/note");
        let attachment = if managed {
            std::fs::create_dir(source.join("attachments")).unwrap();
            source.join("attachments/user.bin")
        } else {
            source.join("user.bin")
        };
        std::fs::write(&attachment, b"attachment canary").unwrap();
        assert_eq!(
            cleanup(&store, operations.path(), &key).await.unwrap(),
            DeleteEmptySessionOutcome::NotEmpty
        );
        assert_eq!(std::fs::read(attachment).unwrap(), b"attachment canary");
    }
}

#[tokio::test]
async fn ignored_entries_cannot_make_cleanup_hold_the_write_lock_for_an_unbounded_directory() {
    for location in ["", "attachments", "enhanced"] {
        let (vault, operations, store, key) = fixture().await;
        let source = vault.path().join("sessions/note");
        let directory = source.join(location);
        std::fs::create_dir_all(&directory).unwrap();
        for index in 0..=MAX_DIRECTORY_ENTRIES {
            std::fs::write(directory.join(format!(".ignored-{index}")), []).unwrap();
        }
        assert!(
            matches!(
                cleanup(&store, operations.path(), &key).await,
                Err(SessionCleanupError::Rejected(StoreError::ResourceLimit(_)))
            ),
            "{location}"
        );
        assert!(source.join("notes.md").exists());
        assert!(
            directory
                .join(format!(".ignored-{MAX_DIRECTORY_ENTRIES}"))
                .exists()
        );
        assert!(!vault.path().join(".trash").exists());
        assert!(!operations.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        }));
    }
}

#[tokio::test]
async fn no_op_receipts_are_historical_and_uuid_input_identity_is_immutable() {
    let (vault, operations, store, key) = fixture().await;
    store
        .update_meta(
            "note",
            crate::SessionMetaPatch {
                title: Some("Keep".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        cleanup(&store, operations.path(), &key).await.unwrap(),
        DeleteEmptySessionOutcome::NotEmpty
    );
    store
        .update_meta(
            "note",
            crate::SessionMetaPatch {
                title: Some("".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        cleanup(&store, operations.path(), &key).await.unwrap(),
        DeleteEmptySessionOutcome::NotEmpty
    );
    assert!(vault.path().join("sessions/note").exists());
    assert!(matches!(
        store
            .delete_empty_session(operations.path(), None, key.clone(), input("other"))
            .await,
        Err(SessionCleanupError::Rejected(StoreError::Conflict(_)))
    ));
    let missing = SessionCleanupKey {
        request_id: uuid::Uuid::new_v4(),
        ..key.clone()
    };
    assert_eq!(
        store
            .delete_empty_session(operations.path(), None, missing.clone(), input("other"))
            .await
            .unwrap(),
        DeleteEmptySessionOutcome::NotFound
    );
    let meta = serde_json::from_value(
        json!({"id":"other","title":"","created_at":"2026-11-07T11:00:00Z","tags":[]}),
    )
    .unwrap();
    store.create_note_session(&meta, "").await.unwrap();
    assert_eq!(
        store
            .delete_empty_session(operations.path(), None, missing, input("other"))
            .await
            .unwrap(),
        DeleteEmptySessionOutcome::NotFound
    );
    assert!(vault.path().join("sessions/other").exists());
    let fresh = SessionCleanupKey {
        namespace: "member-b".into(),
        ..key
    };
    assert_eq!(
        cleanup(&store, operations.path(), &fresh).await.unwrap(),
        DeleteEmptySessionOutcome::Deleted
    );
}

#[tokio::test]
async fn busy_recording_audio_import_live_buffer_and_attachment_transfer_refuse_cleanup() {
    for kind in ["recording", "audio", "import", "live", "attachment"] {
        let (vault, operations, store, key) = fixture().await;
        let mut audio = None;
        match kind {
            "recording" => store.note_recording_active("note"),
            "audio" => audio = Some(store.lock_session_audio("note").await.unwrap()),
            "import" => store.begin_audio_import("note").await.unwrap(),
            "live" => {
                store.live.lock().await.insert(
                    "note".into(),
                    crate::transcript::LiveTranscriptBuffer {
                        dirty: true,
                        ..Default::default()
                    },
                );
            }
            "attachment" => {
                store
                    .attachment_stages
                    .lock()
                    .unwrap()
                    .insert("note".into(), 1);
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(
                cleanup(&store, operations.path(), &key).await,
                Err(SessionCleanupError::Busy)
            ),
            "{kind}"
        );
        assert!(vault.path().join("sessions/note").exists());
        assert!(!vault.path().join(".trash").exists());
        assert!(!operations.path().read_dir().unwrap().any(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        }));
        drop(audio);
    }
}

#[tokio::test]
async fn corrupt_unsafe_or_oversized_owned_content_fails_closed() {
    for name in [
        "_meta.json",
        "transcript.json",
        "tasks.json",
        "notes.md",
        "attachments",
    ] {
        let (vault, operations, store, key) = fixture().await;
        let source = vault.path().join("sessions/note");
        std::fs::write(
            source.join(name),
            if name == "notes.md" {
                vec![0xff]
            } else {
                b"broken".to_vec()
            },
        )
        .unwrap();
        assert!(
            matches!(
                cleanup(&store, operations.path(), &key).await,
                Err(SessionCleanupError::Rejected(_))
            ),
            "{name}"
        );
        assert!(source.exists());
        assert!(!vault.path().join(".trash").exists());
    }
    let (vault, operations, mut store, key) = fixture().await;
    store.content_limits.note_read_bytes = 1;
    assert!(matches!(
        cleanup(&store, operations.path(), &key).await,
        Err(SessionCleanupError::Rejected(StoreError::ResourceLimit(_)))
    ));
    assert!(vault.path().join("sessions/note").exists());
}

#[tokio::test]
async fn note_save_races_are_serialized_with_the_canonical_predicate_and_trash_move() {
    let (vault, operations, store, key) = fixture().await;
    let (checked_tx, checked_rx) = tokio::sync::oneshot::channel();
    let checked_tx = Arc::new(std::sync::Mutex::new(Some(checked_tx)));
    let release = Arc::new(tokio::sync::Notify::new());
    let task_store = store.clone();
    let task_root = operations.path().to_path_buf();
    let release_task = release.clone();
    let cleanup = tokio::spawn(async move {
        task_store
            .delete_empty_session_with(&task_root, None, key, input("note"), move |stage| {
                let release = release_task.clone();
                let checked = checked_tx.clone();
                async move {
                    if stage == Stage::Checked {
                        checked.lock().unwrap().take().unwrap().send(()).unwrap();
                        release.notified().await;
                    }
                    Ok(())
                }
            })
            .await
    });
    checked_rx.await.unwrap();
    let writer_store = store.clone();
    let writer =
        tokio::spawn(async move { writer_store.write_note("note", "Concurrent save").await });
    release.notify_one();
    assert_eq!(
        cleanup.await.unwrap().unwrap(),
        DeleteEmptySessionOutcome::Deleted
    );
    assert!(writer.await.unwrap().is_err());
    assert!(!vault.path().join("sessions/note").exists());
}

#[tokio::test]
async fn out_of_band_edit_after_intent_is_preserved_and_never_completed_by_replay() {
    let (vault, operations, store, key) = fixture().await;
    let note = vault.path().join("sessions/note/notes.md");
    let edit = note.clone();
    let result = store
        .delete_empty_session_with(
            operations.path(),
            None,
            key.clone(),
            input("note"),
            move |stage| {
                if stage == Stage::Intent {
                    std::fs::write(&edit, "External edit").unwrap();
                }
                async { Ok(()) }
            },
        )
        .await;
    assert!(matches!(result, Err(SessionCleanupError::Uncertain(_))));
    assert!(matches!(
        cleanup(&store, operations.path(), &key).await,
        Err(SessionCleanupError::Uncertain(_))
    ));
    assert_eq!(std::fs::read_to_string(note).unwrap(), "External edit");
}

#[tokio::test]
async fn boundary_failures_recover_only_proven_moves_and_never_repeat_a_pending_deletion() {
    for stage in [
        Stage::Intent,
        Stage::Renamed,
        Stage::Synced,
        Stage::Completed,
    ] {
        let (vault, operations, store, key) = fixture().await;
        let result = store
            .delete_empty_session_with(
                operations.path(),
                None,
                key.clone(),
                input("note"),
                move |at| async move {
                    if at == stage {
                        Err(StoreError::Io("injected boundary failure".into()))
                    } else {
                        Ok(())
                    }
                },
            )
            .await;
        assert!(matches!(result, Err(SessionCleanupError::Uncertain(_))));
        drop(store);
        let store = SessionStore::new(vault.path().into());
        if stage == Stage::Intent {
            assert!(matches!(
                cleanup(&store, operations.path(), &key).await,
                Err(SessionCleanupError::Uncertain(_))
            ));
            assert!(vault.path().join("sessions/note/notes.md").exists());
        } else {
            assert_eq!(
                cleanup(&store, operations.path(), &key).await.unwrap(),
                DeleteEmptySessionOutcome::Deleted
            );
            assert!(!vault.path().join("sessions/note").exists());
        }
    }
}

#[tokio::test]
async fn dotted_session_collision_uses_the_existing_trash_naming_and_recovers_after_restart() {
    let (vault, operations, store, key) = fixture().await;
    let meta = serde_json::from_value(json!({
        "id": "note.v2", "title": "", "created_at": "2026-11-07T11:00:00Z", "tags": []
    }))
    .unwrap();
    store.create_note_session(&meta, "").await.unwrap();
    let source = vault.path().join("sessions/note.v2");
    let collision =
        hypr_fs_sync_core::export::prepare_trash_destination(vault.path(), &source).unwrap();
    std::fs::create_dir(&collision).unwrap();
    std::fs::write(collision.join("user.bin"), b"existing trash canary").unwrap();
    assert!(matches!(
        store
            .delete_empty_session_with(
                operations.path(),
                None,
                key.clone(),
                input("note.v2"),
                |stage| async move {
                    if stage == Stage::Renamed {
                        Err(StoreError::Io("interrupted collision move".into()))
                    } else {
                        Ok(())
                    }
                },
            )
            .await,
        Err(SessionCleanupError::Uncertain(_))
    ));
    let moved = trash(vault.path(), operations.path());
    assert_eq!(moved.file_name().unwrap(), "note-1.v2");
    drop(store);
    let store = SessionStore::new(vault.path().into());
    assert_eq!(
        store
            .delete_empty_session(operations.path(), None, key, input("note.v2"))
            .await
            .unwrap(),
        DeleteEmptySessionOutcome::Deleted
    );
    assert!(!source.exists());
    assert!(moved.join("_meta.json").exists());
    assert_eq!(
        std::fs::read(collision.join("user.bin")).unwrap(),
        b"existing trash canary"
    );
    assert!(vault.path().join("sessions/note").exists());
}

#[tokio::test]
async fn source_reappearance_or_missing_trash_after_an_interrupted_move_stays_uncertain() {
    for reappear in [true, false] {
        let (vault, operations, store, key) = fixture().await;
        assert!(
            store
                .delete_empty_session_with(
                    operations.path(),
                    None,
                    key.clone(),
                    input("note"),
                    |stage| async move {
                        if stage == Stage::Renamed {
                            Err(StoreError::Io("stop".into()))
                        } else {
                            Ok(())
                        }
                    }
                )
                .await
                .is_err()
        );
        let destination = trash(vault.path(), operations.path());
        if reappear {
            assert!(store.restore_session("note").await.unwrap());
            store.write_note("note", "Restored edit").await.unwrap();
        } else {
            std::fs::rename(&destination, vault.path().join("retained-elsewhere")).unwrap();
        }
        drop(store);
        let store = SessionStore::new(vault.path().into());
        assert!(matches!(
            cleanup(&store, operations.path(), &key).await,
            Err(SessionCleanupError::Uncertain(_))
        ));
        if reappear {
            assert_eq!(
                store.read_note("note").await.unwrap().as_deref(),
                Some("Restored edit")
            );
        } else {
            assert!(vault.path().join("retained-elsewhere/notes.md").exists());
        }
    }
}

#[tokio::test]
async fn future_or_corrupt_receipts_never_remove_a_current_source() {
    for future in [true, false] {
        let (vault, operations, store, key) = fixture().await;
        store
            .update_meta(
                "note",
                crate::SessionMetaPatch {
                    title: Some("Keep".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            cleanup(&store, operations.path(), &key).await.unwrap(),
            DeleteEmptySessionOutcome::NotEmpty
        );
        let path = receipt_path(operations.path());
        if future {
            let mut value = receipt_json(operations.path());
            value["schema_version"] = json!(99);
            std::fs::write(&path, value.to_string()).unwrap();
        } else {
            std::fs::write(&path, "broken").unwrap();
        }
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            cleanup(&store, operations.path(), &key).await,
            Err(SessionCleanupError::Uncertain(_))
        ));
        assert_eq!(std::fs::read(path).unwrap(), before);
        assert!(vault.path().join("sessions/note").exists());
    }
}

#[tokio::test]
async fn killed_process_boundaries_leave_only_recoverable_trash_or_a_preserved_source() {
    for stage in ["intent", "renamed", "synced", "completed"] {
        let (vault, operations, store, key) = fixture().await;
        drop(store);
        let executable = std::env::current_exe().unwrap();
        let vault_path = vault.path().to_path_buf();
        let operations_path = operations.path().to_path_buf();
        let uuid = key.request_id.to_string();
        let result = tokio::task::spawn_blocking(move || {
            std::process::Command::new(executable)
                .args([
                    "--ignored",
                    "--exact",
                    "session_cleanup::tests::cleanup_crash_child",
                ])
                .env("LOOFAH_CLEANUP_TEST_VAULT", vault_path)
                .env("LOOFAH_CLEANUP_TEST_OPERATIONS", operations_path)
                .env("LOOFAH_CLEANUP_TEST_REQUEST", uuid)
                .env("LOOFAH_CLEANUP_TEST_STAGE", stage)
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert_eq!(
            result.status.code(),
            Some(91),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let store = SessionStore::new(vault.path().into());
        if stage == "intent" {
            assert!(matches!(
                cleanup(&store, operations.path(), &key).await,
                Err(SessionCleanupError::Uncertain(_))
            ));
            assert_eq!(
                store.read_note("note").await.unwrap().as_deref(),
                Some("  &nbsp;  ")
            );
        } else {
            assert_eq!(
                cleanup(&store, operations.path(), &key).await.unwrap(),
                DeleteEmptySessionOutcome::Deleted
            );
            assert!(!vault.path().join("sessions/note").exists());
        }
    }
}

#[tokio::test]
#[ignore = "subprocess fixture for abrupt cleanup termination"]
async fn cleanup_crash_child() {
    let Ok(vault) = std::env::var("LOOFAH_CLEANUP_TEST_VAULT") else {
        return;
    };
    let operations = PathBuf::from(std::env::var("LOOFAH_CLEANUP_TEST_OPERATIONS").unwrap());
    let selected = std::env::var("LOOFAH_CLEANUP_TEST_STAGE").unwrap();
    let key = SessionCleanupKey {
        vault_id: "vault-a".into(),
        namespace: "member-a".into(),
        request_id: std::env::var("LOOFAH_CLEANUP_TEST_REQUEST")
            .unwrap()
            .parse()
            .unwrap(),
    };
    SessionStore::new(vault.into())
        .delete_empty_session_with(&operations, None, key, input("note"), move |stage| {
            let name = match stage {
                Stage::Checked => "checked",
                Stage::Intent => "intent",
                Stage::Renamed => "renamed",
                Stage::Synced => "synced",
                Stage::Completed => "completed",
            };
            if name == selected {
                std::process::exit(91);
            }
            async { Ok(()) }
        })
        .await
        .unwrap();
    panic!("selected crash boundary was not reached");
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_note_and_attachment_paths_do_not_cross_the_vault_boundary() {
    for name in ["notes.md", "attachments"] {
        let (vault, operations, store, key) = fixture().await;
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("canary"), "outside").unwrap();
        let path = vault.path().join("sessions/note").join(name);
        if path.exists() {
            std::fs::remove_file(&path).unwrap();
        }
        std::os::unix::fs::symlink(outside.path(), &path).unwrap();
        assert!(matches!(
            cleanup(&store, operations.path(), &key).await,
            Err(SessionCleanupError::Rejected(_))
        ));
        assert_eq!(
            std::fs::read_to_string(outside.path().join("canary")).unwrap(),
            "outside"
        );
        assert!(vault.path().join("sessions/note").exists());
    }
}
