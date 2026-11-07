use super::*;
use sha2::{Digest, Sha256};
use std::{io::Write, time::Duration};

fn input(bytes: &[u8]) -> UploadAttachmentRequest {
    UploadAttachmentRequest {
        filename: "file.pdf".into(),
        size_bytes: bytes.len() as u64,
        sha256: Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    }
}
async fn fixture() -> (tempfile::TempDir, tempfile::TempDir, SessionStore) {
    let vault = tempfile::tempdir().unwrap();
    let operations = tempfile::tempdir().unwrap();
    let store = SessionStore::new(vault.path().into());
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Upload","created_at":"2026-11-06T10:00:00Z","tags":[]})).unwrap();
    store
        .create_note_session(&meta, "Canonical note")
        .await
        .unwrap();
    std::fs::write(
        vault.path().join("sessions/note/unknown-user.pdf"),
        b"Unknown user file",
    )
    .unwrap();
    (vault, operations, store)
}
async fn plan(
    store: &SessionStore,
    operations: &Path,
    id: uuid::Uuid,
    bytes: &[u8],
) -> AttachmentPublicationPlan {
    let request = input(bytes);
    let bytes = bytes.to_vec();
    store
        .stage_attachment("note", &request.filename, move |file| {
            file.write_all(&bytes)
        })
        .await
        .unwrap()
        .unwrap()
        .prepare_publication(operations, "alice", id, request)
        .await
        .unwrap()
}
fn receipt_path(root: &Path, id: uuid::Uuid) -> PathBuf {
    root.join(format!("{}.json", operation_key("alice", id)))
}

#[tokio::test]
async fn publication_is_deduplicated_and_replays_never_recreate_a_deleted_file_or_overwrite_restored_bytes()
 {
    let (vault, operations, store) = fixture().await;
    store
        .save_attachment("note", "file.pdf", b"Existing file".to_vec())
        .await
        .unwrap();
    let id = uuid::Uuid::new_v4();
    assert_eq!(
        plan(&store, operations.path(), id, b"Original upload")
            .await
            .commit()
            .await
            .unwrap(),
        AttachmentPublication::Published("file 1.pdf".into())
    );
    let target = vault.path().join("sessions/note/attachments/file 1.pdf");
    assert_eq!(std::fs::read(&target).unwrap(), b"Original upload");
    std::fs::remove_file(&target).unwrap();
    assert_eq!(
        plan(&store, operations.path(), id, b"Original upload")
            .await
            .commit()
            .await
            .unwrap(),
        AttachmentPublication::Published("file 1.pdf".into())
    );
    assert!(!target.exists());
    std::fs::write(&target, b"User restored and edited").unwrap();
    let status = store
        .prepare_attachment_upload_status(
            operations.path(),
            "alice",
            id,
            "note",
            input(b"Original upload"),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        status.commit().await.unwrap(),
        AttachmentPublication::Published("file 1.pdf".into())
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"User restored and edited");
    assert_eq!(
        std::fs::read_dir(target.parent().unwrap()).unwrap().count(),
        2
    );
    assert_eq!(
        std::fs::read(vault.path().join("sessions/note/notes.md")).unwrap(),
        b"Canonical note"
    );
    assert_eq!(
        std::fs::read(vault.path().join("sessions/note/unknown-user.pdf")).unwrap(),
        b"Unknown user file"
    );
}

