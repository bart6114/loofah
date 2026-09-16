use std::path::Path;

use crate::{SessionMeta, SessionMetaPatch, SessionStore};

const ID: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
const CANONICAL_DIR: &str = "sessions/6ba7b810-9dad-11d1-80b4-00c04fd430c8";
const LEGACY_ID: &str = "550e8400-e29b-41d4-a716-446655440000";

fn meta(id: &str, title: &str) -> SessionMeta {
    SessionMeta {
        id: id.to_string(),
        title: title.to_string(),
        started_at: None,
        ended_at: None,
        created_at: "2026-07-24T00:00:00Z".to_string(),
        tags: vec![],
        tag_suggestions: None,
        tracking_id: None,
        folder: None,
        author: None,
        skill: None,
        extra: Default::default(),
    }
}

fn seed_session_at(vault: &Path, relative_dir: &str, id: &str, title: &str) {
    let dir = vault.join(relative_dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("_meta.json"),
        serde_json::to_vec_pretty(&meta(id, title)).unwrap(),
    )
    .unwrap();
}

async fn test_store() -> (SessionStore, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let store = SessionStore::new(temp.path().to_path_buf());
    (store, temp)
}

fn transcript(id: &str, word_text: &str) -> hypr_fs_format::TranscriptWithData {
    hypr_fs_format::TranscriptWithData {
        id: id.to_string(),
        user_id: String::new(),
        created_at: "2026-07-24T00:00:00Z".to_string(),
        session_id: ID.to_string(),
        started_at: 0.0,
        ended_at: None,
        memo_md: String::new(),
        words: vec![hypr_fs_format::TranscriptWord {
            id: Some("w0".to_string()),
            text: word_text.to_string(),
            start_ms: 0.0,
            end_ms: 0.0,
            channel: 0.0,
            speaker: None,
            metadata: None,
        }],
        speaker_hints: vec![],
    }
}

#[tokio::test]
async fn two_same_day_delete_restore_cycles_round_trip() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), CANONICAL_DIR, ID, "Readable");
    store.write_note(ID, "first").await.unwrap();

    store.delete_session(ID).await.unwrap();
    assert!(store.restore_session(ID).await.unwrap());
    store.write_note(ID, "second").await.unwrap();
    store.delete_session(ID).await.unwrap();
    assert!(store.restore_session(ID).await.unwrap());

    assert_eq!(
        std::fs::read_to_string(vault.path().join(CANONICAL_DIR).join("notes.md")).unwrap(),
        "second",
        "each cycle must restore the exact directory trashed by the latest delete"
    );
}

#[tokio::test]
async fn restore_fails_safely_when_the_destination_is_occupied() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), CANONICAL_DIR, ID, "Readable");
    store.delete_session(ID).await.unwrap();

    // Something (a sync client, the user) recreated the destination in the meantime.
    std::fs::create_dir_all(vault.path().join(CANONICAL_DIR)).unwrap();
    std::fs::write(
        vault.path().join(CANONICAL_DIR).join("other.md"),
        "not ours",
    )
    .unwrap();

    assert!(store.restore_session(ID).await.is_err());
    assert_eq!(
        std::fs::read_to_string(vault.path().join(CANONICAL_DIR).join("other.md")).unwrap(),
        "not ours",
        "a failed restore must never merge onto or replace the occupied destination"
    );
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(
        vault
            .path()
            .join(".trash")
            .join(&date)
            .join(CANONICAL_DIR)
            .join("_meta.json")
            .is_file(),
        "the trash entry must stay for manual recovery"
    );
}

