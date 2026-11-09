use std::{sync::Arc, time::Duration};

use super::*;

async fn fixture() -> (tempfile::TempDir, SessionStore) {
    let root = tempfile::tempdir().unwrap();
    let store = SessionStore::new(root.path().into());
    let meta = serde_json::from_value(serde_json::json!({
        "id":"note", "title":"Original", "created_at":"2026-11-07T10:00:00Z", "tags":[]
    }))
    .unwrap();
    store.create_note_session(&meta, "Original").await.unwrap();
    (root, store)
}

#[tokio::test]
async fn source_reads_canonical_files_and_shared_tag_rules_without_refreshing_the_index() {
    let (root, store) = fixture().await;
    let directory = root.path().join("sessions/note");
    let metadata = serde_json::json!({"id":"note","title":"Canonical title","created_at":"2026-11-07T10:00:00Z","tags":["#Plan","alpha"],"tag_suggestions":{"items":[],"dismissed":["RETIRED"]},"future_field":{"preserve":true}});
    std::fs::write(directory.join("_meta.json"), metadata.to_string()).unwrap();
    std::fs::write(
        directory.join("notes.md"),
        "---\nid: note:note\nposition: 0\n---\nCanonical note",
    )
    .unwrap();
    std::fs::write(directory.join("_memo.md"), "Ignored older note").unwrap();
    std::fs::write(directory.join("unknown-user.txt"), "Do not enumerate this").unwrap();
    std::fs::write(directory.join(".private"), "Do not enumerate this either").unwrap();
    std::fs::write(
        root.path().join("people.json"),
        r#"{"people":[{"id":"bob","name":"Bob"},{"id":"alice","name":"Alice"}]}"#,
    )
    .unwrap();
    std::fs::write(root.path().join("tags.json"), r#"{"tags":[{"id":"release","name":"Release"},{"id":"invalid","name":"bad tag"},{"id":"import","name":"import-audio"}]}"#).unwrap();
    std::fs::write(directory.join("transcript.json"), serde_json::json!({"transcripts":[
        {"id":"late-b","session_id":"note","started_at":20,"created_at":"b","memo_md":"later","words":[]},
        {"id":"early","session_id":"note","started_at":10,"created_at":"z","memo_md":"first memo","words":[{"text":"Ship Friday","start_ms":0,"end_ms":10,"channel":0}]},
        {"id":"late-a","session_id":"note","started_at":20,"created_at":"a","words":[]}
    ]}).to_string()).unwrap();
    let paths = [
        directory.join("_meta.json"),
        directory.join("notes.md"),
        directory.join("_memo.md"),
        directory.join("unknown-user.txt"),
        directory.join(".private"),
        directory.join("transcript.json"),
        root.path().join("people.json"),
        root.path().join("tags.json"),
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    let source = store
        .read_processing_source(None, "note", SourceReadLimits::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.meta.title, "Canonical title");
    assert_eq!(source.note_markdown.as_deref(), Some("Canonical note"));
    assert_eq!(
        source.meta.extra["future_field"],
        serde_json::json!({"preserve":true})
    );
    assert_eq!(
        source
            .transcripts
            .iter()
            .map(|transcript| transcript.id.as_str())
            .collect::<Vec<_>>(),
        ["early", "late-a", "late-b"]
    );
    assert_eq!(source.transcripts[0].words[0].text, "Ship Friday");
    assert_eq!(source.people, store.list_people().await.unwrap());
    assert_eq!(
        source.tag_context,
        store.session_tag_context("note").await.unwrap()
    );
    assert_eq!(source.tag_context.available, ["alpha", "plan", "release"]);
    assert_eq!(source.tag_context.dismissed, ["retired"]);
    for (path, bytes) in paths.iter().zip(before) {
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}

#[tokio::test]
async fn missing_optional_files_and_legacy_notes_keep_their_existing_read_semantics() {
    let (root, store) = fixture().await;
    let directory = root.path().join("sessions/note");
    std::fs::rename(directory.join("notes.md"), directory.join("_memo.md")).unwrap();
    let source = store
        .read_processing_source(None, "note", SourceReadLimits::default())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.note_markdown.as_deref(), Some("Original"));
    assert!(source.transcripts.is_empty());
    assert!(source.people.is_empty());
    std::fs::write(root.path().join("people.json"), "{broken").unwrap();
    std::fs::write(root.path().join("tags.json"), "{broken").unwrap();
    assert!(
        store
            .read_processing_source(None, "note", SourceReadLimits::default())
            .await
            .unwrap()
            .unwrap()
            .people
            .is_empty()
    );
    assert!(
        store
            .read_processing_source(None, "missing", SourceReadLimits::default())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .read_processing_source(None, "../note", SourceReadLimits::default())
            .await
            .is_err()
    );
    store.set_startup_pending(true);
    assert!(
        store
            .read_processing_source(None, "note", SourceReadLimits::default())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn transcript_corruption_and_size_limits_fail_without_truncating_or_repairing_content() {
    let (root, store) = fixture().await;
    let path = root.path().join("sessions/note/transcript.json");
    std::fs::write(&path, "{broken").unwrap();
    assert!(matches!(
        store
            .read_processing_source(None, "note", SourceReadLimits::default())
            .await,
        Err(StoreError::Serialize(_))
    ));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
    std::fs::write(&path, r#"{"transcripts":[]}"#).unwrap();
    let limits = SourceReadLimits {
        transcript_bytes: 4,
        ..Default::default()
    };
    assert!(matches!(
        store.read_processing_source(None, "note", limits).await,
        Err(StoreError::ResourceLimit(_))
    ));
    let limits = SourceReadLimits {
        metadata_bytes: 4,
        ..Default::default()
    };
    assert!(matches!(
        store.read_processing_source(None, "note", limits).await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert_eq!(
        store
            .read_note_snapshot("note")
            .await
            .unwrap()
            .unwrap()
            .markdown
            .as_deref(),
        Some("Original")
    );
    let limits = SourceReadLimits {
        note_bytes: 4,
        ..Default::default()
    };
    assert!(matches!(
        store.read_processing_source(None, "note", limits).await,
        Err(StoreError::ResourceLimit(_))
    ));
    std::fs::write(root.path().join("people.json"), r#"{"people":[]}"#).unwrap();
    let limits = SourceReadLimits {
        registry_bytes: 4,
        ..Default::default()
    };
    assert!(matches!(
        store.read_processing_source(None, "note", limits).await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        r#"{"transcripts":[]}"#
    );
}

#[cfg(unix)]
#[tokio::test]
async fn transcript_and_registry_symlinks_cannot_read_another_vault() {
    for relative in ["sessions/note/transcript.json", "people.json", "tags.json"] {
        let (root, store) = fixture().await;
        let outside = tempfile::tempdir().unwrap();
        let target = outside.path().join("private.json");
        std::fs::write(&target, r#"{"transcripts":[],"people":[],"tags":[]}"#).unwrap();
        std::os::unix::fs::symlink(&target, root.path().join(relative)).unwrap();
        assert!(
            store
                .read_processing_source(None, "note", SourceReadLimits::default())
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            r#"{"transcripts":[],"people":[],"tags":[]}"#
        );
    }
}

#[tokio::test]
async fn snapshots_exclude_concurrent_compound_writes_and_keep_ownership_after_caller_cancellation()
{
    for cancel in [false, true] {
        let (_root, store) = fixture().await;
        let entered = Arc::new(tokio::sync::Notify::new());
        let resume = Arc::new(tokio::sync::Notify::new());
        let (signal, wait) = (entered.clone(), resume.clone());
        let reader_store = store.clone();
        let reader = tokio::spawn(async move {
            reader_store
                .read_processing_source_with(
                    None,
                    "note",
                    SourceReadLimits::default(),
                    move || async move {
                        signal.notify_one();
                        wait.notified().await;
                    },
                )
                .await
        });
        entered.notified().await;
        if cancel {
            reader.abort();
        }
        let writer_store = store.clone();
        let started = Arc::new(tokio::sync::Notify::new());
        let signal = started.clone();
        let writer = tokio::spawn(async move {
            signal.notify_one();
            writer_store
                .save_note("note", "Replacement", Some("Replacement"))
                .await
        });
        started.notified().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(40), store.lock_snapshot())
                .await
                .is_err()
        );
        assert!(!writer.is_finished());
        resume.notify_one();
        if !cancel {
            let source = reader.await.unwrap().unwrap().unwrap();
            assert_eq!(source.meta.title, "Original");
            assert_eq!(source.note_markdown.as_deref(), Some("Original"));
        } else {
            assert!(reader.await.unwrap_err().is_cancelled());
        }
        writer.await.unwrap().unwrap();
        let source = store
            .read_processing_source(None, "note", SourceReadLimits::default())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(source.meta.title, "Replacement");
        assert_eq!(source.note_markdown.as_deref(), Some("Replacement"));
    }
}
