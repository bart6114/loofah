use super::*;
use serde_json::json;

async fn fixture() -> (tempfile::TempDir, SessionStore) {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().into());
    let meta=serde_json::from_value(json!({"id":"note","title":"Keep title","created_at":"2026-11-08T00:00:00Z","tags":["existing"],"tag_suggestions":{"items":["import/audio","other"],"dismissed":["ignored"]},"future":{"keep":[1,true,"value"]},"skill":"writer"})).unwrap();
    store.create_note_session(&meta, "Keep note").await.unwrap();
    let path = root.path().join("sessions/note/_meta.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["tag_suggestions"]["future"] = json!({"keep":[false,"nested"]});
    std::fs::write(&path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    std::fs::write(
        root.path().join("sessions/note/tasks.json"),
        b"{\"future\":true,\"tasks\":[]}",
    )
    .unwrap();
    std::fs::write(
        root.path().join("sessions/note/foreign.bin"),
        b"Owned by user",
    )
    .unwrap();
    std::fs::write(root.path().join("tags.json"),r#"{"future":{"keep":[true,2]},"tags":[{"id":"existing","name":"existing","future":{"keep":3}}]}"#).unwrap();
    (root, store)
}

async fn baseline(store: &SessionStore) -> hypr_loofah_protocol::MetadataSnapshot {
    store.read_metadata_snapshot("note").await.unwrap().unwrap()
}

#[tokio::test]
async fn checked_ensure_is_lossless_idempotent_and_serializes_concurrent_registration() {
    let (root, store) = fixture().await;
    let metadata = std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap();
    let (first, second) = tokio::join!(
        store.ensure_tag_checked(" #IMPORT/audio "),
        store.ensure_tag_checked("import/audio")
    );
    assert_eq!(first.unwrap(), second.unwrap());
    let path = root.path().join("tags.json");
    let bytes = std::fs::read(&path).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["tags"].as_array().unwrap().len(), 2);
    assert_eq!(value["future"], json!({"keep":[true,2]}));
    assert_eq!(value["tags"][0]["future"], json!({"keep":3}));
    assert_eq!(
        store.ensure_tag_checked("IMPORT/audio").await.unwrap().id,
        "import/audio"
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap(),
        metadata
    );
}

#[tokio::test]
async fn checked_ensure_reuses_original_entry_and_refuses_corrupt_oversize_or_unready_sources() {
    let (root, store) = fixture().await;
    let path = root.path().join("tags.json");
    let original =
        br#"{"future":true,"tags":[{"id":"existing","name":"Original display","future":123}]}"#;
    std::fs::write(&path, original).unwrap();
    assert_eq!(
        store.ensure_tag_checked("EXISTING").await.unwrap().name,
        "Original display"
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    for bytes in [
        b"not json".as_slice(),
        br#"{"tags":[],"future":{"x":1,"x":2}}"#,
    ] {
        std::fs::write(&path, bytes).unwrap();
        assert!(store.ensure_tag_checked("new").await.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::write(&path, original).unwrap();
    let mut bounded = store.clone();
    bounded.content_limits.metadata_bytes = original.len();
    assert!(matches!(
        bounded.ensure_tag_checked("new").await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    store.set_startup_pending(true);
    assert!(store.ensure_tag_checked("new").await.is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
}

#[tokio::test]
async fn checked_accept_preserves_unknown_registry_and_metadata_fields_tasks_and_attachments() {
    let (root, store) = fixture().await;
    let before = baseline(&store).await;
    let tasks = std::fs::read(root.path().join("sessions/note/tasks.json")).unwrap();
    let MetadataSaveOutcome::Saved(saved) = store
        .accept_tag_suggestion_with_recovery(None, "note", " #IMPORT/audio ", &before.revision)
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("not saved")
    };
    assert_eq!(saved.meta.tags, ["existing", "import/audio"]);
    assert_eq!(
        saved.meta.tag_suggestions.as_ref().unwrap().items,
        ["other"]
    );
    assert_eq!(
        saved.meta.tag_suggestions.as_ref().unwrap().dismissed,
        ["ignored"]
    );
    assert_eq!(saved.meta.title, before.meta.title);
    assert_eq!(saved.meta.extra, before.meta.extra);
    assert_eq!(saved.meta.skill, before.meta.skill);
    let metadata: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        metadata["tag_suggestions"]["future"],
        json!({"keep":[false,"nested"]})
    );
    let registry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("tags.json")).unwrap()).unwrap();
    assert_eq!(registry["future"], json!({"keep":[true,2]}));
    assert_eq!(registry["tags"][0]["future"], json!({"keep":3}));
    assert_eq!(
        registry["tags"][1],
        json!({"id":"import/audio","name":"import/audio"})
    );
    assert_eq!(
        std::fs::read(root.path().join("sessions/note/tasks.json")).unwrap(),
        tasks
    );
    assert_eq!(
        std::fs::read(root.path().join("sessions/note/foreign.bin")).unwrap(),
        b"Owned by user"
    );
    assert_eq!(store.session_get("note").unwrap().meta, saved.meta);
    assert_eq!(
        store.tag_suggestions("import", &[], 8).await.unwrap(),
        ["import/audio"]
    );
    let bytes = std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap();
    let MetadataSaveOutcome::Saved(again) = store
        .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &saved.revision)
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("not saved")
    };
    assert_eq!(again, saved);
    assert_eq!(
        std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap(),
        bytes
    );
}

#[tokio::test]
async fn dismiss_changes_only_suggestion_state_without_touching_the_registry() {
    let (root, store) = fixture().await;
    let before = baseline(&store).await;
    let registry = std::fs::read(root.path().join("tags.json")).unwrap();
    let MetadataSaveOutcome::Saved(saved) = store
        .dismiss_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision)
        .await
        .unwrap()
        .unwrap()
    else {
        panic!("not saved")
    };
    assert_eq!(saved.meta.tags, before.meta.tags);
    assert_eq!(saved.meta.extra, before.meta.extra);
    let metadata: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        metadata["tag_suggestions"]["future"],
        json!({"keep":[false,"nested"]})
    );
    assert_eq!(
        saved.meta.tag_suggestions.unwrap().dismissed,
        ["ignored", "import/audio"]
    );
    assert_eq!(
        std::fs::read(root.path().join("tags.json")).unwrap(),
        registry
    );
    assert!(
        store
            .accept_tag_suggestion_with_recovery(None, "missing", "tag", "revision")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn stale_metadata_and_prospective_metadata_or_registry_limits_make_no_writes() {
    let (root, store) = fixture().await;
    let original = baseline(&store).await;
    let path = root.path().join("sessions/note/_meta.json");
    let mut raw = std::fs::read(&path).unwrap();
    raw.push(b'\n');
    std::fs::write(&path, &raw).unwrap();
    let registry = std::fs::read(root.path().join("tags.json")).unwrap();
    assert!(matches!(
        store
            .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &original.revision)
            .await
            .unwrap(),
        Some(MetadataSaveOutcome::Conflict(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), raw);
    assert_eq!(
        std::fs::read(root.path().join("tags.json")).unwrap(),
        registry
    );
    let raw =
        serde_json::to_vec(&serde_json::from_slice::<serde_json::Value>(&raw).unwrap()).unwrap();
    std::fs::write(&path, &raw).unwrap();
    let before = baseline(&store).await;
    let mut bounded = store.clone();
    bounded.content_limits.metadata_bytes = raw.len();
    assert!(matches!(
        bounded
            .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision)
            .await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), raw);
    assert_eq!(
        std::fs::read(root.path().join("tags.json")).unwrap(),
        registry
    );
    let mut registry_value: serde_json::Value = serde_json::from_slice(&registry).unwrap();
    registry_value["padding"] = serde_json::Value::String("x".repeat(2048));
    let registry = serde_json::to_vec(&registry_value).unwrap();
    std::fs::write(root.path().join("tags.json"), &registry).unwrap();
    bounded.content_limits.metadata_bytes = registry.len();
    assert!(matches!(
        bounded
            .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision)
            .await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), raw);
    assert_eq!(
        std::fs::read(root.path().join("tags.json")).unwrap(),
        registry
    );
}

