use super::*;
use serde_json::json;

async fn fixture() -> (tempfile::TempDir, SessionStore, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let vault = root.path().join("vault");
    std::fs::create_dir_all(&vault).unwrap();
    let store = SessionStore::new(vault);
    store.create_session_meta(&serde_json::from_value(json!({"id":"note","title":"Title","created_at":"2026-11-08T10:00:00Z","tags":[],"unknown":{"retain":true}})).unwrap()).await.unwrap();
    store.write_note("note", "Body").await.unwrap();
    let operations = root.path().join("private/lifecycle");
    (root, store, operations)
}
fn key() -> SessionLifecycleKey {
    SessionLifecycleKey {
        vault_id: "vault".into(),
        namespace: "member".into(),
        request_id: uuid::Uuid::new_v4(),
    }
}
async fn revision(store: &SessionStore) -> String {
    store
        .read_session_lifecycle(None, "note")
        .await
        .unwrap()
        .unwrap()
        .revision
}
async fn remove(store: &SessionStore, root: &Path, key: SessionLifecycleKey) -> (String, String) {
    let revision = revision(store).await;
    let result = store
        .delete_session_checked(
            root,
            None,
            key,
            DeleteSessionRequest {
                session_id: "note".into(),
                expected_revision: revision.clone(),
            },
        )
        .await
        .unwrap();
    let DeleteSessionOutcome::Trashed { trash_token, .. } = result else {
        panic!("{result:?}")
    };
    (trash_token, revision)
}
async fn restore(
    store: &SessionStore,
    root: &Path,
    token: String,
    revision: String,
    key: SessionLifecycleKey,
) -> Result<RestoreSessionOutcome, LifecycleError> {
    store
        .restore_session_checked(
            root,
            None,
            key,
            RestoreSessionRequest {
                trash_token: token,
                expected_revision: revision,
            },
            Some("note".into()),
        )
        .await
}