#[tokio::test]
async fn restore_rejects_a_tampered_trash_entry_and_expires_a_vanished_one() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), CANONICAL_DIR, ID, "Readable");
    store.delete_session(ID).await.unwrap();

    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let trashed = vault.path().join(".trash").join(&date).join(CANONICAL_DIR);
    std::fs::write(
        trashed.join("_meta.json"),
        serde_json::to_vec_pretty(&meta("someone-else", "Impostor")).unwrap(),
    )
    .unwrap();
    assert!(
        store.restore_session(ID).await.is_err(),
        "a trash entry claiming a different id must not be restored"
    );

    let (store2, vault2) = test_store().await;
    seed_session_at(vault2.path(), CANONICAL_DIR, ID, "Readable");
    store2.delete_session(ID).await.unwrap();
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    std::fs::remove_dir_all(vault2.path().join(".trash").join(&date)).unwrap();
    assert!(
        !store2.restore_session(ID).await.unwrap(),
        "a vanished trash entry is an expired undo, not an error"
    );
}

/// The undo toast is process-local: a fresh store (an app restart) has no
/// recent-deletion record, so restore reports "nothing to restore" while the trashed
/// directory stays on disk for manual recovery.
#[tokio::test]
async fn restore_returns_false_after_a_restart() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), CANONICAL_DIR, ID, "Readable");
    store.delete_session(ID).await.unwrap();

    let cold = SessionStore::new(vault.path().to_path_buf());
    assert!(!cold.restore_session(ID).await.unwrap());
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(
        vault
            .path()
            .join(".trash")
            .join(&date)
            .join(CANONICAL_DIR)
            .is_dir(),
        "the trashed directory must remain for manual recovery"
    );
}