#[tokio::test]
async fn length_hash_and_request_identity_mismatches_cannot_publish_bytes() {
    let (vault, operations, store) = fixture().await;
    let id = uuid::Uuid::new_v4();
    for declared in [input(b"longer contents"), input(b"changed")] {
        let stage = store
            .stage_attachment("note", "file.pdf", |file| file.write_all(b"payload"))
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(
            stage
                .prepare_publication(operations.path(), "alice", id, declared)
                .await,
            Err(StoreError::Conflict(_))
        ));
    }
    assert!(
        std::fs::read_dir(vault.path().join("sessions/note/attachments"))
            .unwrap()
            .next()
            .is_none()
    );
    plan(&store, operations.path(), id, b"payload")
        .await
        .commit()
        .await
        .unwrap();
    assert!(matches!(
        store
            .prepare_attachment_upload_status(
                operations.path(),
                "alice",
                id,
                "note",
                input(b"changed")
            )
            .await,
        Err(StoreError::Conflict(_))
    ));
    assert!(
        store
            .prepare_attachment_upload_status(
                operations.path(),
                "bob",
                id,
                "note",
                input(b"payload")
            )
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        std::fs::read(vault.path().join("sessions/note/attachments/file.pdf")).unwrap(),
        b"payload"
    );
    assert!(store.attachment_stages.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_status_read_can_confirm_an_interrupted_commit_after_restart_without_uploading_the_body_again()
 {
    let (vault, operations, store) = fixture().await;
    let id = uuid::Uuid::new_v4();
    let result = plan(&store, operations.path(), id, b"payload")
        .await
        .commit_with(|| Err(StoreError::Io("lost before final receipt".into())))
        .await;
    assert!(result.is_err());
    assert!(matches!(
        read_receipt::<Receipt>(&receipt_path(operations.path(), id))
            .unwrap()
            .unwrap()
            .state,
        State::Pending
    ));
    let restarted = SessionStore::new(vault.path().into());
    let status = restarted
        .prepare_attachment_upload_status(operations.path(), "alice", id, "note", input(b"payload"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        status.commit().await.unwrap(),
        AttachmentPublication::Published("file.pdf".into())
    );
    assert!(matches!(
        read_receipt::<Receipt>(&receipt_path(operations.path(), id))
            .unwrap()
            .unwrap()
            .state,
        State::Committed
    ));
    assert_eq!(
        std::fs::read_dir(vault.path().join("sessions/note/attachments"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn pending_intents_never_resume_a_publish_and_changed_recovery_targets_remain_untouched() {
    let (vault, operations, store) = fixture().await;
    let id = uuid::Uuid::new_v4();
    let request = Request {
        session: "note".into(),
        input: input(b"payload"),
    };
    persist(
        &receipt_path(operations.path(), id),
        &Receipt {
            schema_version: 1,
            request,
            filename: "file.pdf".into(),
            state: State::Pending,
        },
    )
    .unwrap();
    assert_eq!(
        plan(&store, operations.path(), id, b"payload")
            .await
            .commit()
            .await
            .unwrap(),
        AttachmentPublication::Uncertain
    );
    assert!(
        !vault
            .path()
            .join("sessions/note/attachments/file.pdf")
            .exists()
    );
    let id = uuid::Uuid::new_v4();
    assert!(
        plan(&store, operations.path(), id, b"payload")
            .await
            .commit_with(|| Err(StoreError::Io("lost reply".into())))
            .await
            .is_err()
    );
    std::fs::write(
        vault.path().join("sessions/note/attachments/file.pdf"),
        b"User changed",
    )
    .unwrap();
    let status = store
        .prepare_attachment_upload_status(operations.path(), "alice", id, "note", input(b"payload"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        status.commit().await.unwrap(),
        AttachmentPublication::Uncertain
    );
    assert_eq!(
        std::fs::read(vault.path().join("sessions/note/attachments/file.pdf")).unwrap(),
        b"User changed"
    );
}

#[tokio::test]
async fn changed_staging_generations_are_rejected_and_future_receipts_are_preserved() {
    let (vault, operations, store) = fixture().await;
    let id = uuid::Uuid::new_v4();
    let prepared = plan(&store, operations.path(), id, b"payload").await;
    let stage = prepared.stage.as_ref().unwrap().path().to_owned();
    std::fs::rename(&stage, stage.with_extension("retained")).unwrap();
    std::fs::write(&stage, b"Changed private stage").unwrap();
    assert_eq!(
        prepared.commit().await.unwrap(),
        AttachmentPublication::Changed
    );
    assert!(
        !vault
            .path()
            .join("sessions/note/attachments/file.pdf")
            .exists()
    );
    let id = uuid::Uuid::new_v4();
    persist(
        &receipt_path(operations.path(), id),
        &Receipt {
            schema_version: 999,
            request: Request {
                session: "note".into(),
                input: input(b"payload"),
            },
            filename: "../../private".into(),
            state: State::Pending,
        },
    )
    .unwrap();
    let original = std::fs::read(receipt_path(operations.path(), id)).unwrap();
    assert_eq!(
        plan(&store, operations.path(), id, b"payload")
            .await
            .commit()
            .await
            .unwrap(),
        AttachmentPublication::Uncertain
    );
    assert_eq!(
        std::fs::read(receipt_path(operations.path(), id)).unwrap(),
        original
    );
}

#[tokio::test]
async fn completed_receipts_can_be_read_after_session_deletion_without_recreating_the_session() {
    let (vault, operations, store) = fixture().await;
    let id = uuid::Uuid::new_v4();
    plan(&store, operations.path(), id, b"payload")
        .await
        .commit()
        .await
        .unwrap();
    store.delete_session("note").await.unwrap();
    let status = store
        .prepare_attachment_upload_status(operations.path(), "alice", id, "note", input(b"payload"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        status.commit().await.unwrap(),
        AttachmentPublication::Published("file.pdf".into())
    );
    assert!(!vault.path().join("sessions/note").exists());
}

#[tokio::test]
async fn abandoned_commits_hold_publication_and_write_guards_until_the_receipt_is_durable() {
    let (_vault, operations, store) = fixture().await;
    let id = uuid::Uuid::new_v4();
    let prepared = plan(&store, operations.path(), id, b"payload").await;
    let (entered, started) = tokio::sync::oneshot::channel();
    let (release, paused) = std::sync::mpsc::channel();
    let task = tokio::spawn(async move {
        prepared
            .commit_with(move || {
                let _ = entered.send(());
                paused.recv().unwrap();
                Ok(())
            })
            .await
    });
    started.await.unwrap();
    task.abort();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), store.lock_snapshot())
            .await
            .is_err()
    );
    assert!(
        store
            .attachment_publications
            .clone()
            .try_lock_owned()
            .is_err()
    );
    assert!(store.attachment_stages.lock().unwrap().contains_key("note"));
    release.send(()).unwrap();
    let status = tokio::time::timeout(
        Duration::from_secs(2),
        store.prepare_attachment_upload_status(
            operations.path(),
            "alice",
            id,
            "note",
            input(b"payload"),
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(
        status.commit().await.unwrap(),
        AttachmentPublication::Published("file.pdf".into())
    );
    assert!(store.attachment_stages.lock().unwrap().is_empty());
}