#[tokio::test]
async fn corrupt_and_duplicate_registry_data_is_kept_without_metadata_mutation() {
    let (root, store) = fixture().await;
    let before = baseline(&store).await;
    let raw = std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap();
    for value in [
        "not json",
        r#"{"tags":{}}"#,
        r#"{"tags":[],"tags":[]}"#,
        r#"{"future":{"same":1,"same":2},"tags":[]}"#,
        r#"{"tags":[{"id":"x","id":"y","name":"x"}]}"#,
        r#"{"tags":[{"id":"x","name":"x"},{"id":"x","name":"other"}]}"#,
    ] {
        std::fs::write(root.path().join("tags.json"), value).unwrap();
        assert!(
            store
                .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision)
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(root.path().join("tags.json")).unwrap(),
            value
        );
        assert_eq!(
            std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap(),
            raw
        );
    }
}

#[tokio::test]
async fn concurrent_checked_actions_have_one_winner_and_keep_the_latest_unknown_metadata() {
    let (_root, store) = fixture().await;
    let before = baseline(&store).await;
    let (first, second) = tokio::join!(
        store.accept_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision),
        store.dismiss_tag_suggestion_with_recovery(None, "note", "other", &before.revision)
    );
    let outcomes = [first.unwrap().unwrap(), second.unwrap().unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, MetadataSaveOutcome::Saved(_)))
            .count(),
        1
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|outcome| matches!(outcome, MetadataSaveOutcome::Conflict(_)))
            .count(),
        1
    );
    assert_eq!(baseline(&store).await.meta.extra, before.meta.extra);
}