#[tokio::test]
async fn migration_preserves_every_byte_and_rebuild_observes_later_metadata() {
    let (store, vault) = test_store().await;
    for (source, id, title) in [
        ("2026-09-15 — Planning — abcdef", ID, "Planning"),
        ("Custom name", LEGACY_ID, ""),
    ] {
        let dir = vault.path().join("sessions").join(source);
        let mut metadata = meta(id, title);
        metadata.folder = Some("retain this metadata".into());
        metadata
            .extra
            .insert("future".into(), serde_json::json!({"keep": true}));
        std::fs::create_dir_all(dir.join("attachments")).unwrap();
        std::fs::create_dir_all(dir.join("enhanced")).unwrap();
        std::fs::write(
            dir.join("_meta.json"),
            serde_json::to_vec_pretty(&metadata).unwrap(),
        )
        .unwrap();
        for (file, bytes) in [
            ("notes.md", b"![image](attachments/image.png)".as_slice()),
            ("_memo.md", b"legacy note"),
            ("audio.mp3", &[0, 1, 255, 42]),
            ("transcript.json", b"{\"transcripts\":[]}"),
            ("tasks.json", b"{\"tasks\":[]}"),
            ("enhanced/doc-1.md", b"---\nkind: summary\n---\nRecap"),
            ("attachments/image.png", &[137, 80, 78, 71]),
            ("unknown.bin", &[0, 2, 0, 255]),
            (".hidden", b"untouched"),
        ] {
            std::fs::write(dir.join(file), bytes).unwrap();
        }
    }
    let before = snapshot(
        vault
            .path()
            .join("sessions/2026-09-15 — Planning — abcdef")
            .as_path(),
    );
    let startup = store.normalize_startup_layout().await.unwrap();
    assert_eq!(startup.migration.renamed.len(), 2);
    assert!(startup.migration.failed.is_empty());
    assert_eq!(snapshot(&vault.path().join(CANONICAL_DIR)), before);
    // Metadata discovered before acquiring a session's read transaction may be stale.
    let mut changed = meta(ID, "changed after scan");
    changed.folder = Some("retain this metadata".into());
    std::fs::write(
        vault.path().join(CANONICAL_DIR).join("_meta.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    store
        .rebuild_index_from_startup_layout(startup)
        .await
        .unwrap();
    assert_eq!(
        store.session_get(ID).unwrap().meta.title,
        "changed after scan"
    );
    assert_eq!(
        store.session_get(ID).unwrap().meta.folder.as_deref(),
        Some("retain this metadata")
    );
    store.refresh_session(ID).await.unwrap();
    assert_eq!(
        store.session_get(ID).unwrap().meta.title,
        "changed after scan"
    );
    assert!(
        store
            .normalize_startup_layout()
            .await
            .unwrap()
            .migration
            .renamed
            .is_empty()
    );
}

fn snapshot(dir: &Path) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn collect(
        base: &Path,
        dir: &Path,
        result: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    ) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(base, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(base).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut result = Default::default();
    collect(dir, dir, &mut result);
    result
}

#[tokio::test]
async fn migration_conflicts_do_not_hide_the_canonical_copy_or_move_sources() {
    let (store, vault) = test_store().await;
    for (path, id) in [
        (CANONICAL_DIR, ID),
        ("sessions/Readable copy", ID),
        ("sessions/one", "duplicate"),
        ("sessions/two", "duplicate"),
        ("sessions/unsafe", "../escape"),
        ("sessions/healthy", "healthy-id"),
    ] {
        seed_session_at(vault.path(), path, id, path);
    }
    seed_session_at(
        vault.path(),
        &format!("sessions/{LEGACY_ID}"),
        "wrong-identity",
        "mismatch",
    );
    let layout = store.normalize_startup_layout().await.unwrap();
    assert_eq!(layout.migration.renamed.len(), 1);
    assert_eq!(layout.migration.skipped.len(), 5);
    store
        .rebuild_index_from_startup_layout(layout)
        .await
        .unwrap();
    assert!(store.session_get(ID).is_some());
    assert!(store.session_get("healthy-id").is_some());
    assert!(store.session_get("duplicate").is_none());
    assert!(store.read_meta(LEGACY_ID).await.is_err());
    assert!(store.read_meta("wrong-identity").await.unwrap().is_none());
    for source in ["Readable copy", "one", "two", "unsafe"] {
        assert!(
            vault
                .path()
                .join("sessions")
                .join(source)
                .join("_meta.json")
                .is_file()
        );
    }
    assert!(store.read_meta("duplicate").await.unwrap().is_none());
    assert!(
        hypr_vault_read::meta::read_note(vault.path(), "duplicate")
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn migration_ignores_nested_hidden_symlinked_and_metadata_free_directories() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), "sessions/Work/nested", ID, "Nested");
    seed_session_at(vault.path(), "sessions/.hidden", LEGACY_ID, "Hidden");
    seed_session_at(vault.path(), ".trash/old", "trashed", "Trashed");
    std::fs::create_dir_all(vault.path().join("sessions/no-meta")).unwrap();
    std::fs::write(
        vault.path().join("sessions/no-meta/unknown.md"),
        "user attachment",
    )
    .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        vault.path().join("sessions/.hidden"),
        vault.path().join("sessions/link"),
    )
    .unwrap();
    let layout = store.normalize_startup_layout().await.unwrap();
    assert!(layout.migration.renamed.is_empty());
    assert_eq!(layout.session_count(), 0);
    assert!(
        vault
            .path()
            .join("sessions/Work/nested/_meta.json")
            .is_file()
    );
}

#[tokio::test]
async fn a_delayed_end_stamp_keeps_the_next_captures_reservation() {
    let (store, _vault) = test_store().await;
    store.create_session_meta(&meta(ID, "")).await.unwrap();
    store.prepare_recording(ID).await.unwrap();
    store
        .mark_recording_started(ID, "2026-09-15T10:00:00Z")
        .await
        .unwrap();

    store.prepare_recording(ID).await.unwrap();
    store
        .mark_recording_ended(ID, "2026-09-15T10:01:00Z")
        .await
        .unwrap();
    assert!(store.is_recording(ID));
    assert!(store.freeze_for_vault_move().await.is_err());

    store
        .mark_recording_started(ID, "2026-09-15T10:02:00Z")
        .await
        .unwrap();
    store
        .mark_recording_ended(ID, "2026-09-15T10:03:00Z")
        .await
        .unwrap();
    assert!(!store.is_recording(ID));
    assert!(store.freeze_for_vault_move().await.is_ok());
}