#[tokio::test]
async fn moves_and_restores_every_original_byte_without_rewriting_metadata_or_attachments() {
    let (_root, store, ops) = fixture().await;
    let directory = store.vault_base.join("sessions/note");
    let raw=br#"{"id":"note","title":"Title","created_at":"2026-11-08T10:00:00Z","tags":[],"future":{"nested":[1,2]}}"#;
    std::fs::write(directory.join("_meta.json"), raw).unwrap();
    std::fs::write(directory.join("minutes.md"), "user-owned note").unwrap();
    std::fs::create_dir_all(directory.join("attachments")).unwrap();
    std::fs::write(directory.join("attachments/image.png"), [1, 2, 3]).unwrap();
    std::fs::write(directory.join(".private"), "opaque").unwrap();
    let (token, revision) = remove(&store, &ops, key()).await;
    assert!(!directory.exists());
    assert!(matches!(
        restore(&store, &ops, token, revision, key()).await.unwrap(),
        RestoreSessionOutcome::Restored { .. }
    ));
    assert_eq!(std::fs::read(directory.join("_meta.json")).unwrap(), raw);
    assert_eq!(
        std::fs::read_to_string(directory.join("minutes.md")).unwrap(),
        "user-owned note"
    );
    assert_eq!(
        std::fs::read(directory.join("attachments/image.png")).unwrap(),
        [1, 2, 3]
    );
    assert_eq!(
        std::fs::read_to_string(directory.join(".private")).unwrap(),
        "opaque"
    );
}
#[tokio::test]
async fn stale_delete_conflicts_and_a_reused_uuid_never_changes_its_input_or_decision() {
    let (_root, store, ops) = fixture().await;
    let stale = revision(&store).await;
    store.write_note("note", "New body").await.unwrap();
    let k = key();
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: stale,
    };
    let outcome = store
        .delete_session_checked(&ops, None, k.clone(), input.clone())
        .await
        .unwrap();
    assert!(matches!(outcome, DeleteSessionOutcome::Conflict { .. }));
    let current = revision(&store).await;
    assert_eq!(
        store
            .delete_session_checked(&ops, None, k.clone(), input)
            .await
            .unwrap(),
        outcome
    );
    assert!(matches!(
        store
            .delete_session_checked(
                &ops,
                None,
                k,
                DeleteSessionRequest {
                    session_id: "note".into(),
                    expected_revision: current
                }
            )
            .await,
        Err(LifecycleError::Rejected(StoreError::Conflict(_)))
    ));
    assert!(!store.vault_base.join(".trash").exists());
}
#[tokio::test]
async fn replaying_a_completed_delete_after_restore_or_a_later_delete_never_moves_current_files() {
    let (_root, store, ops) = fixture().await;
    let k = key();
    let (first, revision) = remove(&store, &ops, k.clone()).await;
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: revision.clone(),
    };
    restore(&store, &ops, first.clone(), revision.clone(), key())
        .await
        .unwrap();
    let historical = store
        .delete_session_checked(&ops, None, k, input)
        .await
        .unwrap();
    assert_eq!(
        historical,
        DeleteSessionOutcome::Trashed {
            trash_token: first.clone(),
            revision: revision.clone()
        }
    );
    assert!(store.vault_base.join("sessions/note").exists());
    let (second, _) = remove(&store, &ops, key()).await;
    assert_ne!(first, second);
    assert!(matches!(
        restore(&store, &ops, first, revision, key()).await.unwrap(),
        RestoreSessionOutcome::NotFound
    ));
    assert!(!store.vault_base.join("sessions/note").exists());
}
#[tokio::test]
async fn restore_never_overwrites_an_occupied_original_destination() {
    let (_root, store, ops) = fixture().await;
    let (token, revision) = remove(&store, &ops, key()).await;
    let directory = store.vault_base.join("sessions/note");
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(directory.join("new-user-file"), "new instance").unwrap();
    let result = restore(&store, &ops, token, revision, key()).await.unwrap();
    assert!(matches!(
        result,
        RestoreSessionOutcome::Conflict {
            reason: RestoreSessionConflictReason::DestinationOccupied,
            ..
        }
    ));
    assert_eq!(
        std::fs::read_to_string(directory.join("new-user-file")).unwrap(),
        "new instance"
    );
}
#[tokio::test]
async fn recording_and_dirty_buffers_block_new_delete_without_receipts_or_trash_writes() {
    let (_root, store, ops) = fixture().await;
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: revision(&store).await,
    };
    store.note_recording_active("note");
    assert!(matches!(
        store.delete_session_checked(&ops, None, key(), input).await,
        Err(LifecycleError::Busy)
    ));
    assert!(!ops.exists());
    assert!(!store.vault_base.join(".trash").exists());
}
#[tokio::test]
async fn duplicate_metadata_keys_fail_closed_without_moving_original_bytes() {
    let (_root, store, ops) = fixture().await;
    let path = store.vault_base.join("sessions/note/_meta.json");
    let raw = br#"{"id":"note","id":"other","title":"Title","created_at":"2026-11-08","tags":[]}"#;
    std::fs::write(&path, raw).unwrap();
    assert!(store.read_session_lifecycle(None, "note").await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), raw);
    assert!(!ops.exists());
}
#[tokio::test]
async fn restart_and_lost_ack_reconcile_only_a_proven_finished_directory_move() {
    for stage in [Stage::Renamed, Stage::Synced, Stage::Completed] {
        let (_root, store, ops) = fixture().await;
        let k = key();
        let input = DeleteSessionRequest {
            session_id: "note".into(),
            expected_revision: revision(&store).await,
        };
        let result = store
            .delete_session_with(
                &ops,
                None,
                k.clone(),
                input.clone(),
                move |current| async move {
                    if current == stage {
                        Err(StoreError::Io("lost ack".into()))
                    } else {
                        Ok(())
                    }
                },
            )
            .await;
        assert!(matches!(result, Err(LifecycleError::Uncertain(_))));
        let cold = SessionStore::new(store.vault_base.clone());
        let result = cold
            .delete_session_checked(&ops, None, k, input)
            .await
            .unwrap();
        assert!(matches!(result, DeleteSessionOutcome::Trashed { .. }));
        assert!(!cold.vault_base.join("sessions/note").exists());
    }
}
#[tokio::test]
async fn an_intent_without_a_proven_move_never_redoes_a_deletion_after_restart() {
    let (_root, store, ops) = fixture().await;
    let k = key();
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: revision(&store).await,
    };
    assert!(
        store
            .delete_session_with(&ops, None, k.clone(), input.clone(), |stage| async move {
                if stage == Stage::Intent {
                    Err(StoreError::Io("stop".into()))
                } else {
                    Ok(())
                }
            })
            .await
            .is_err()
    );
    let cold = SessionStore::new(store.vault_base.clone());
    assert!(matches!(
        cold.delete_session_checked(&ops, None, k, input).await,
        Err(LifecycleError::Uncertain(_))
    ));
    assert!(cold.vault_base.join("sessions/note/_meta.json").exists());
}
#[tokio::test]
async fn restore_lost_ack_survives_restart_and_never_recreates_a_later_deleted_session() {
    let (_root, store, ops) = fixture().await;
    let (token, revision) = remove(&store, &ops, key()).await;
    let restore_key = key();
    let input = RestoreSessionRequest {
        trash_token: token,
        expected_revision: revision,
    };
    assert!(
        store
            .restore_session_with(
                &ops,
                None,
                restore_key.clone(),
                input.clone(),
                Some("note".into()),
                |stage| async move {
                    if stage == Stage::Renamed {
                        Err(StoreError::Io("lost".into()))
                    } else {
                        Ok(())
                    }
                }
            )
            .await
            .is_err()
    );
    let cold = SessionStore::new(store.vault_base.clone());
    let result = cold
        .restore_session_checked(
            &ops,
            None,
            restore_key.clone(),
            input.clone(),
            Some("note".into()),
        )
        .await
        .unwrap();
    assert!(matches!(result, RestoreSessionOutcome::Restored { .. }));
    remove(&cold, &ops, key()).await;
    assert_eq!(
        cold.restore_session_checked(&ops, None, restore_key, input, Some("note".into()))
            .await
            .unwrap(),
        result
    );
    assert!(!cold.vault_base.join("sessions/note").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn never_follows_owned_symlinks_and_preserves_opaque_user_symlinks() {
    let (root, store, ops) = fixture().await;
    let outside = root.path().join("outside");
    std::fs::write(&outside, "external").unwrap();
    let note = store.vault_base.join("sessions/note");
    std::fs::remove_file(note.join("notes.md")).unwrap();
    std::os::unix::fs::symlink(&outside, note.join("notes.md")).unwrap();
    assert!(store.read_session_lifecycle(None, "note").await.is_err());
    assert!(!ops.exists());
    std::fs::remove_file(note.join("notes.md")).unwrap();
    std::fs::write(note.join("notes.md"), "Body").unwrap();
    std::os::unix::fs::symlink(&outside, note.join("user-link")).unwrap();
    let (token, revision) = remove(&store, &ops, key()).await;
    restore(&store, &ops, token, revision, key()).await.unwrap();
    assert_eq!(std::fs::read_link(note.join("user-link")).unwrap(), outside);
    assert_eq!(std::fs::read_to_string(outside).unwrap(), "external");
}
#[tokio::test]
async fn changed_trash_requires_a_new_explicit_checked_restore_intent_and_keeps_original_decision()
{
    let (_root, store, ops) = fixture().await;
    let (token, old_revision) = remove(&store, &ops, key()).await;
    let deletion: DeleteReceipt = receipts::read(receipts::deletion_path(&ops, &token).unwrap())
        .await
        .unwrap()
        .unwrap();
    let target = deletion.target.unwrap();
    std::fs::write(
        store.vault_base.join(&target.trash).join("notes.md"),
        "Edited in Trash",
    )
    .unwrap();
    let old_key = key();
    let result = restore(
        &store,
        &ops,
        token.clone(),
        old_revision.clone(),
        old_key.clone(),
    )
    .await
    .unwrap();
    let RestoreSessionOutcome::Conflict {
        reason: RestoreSessionConflictReason::TrashChanged,
        current_revision: Some(current),
    } = result.clone()
    else {
        panic!("{result:?}")
    };
    assert_eq!(
        restore(&store, &ops, token.clone(), old_revision, old_key)
            .await
            .unwrap(),
        result
    );
    assert!(matches!(
        restore(&store, &ops, token, current, key()).await.unwrap(),
        RestoreSessionOutcome::Restored { .. }
    ));
    assert_eq!(
        std::fs::read_to_string(store.vault_base.join("sessions/note/notes.md")).unwrap(),
        "Edited in Trash"
    );
}
#[tokio::test]
async fn invalid_or_foreign_receipts_never_replace_current_files_or_get_reset() {
    let (_root, store, ops) = fixture().await;
    let k = key();
    let (token, revision) = remove(&store, &ops, k).await;
    let path = receipts::deletion_path(&ops, &token).unwrap();
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["schema_version"] = json!(99);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    let original = std::fs::read(&path).unwrap();
    assert!(restore(&store, &ops, token, revision, key()).await.is_err());
    assert_eq!(std::fs::read(path).unwrap(), original);
    assert!(!store.vault_base.join("sessions/note").exists());
}

#[tokio::test]
async fn changing_owned_files_after_durable_intent_never_moves_or_retries_the_session() {
    let (_root, store, ops) = fixture().await;
    let k = key();
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: revision(&store).await,
    };
    let note = store.vault_base.join("sessions/note/notes.md");
    let changed = note.clone();
    let result = store
        .delete_session_with(&ops, None, k.clone(), input.clone(), move |stage| {
            let changed = changed.clone();
            async move {
                if stage == Stage::Intent {
                    // Replace the locked generation; an in-place Windows write can truncate before failing.
                    hypr_storage::fs::atomic_write(&changed, "new external intent")
                        .map_err(io_error)?;
                }
                Ok(())
            }
        })
        .await;
    assert!(matches!(result, Err(LifecycleError::Uncertain(_))));
    assert_eq!(
        std::fs::read_to_string(&note).unwrap(),
        "new external intent"
    );
    assert!(matches!(
        store.delete_session_checked(&ops, None, k, input).await,
        Err(LifecycleError::Uncertain(_))
    ));
    assert_eq!(
        std::fs::read_to_string(note).unwrap(),
        "new external intent"
    );
}

#[tokio::test]
async fn local_compatibility_details_keep_original_paths_and_timestamp_after_restore_and_restart() {
    let (_root, store, ops) = fixture().await;
    let k = key();
    let rev = revision(&store).await;
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: rev.clone(),
    };
    assert!(
        store
            .session_delete_details(&ops, &k, &input)
            .await
            .unwrap()
            .is_none()
    );
    let outcome = store
        .delete_session_checked(&ops, None, k.clone(), input.clone())
        .await
        .unwrap();
    let DeleteSessionOutcome::Trashed { trash_token, .. } = outcome else {
        panic!()
    };
    let first = store
        .session_delete_details(&ops, &k, &input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.path, store.vault_base.join("sessions/note"));
    assert!(first.trash_path.exists());
    assert!(chrono::DateTime::parse_from_rfc3339(&first.deleted_at).is_ok());
    restore(&store, &ops, trash_token, rev, key())
        .await
        .unwrap();
    let cold = SessionStore::new(store.vault_base.clone());
    let replay = cold
        .session_delete_details(&ops, &k, &input)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(replay.path, first.path);
    assert_eq!(replay.trash_path, first.trash_path);
    assert_eq!(replay.deleted_at, first.deleted_at);
    assert!(replay.path.exists());
    assert!(!replay.trash_path.exists());
    let mut other = k;
    other.namespace = "other-member".into();
    assert!(
        cold.session_delete_details(&ops, &other, &input)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn a_retained_intent_never_reports_not_committed_when_later_admission_is_busy() {
    let (_root, store, ops) = fixture().await;
    let k = key();
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: revision(&store).await,
    };
    assert!(matches!(
        store
            .delete_session_with(&ops, None, k.clone(), input.clone(), |stage| async move {
                if stage == Stage::Intent {
                    Err(StoreError::Io("lost caller".into()))
                } else {
                    Ok(())
                }
            })
            .await,
        Err(LifecycleError::Uncertain(_))
    ));
    store.note_recording_active("note");
    assert!(matches!(
        store
            .delete_session_checked(&ops, None, k, input.clone())
            .await,
        Err(LifecycleError::Uncertain(_))
    ));
    assert!(matches!(
        store.delete_session_checked(&ops, None, key(), input).await,
        Err(LifecycleError::Busy)
    ));
    assert!(store.vault_base.join("sessions/note/notes.md").exists());
}

#[tokio::test]
async fn trusted_local_proof_is_bound_to_store_session_and_still_rejects_active_or_other_transients()
 {
    let (_root, store, _ops) = fixture().await;
    let hypr_storage::vault::LeaseAttempt::Acquired(lease) =
        hypr_storage::vault::VaultLease::try_acquire_existing(store.vault_base()).unwrap()
    else {
        panic!("fixture lease")
    };
    store.pin_vault_lease(&lease).unwrap();
    let directory = store.vault_base.join("sessions/note");
    std::fs::write(directory.join("audio_mic.wav"), b"retained raw").unwrap();
    let proof = || {
        store
            .checked_local_lifecycle_proof("note", lease.pin(), Box::new(()))
            .unwrap()
    };
    assert!(
        store
            .read_local_session_lifecycle(None, "other", proof())
            .await
            .is_err()
    );
    let other = SessionStore::new(store.vault_base.clone());
    assert!(
        other
            .read_local_session_lifecycle(None, "note", proof())
            .await
            .is_err()
    );
    store.note_recording_active("note");
    assert!(matches!(
        store
            .read_local_session_lifecycle(None, "note", proof())
            .await,
        Err(LifecycleError::Busy)
    ));
    store.release_recording_prepare("note").await.unwrap();
    assert!(
        store
            .read_local_session_lifecycle(None, "note", proof())
            .await
            .unwrap()
            .is_some()
    );
    std::fs::write(directory.join("audio.wav.tmp"), b"unconfirmed encode").unwrap();
    assert!(
        store
            .read_local_session_lifecycle(None, "note", proof())
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(directory.join("audio_mic.wav")).unwrap(),
        b"retained raw"
    );
    assert_eq!(
        std::fs::read(directory.join("audio.wav.tmp")).unwrap(),
        b"unconfirmed encode"
    );
}

#[tokio::test]
async fn local_host_guard_survives_cancelled_waiter_until_owned_trash_move_finishes() {
    use std::sync::Arc;
    let (_root, store, ops) = fixture().await;
    let hypr_storage::vault::LeaseAttempt::Acquired(lease) =
        hypr_storage::vault::VaultLease::try_acquire_existing(store.vault_base()).unwrap()
    else {
        panic!("fixture lease")
    };
    store.pin_vault_lease(&lease).unwrap();
    std::fs::write(
        store.vault_base.join("sessions/note/audio_spk.wav"),
        b"raw system",
    )
    .unwrap();
    let snapshot = store
        .read_local_session_lifecycle(
            None,
            "note",
            store
                .checked_local_lifecycle_proof("note", lease.pin(), Box::new(()))
                .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    let physical = Arc::new(tokio::sync::RwLock::new(()));
    let proof = store
        .checked_local_lifecycle_proof(
            "note",
            lease.pin(),
            Box::new(physical.clone().write_owned().await),
        )
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Semaphore::new(0));
    let k = key();
    let input = DeleteSessionRequest {
        session_id: "note".into(),
        expected_revision: snapshot.revision,
    };
    let task = {
        let store = store.clone();
        let ops = ops.clone();
        let k = k.clone();
        let input = input.clone();
        let entered = entered.clone();
        let release = release.clone();
        tokio::spawn(async move {
            store
                .delete_session_with_proof(&ops, None, k, input, Some(proof.0), move |stage| {
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        if stage == Stage::Intent {
                            entered.notify_one();
                            let _release = release.acquire().await.unwrap();
                        }
                        Ok(())
                    }
                })
                .await
        })
    };
    entered.notified().await;
    assert!(physical.clone().try_read_owned().is_err());
    task.abort();
    let _ = task.await;
    assert!(physical.clone().try_read_owned().is_err());
    release.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if physical.clone().try_read_owned().is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        SessionStore::session_delete_replay(&ops, &k, &input)
            .await
            .unwrap(),
        Some(DeleteSessionOutcome::Trashed { .. })
    ));
    assert!(!store.vault_base.join("sessions/note").exists());
}