#[tokio::test]
async fn startup_pending_refuses_checked_actions_without_writing() {
    let (root, store) = fixture().await;
    let before = baseline(&store).await;
    let metadata = std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap();
    let registry = std::fs::read(root.path().join("tags.json")).unwrap();
    store.set_startup_pending(true);
    assert!(matches!(
        store
            .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision)
            .await,
        Err(StoreError::Io(message)) if message.contains("startup")
    ));
    assert!(matches!(
        store
            .dismiss_tag_suggestion_with_recovery(None, "note", "other", &before.revision)
            .await,
        Err(StoreError::Io(message)) if message.contains("startup")
    ));
    assert_eq!(
        std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap(),
        metadata
    );
    assert_eq!(
        std::fs::read(root.path().join("tags.json")).unwrap(),
        registry
    );
}

#[tokio::test]
async fn dropping_a_waiting_caller_does_not_abandon_the_owned_checked_action() {
    use std::{future::Future, task::Poll};
    let (_root, store) = fixture().await;
    let before = baseline(&store).await;
    let lock = store.lock_snapshot().await;
    let mut pending = Box::pin(store.accept_tag_suggestion_with_recovery(
        None,
        "note",
        "import/audio",
        &before.revision,
    ));
    std::future::poll_fn(|cx| {
        assert!(pending.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    drop(pending);
    drop(lock);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if baseline(&store)
                .await
                .meta
                .tags
                .iter()
                .any(|tag| tag == "import/audio")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn prospective_unknown_nested_metadata_bytes_are_budgeted_before_registry_writes() {
    let (root, store) = fixture().await;
    let path = root.path().join("sessions/note/_meta.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["tag_suggestions"]["future"] = json!({"large":"x".repeat(2048)});
    let raw = serde_json::to_vec(&value).unwrap();
    std::fs::write(&path, &raw).unwrap();
    let before = baseline(&store).await;
    let mut bounded = store.clone();
    bounded.content_limits.metadata_bytes = raw.len();
    let registry = std::fs::read(root.path().join("tags.json")).unwrap();
    assert!(matches!(
        bounded
            .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision)
            .await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), raw);
    assert_eq!(
        std::fs::read(root.path().join("tags.json")).unwrap(),
        registry
    );
}

#[tokio::test]
async fn duplicate_unknown_metadata_keys_are_not_normalized_away() {
    let (root, store) = fixture().await;
    let path = root.path().join("sessions/note/_meta.json");
    let raw = std::fs::read_to_string(&path).unwrap().replacen(
        '{',
        "{\"duplicate_future\":1,\"duplicate_future\":2,",
        1,
    );
    std::fs::write(&path, &raw).unwrap();
    let before = baseline(&store).await;
    let registry = std::fs::read(root.path().join("tags.json")).unwrap();
    assert!(
        store
            .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), raw);
    assert_eq!(
        std::fs::read(root.path().join("tags.json")).unwrap(),
        registry
    );
}

#[tokio::test]
async fn checked_actions_finish_admitted_pending_task_work_and_refuse_unknown_recovery() {
    let (root, store) = fixture().await;
    let runtime = tempfile::tempdir().unwrap();
    let before = baseline(&store).await;
    let note = store.read_note_snapshot("note").await.unwrap().unwrap();
    let task:crate::TaskItem=serde_json::from_value(json!({"id":"pending","source_type":"session_raw_note","source_id":"note","source_order":0,"status":"done","text":"Owned pending task","body":[],"due_at":"","assignee":"","created_at":"","updated_at":""})).unwrap();
    let lock = store.lock_snapshot().await;
    let pending = store
        .begin_note_tasks_locked(
            &lock._guard,
            runtime.path(),
            "note",
            &note,
            std::slice::from_ref(&task),
        )
        .await
        .unwrap();
    let intent = pending.path().to_owned();
    drop(pending);
    drop(lock);
    assert!(intent.exists());
    assert!(matches!(
        store
            .accept_tag_suggestion_with_recovery(
                Some(runtime.path()),
                "note",
                "import/audio",
                &before.revision
            )
            .await
            .unwrap(),
        Some(MetadataSaveOutcome::Saved(_))
    ));
    assert!(!intent.exists());
    assert_eq!(
        store.session_get("note").unwrap().meta.tags,
        ["existing", "import/audio"]
    );
    assert_eq!(store.session_tasks("note").await.unwrap(), [task]);
    let before = baseline(&store).await;
    let raw = std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap();
    let registry = std::fs::read(root.path().join("tags.json")).unwrap();
    std::fs::write(&intent, b"{\"schema_version\":999}").unwrap();
    assert!(
        store
            .dismiss_tag_suggestion_with_recovery(
                Some(runtime.path()),
                "note",
                "other",
                &before.revision
            )
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&intent).unwrap(), b"{\"schema_version\":999}");
    assert_eq!(
        std::fs::read(root.path().join("sessions/note/_meta.json")).unwrap(),
        raw
    );
    assert_eq!(
        std::fs::read(root.path().join("tags.json")).unwrap(),
        registry
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_metadata_io_failure_after_registry_add_returns_error_and_keeps_original_metadata() {
    use std::os::unix::fs::PermissionsExt;
    let (root, store) = fixture().await;
    let before = baseline(&store).await;
    let directory = root.path().join("sessions/note");
    let raw = std::fs::read(directory.join("_meta.json")).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result = store
        .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision)
        .await;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(result.is_err());
    assert_eq!(std::fs::read(directory.join("_meta.json")).unwrap(), raw);
    let registry: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.path().join("tags.json")).unwrap()).unwrap();
    assert_eq!(registry["tags"][1]["id"], "import/audio");
    assert_eq!(registry["future"], json!({"keep":[true,2]}));
    assert!(matches!(
        store
            .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision)
            .await
            .unwrap(),
        Some(MetadataSaveOutcome::Saved(_))
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn unsafe_registry_symlinks_fail_closed() {
    use std::os::unix::fs::symlink;
    let (root, store) = fixture().await;
    let before = baseline(&store).await;
    let path = root.path().join("tags.json");
    let held = root.path().join("kept.json");
    std::fs::rename(&path, &held).unwrap();
    symlink(&held, &path).unwrap();
    let bytes = std::fs::read(&held).unwrap();
    assert!(
        store
            .accept_tag_suggestion_with_recovery(None, "note", "import/audio", &before.revision)
            .await
            .is_err()
    );
    assert_eq!(std::fs::read(&held).unwrap(), bytes);
    assert_eq!(baseline(&store).await, before);
}