#[tokio::test]
async fn title_date_and_recording_lifecycle_never_move_the_directory() {
    let (store, vault) = test_store().await;
    store.create_session_meta(&meta(ID, "")).await.unwrap();
    let dir = store.prepare_recording(ID).await.unwrap();
    assert_eq!(dir, std::path::PathBuf::from(CANONICAL_DIR));
    store
        .mark_recording_started(ID, "2026-09-15T10:00:00Z")
        .await
        .unwrap();
    store
        .update_meta(
            ID,
            SessionMetaPatch {
                title: Some("Planning".into()),
                created_at: Some("2020-01-01T00:00:00Z".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    store.write_note(ID, "notes during capture").await.unwrap();
    store.prepare_recording(ID).await.unwrap();
    store.release_recording_prepare(ID).await.unwrap();
    assert!(store.freeze_for_vault_move().await.is_err());
    assert_eq!(store.session_dir(ID).await.unwrap(), dir);
    store
        .mark_recording_ended(ID, "2026-09-15T10:10:00Z")
        .await
        .unwrap();
    assert!(store.freeze_for_vault_move().await.is_ok());
    assert_eq!(store.session_dir(ID).await.unwrap(), dir);
    assert_eq!(
        std::fs::read_dir(vault.path().join("sessions"))
            .unwrap()
            .count(),
        1
    );
}

#[tokio::test]
async fn deleted_sessions_reject_delayed_artifact_writes_and_restore_exact_trash_path() {
    let (store, vault) = test_store().await;
    store.write_meta(&meta(ID, "keep")).await.unwrap();
    store.write_note(ID, "keep these bytes").await.unwrap();
    // Occupy the unsuffixed trash path, forcing deletion to return a collision suffix.
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    std::fs::create_dir_all(vault.path().join(".trash").join(date).join(CANONICAL_DIR)).unwrap();
    store.delete_session(ID).await.unwrap();
    for artifact in [
        "notes.md",
        "transcript.json",
        "tasks.json",
        "enhanced/doc.md",
    ] {
        assert!(
            store
                .write_file(
                    std::path::PathBuf::from(CANONICAL_DIR).join(artifact),
                    b"late".to_vec()
                )
                .await
                .is_err()
        );
    }
    assert!(
        store
            .write_transcript(ID, transcript("late", "must not return"))
            .await
            .is_err()
    );
    assert!(!vault.path().join(CANONICAL_DIR).exists());
    assert!(store.restore_session(ID).await.unwrap());
    assert_eq!(
        store.read_note(ID).await.unwrap().as_deref(),
        Some("keep these bytes")
    );
}

#[tokio::test]
async fn normal_rebuild_never_migrates_and_permission_failures_retry_next_startup() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), "sessions/Readable", ID, "keep");
    store.rebuild_index().await.unwrap();
    assert!(store.session_get(ID).is_none());
    assert!(vault.path().join("sessions/Readable").is_dir());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = vault.path().join("sessions/Readable/_meta.json");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
        let layout = store.normalize_startup_layout().await.unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(layout.session_count(), 0);
        assert_eq!(layout.migration.skipped.len(), 1);
    }
    let retry = store.normalize_startup_layout().await.unwrap();
    assert_eq!(retry.migration.renamed.len(), 1);
    assert_eq!(retry.session_count(), 1);
}

#[tokio::test]
async fn startup_snapshot_does_not_resurrect_a_deleted_session_or_prune_a_new_one() {
    let (store, vault) = test_store().await;
    store.write_meta(&meta(ID, "original")).await.unwrap();
    let layout = store.normalize_startup_layout().await.unwrap();
    store.delete_session(ID).await.unwrap();
    store
        .write_meta(&meta("new-id", "created after scan"))
        .await
        .unwrap();
    store
        .rebuild_index_from_startup_layout(layout)
        .await
        .unwrap();
    assert!(store.session_get(ID).is_none());
    assert!(!vault.path().join(CANONICAL_DIR).exists());
    assert!(store.write_note(ID, "late").await.is_err());
    assert!(store.session_get("new-id").is_some());
}

#[cfg(unix)]
#[tokio::test]
async fn failed_rename_keeps_the_source_and_is_retried_on_next_startup() {
    use std::os::unix::fs::PermissionsExt;
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), "sessions/Readable", ID, "safe");
    let root = vault.path().join("sessions");
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o500)).unwrap();
    let failed = store.normalize_startup_layout().await.unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(failed.migration.failed.len(), 1);
    assert_eq!(failed.session_count(), 0);
    assert!(root.join("Readable/_meta.json").is_file());
    let resumed = store.normalize_startup_layout().await.unwrap();
    assert_eq!(resumed.migration.renamed.len(), 1);
    assert_eq!(resumed.session_count(), 1);
}

#[cfg(unix)]
#[tokio::test]
async fn store_audio_attachment_and_missing_paths_do_not_enumerate_sessions() {
    use std::os::unix::fs::PermissionsExt;
    let (store, vault) = test_store().await;
    store.write_meta(&meta(ID, "target")).await.unwrap();
    store.write_note(ID, "note").await.unwrap();
    let root = vault.path().join("sessions");
    let core = hypr_fs_sync_core::FsSyncCore::new(vault.path().to_path_buf());
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o111)).unwrap();
    assert!(std::fs::read_dir(&root).is_err());
    assert_eq!(store.read_note(ID).await.unwrap().as_deref(), Some("note"));
    assert_eq!(
        core.resolve_session_dir(ID).unwrap(),
        vault.path().join(CANONICAL_DIR)
    );
    assert!(core.attachment_list(ID).unwrap().is_empty());
    assert!(!hypr_fs_sync_core::audio::exists(&core.resolve_session_dir(ID).unwrap()).unwrap());
    assert!(core.attachment_list("missing").unwrap().is_empty());
    assert!(store.read_meta("missing").await.unwrap().is_none());
    assert!(store.read_note("missing").await.unwrap().is_none());
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
}

#[tokio::test]
#[ignore = "local filesystem scale measurement"]
async fn flat_layout_scale_measurement() {
    use std::time::Instant;
    let (store, vault) = test_store().await;
    for i in 0..10_000 {
        let id = format!("session-{i}");
        seed_session_at(
            vault.path(),
            &format!("sessions/readable {i}"),
            &id,
            "Planning",
        );
    }
    let start = Instant::now();
    let layout = store.normalize_startup_layout().await.unwrap();
    eprintln!(
        "10,000-session shallow scan and migration: {:?}",
        start.elapsed()
    );
    assert_eq!(layout.migration.renamed.len(), 10_000);
    let start = Instant::now();
    let report = store
        .rebuild_index_from_startup_layout(layout)
        .await
        .unwrap();
    eprintln!(
        "10,000-session initial content index: {:?}",
        start.elapsed()
    );
    assert_eq!(report.sessions, 10_000);
    for id in ["session-5000", "missing"] {
        let start = Instant::now();
        for _ in 0..1_000 {
            let _ = hypr_vault_read::meta::read_note(vault.path(), id).unwrap();
            let _ = hypr_vault_read::meta::read_session_meta(vault.path(), id).unwrap();
        }
        eprintln!("1,000 note+metadata lookups ({id}): {:?}", start.elapsed());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn startup_index_preserves_existing_metadata_when_a_fresh_read_fails() {
    use std::os::unix::fs::PermissionsExt;
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), "sessions/Readable", ID, "snapshot");
    let layout = store.normalize_startup_layout().await.unwrap();
    store.refresh_session(ID).await.unwrap();
    let path = vault.path().join(CANONICAL_DIR).join("_meta.json");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0)).unwrap();
    let report = store
        .rebuild_index_from_startup_layout(layout)
        .await
        .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(report.errors.len(), 1, "{:?}", report.errors);
    assert!(report.errors[0].contains("_meta.json"));
    assert_eq!(report.sessions, 0);
    assert_eq!(store.session_get(ID).unwrap().meta.title, "snapshot");
}
