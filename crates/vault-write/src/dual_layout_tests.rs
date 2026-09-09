//! Phase 2 regression suite: the store must behave identically whether a session
//! lives in a legacy UUID-named directory or a human-readable (possibly nested,
//! possibly manually renamed) one. Identity is `_meta.json.id`; the location catalog
//! resolves every session-scoped read and write to the physical directory.

use std::path::{Path, PathBuf};

use crate::{SessionMeta, SessionMetaPatch, SessionStore};

const ID: &str = "6ba7b810-9dad-11d1-80b4-00c04fd430c8";
const READABLE_DIR: &str = "sessions/Work/2026-03-20 — Product planning — 6ba7b8";
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
async fn cold_rebuild_indexes_both_layouts_by_full_id() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    seed_session_at(
        vault.path(),
        &format!("sessions/{LEGACY_ID}"),
        LEGACY_ID,
        "Legacy",
    );

    let report = store.rebuild_index().await.unwrap();

    assert_eq!(report.sessions, 2);
    assert!(report.errors.is_empty(), "{:?}", report.errors);
    assert_eq!(store.session_get(ID).unwrap().meta.title, "Readable");
    assert_eq!(store.session_get(LEGACY_ID).unwrap().meta.title, "Legacy");
    assert_eq!(
        store.session_dir(ID).await.unwrap(),
        PathBuf::from(READABLE_DIR)
    );
    assert_eq!(
        store.session_dir(LEGACY_ID).await.unwrap(),
        PathBuf::from(format!("sessions/{LEGACY_ID}"))
    );
}

/// Every session-scoped write resolves the physical directory -- even on a cold store
/// with no rebuild yet (the CLI shape) -- and must never recreate `sessions/<id>`.
#[tokio::test]
async fn writes_by_full_id_land_in_the_readable_directory_not_a_new_uuid_one() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    let dir = vault.path().join(READABLE_DIR);

    store
        .update_meta(
            ID,
            SessionMetaPatch {
                title: Some("Renamed".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    store.write_note(ID, "note body").await.unwrap();
    store
        .write_enhanced_doc(&crate::EnhancedDoc {
            id: "doc-1".to_string(),
            session_id: ID.to_string(),
            kind: "summary".to_string(),
            title: "Recap".to_string(),
            template_id: String::new(),
            sort_order: 1,
            markdown: "# Recap".to_string(),
        })
        .await
        .unwrap();
    store
        .write_transcript(ID, transcript("t1", "hello"))
        .await
        .unwrap();
    store
        .replace_tasks(
            "session_raw_note",
            ID,
            vec![crate::TaskInput {
                id: "task-1".to_string(),
                source_order: 0,
                status: "todo".to_string(),
                text: "Do it".to_string(),
                body: serde_json::json!([]),
                due_at: String::new(),
            }],
        )
        .await
        .unwrap();

    let read_back: SessionMeta =
        serde_json::from_slice(&std::fs::read(dir.join("_meta.json")).unwrap()).unwrap();
    assert_eq!(read_back.title, "Renamed");
    assert_eq!(
        std::fs::read_to_string(dir.join("notes.md")).unwrap(),
        "note body"
    );
    assert!(dir.join("enhanced/doc-1.md").is_file());
    assert!(dir.join("transcript.json").is_file());
    assert!(dir.join("tasks.json").is_file());

    assert!(
        !vault.path().join(format!("sessions/{ID}")).exists(),
        "no UUID-named directory may be recreated beside the readable one"
    );

    assert_eq!(store.read_note(ID).await.unwrap().unwrap(), "note body");
    assert_eq!(
        store
            .read_enhanced_doc(ID, "doc-1")
            .await
            .unwrap()
            .unwrap()
            .markdown,
        "# Recap"
    );
    assert_eq!(
        store.list_tasks("session_raw_note", ID).await.unwrap()[0].text,
        "Do it"
    );
}

#[tokio::test]
async fn live_transcript_flush_lands_in_the_readable_directory() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");

    store
        .append_transcript(
            ID,
            crate::TranscriptDelta {
                transcript_id: "t1".to_string(),
                new_words: vec![hypr_fs_format::TranscriptWord {
                    id: Some("w0".to_string()),
                    text: "live".to_string(),
                    start_ms: 0.0,
                    end_ms: 0.0,
                    channel: 0.0,
                    speaker: None,
                    metadata: None,
                }],
                replaced_ids: vec![],
                new_hints: vec![],
                started_at_ms: 0.0,
            },
        )
        .await
        .unwrap();
    store.flush_transcript(ID).await.unwrap();

    let raw =
        std::fs::read_to_string(vault.path().join(READABLE_DIR).join("transcript.json")).unwrap();
    assert!(raw.contains("live"));
    assert!(!vault.path().join(format!("sessions/{ID}")).exists());
}

#[tokio::test]
async fn audio_store_list_delete_use_the_readable_directory() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    let dir = vault.path().join(READABLE_DIR);

    let source = vault.path().join("import.wav");
    std::fs::write(&source, b"wav-bytes").unwrap();
    let stored = store
        .store_audio(ID, source.to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(Path::new(&stored), dir.join("audio.wav"));
    assert!(!source.exists());

    std::fs::create_dir_all(dir.join("audio")).unwrap();
    std::fs::write(dir.join("audio/take.wav"), b"").unwrap();
    assert_eq!(
        store.list_audio(ID).await.unwrap(),
        vec!["take.wav".to_string()]
    );
    store.delete_audio(ID, "take.wav").await.unwrap();
    assert!(!dir.join("audio/take.wav").exists());
}

#[tokio::test]
async fn delete_and_restore_preserve_the_readable_name_and_nested_folder() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    store.rebuild_index().await.unwrap();
    store.write_note(ID, "keep me").await.unwrap();

    store.delete_session(ID).await.unwrap();
    assert!(!vault.path().join(READABLE_DIR).exists());
    assert!(store.session_get(ID).is_none());
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let trashed = vault.path().join(".trash").join(&date).join(READABLE_DIR);
    assert!(
        trashed.join("notes.md").is_file(),
        "trash must preserve the readable nested path, not a UUID reconstruction"
    );

    assert!(store.restore_session(ID).await.unwrap());
    assert_eq!(
        std::fs::read_to_string(vault.path().join(READABLE_DIR).join("notes.md")).unwrap(),
        "keep me"
    );
    assert_eq!(store.session_get(ID).unwrap().meta.title, "Readable");
    assert_eq!(
        store.session_dir(ID).await.unwrap(),
        PathBuf::from(READABLE_DIR)
    );
}

#[tokio::test]
async fn two_same_day_delete_restore_cycles_round_trip() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    store.write_note(ID, "first").await.unwrap();

    store.delete_session(ID).await.unwrap();
    assert!(store.restore_session(ID).await.unwrap());
    store.write_note(ID, "second").await.unwrap();
    store.delete_session(ID).await.unwrap();
    assert!(store.restore_session(ID).await.unwrap());

    assert_eq!(
        std::fs::read_to_string(vault.path().join(READABLE_DIR).join("notes.md")).unwrap(),
        "second",
        "each cycle must restore the exact directory trashed by the latest delete"
    );
}

#[tokio::test]
async fn restore_fails_safely_when_the_destination_is_occupied() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    store.delete_session(ID).await.unwrap();

    // Something (a sync client, the user) recreated the destination in the meantime.
    std::fs::create_dir_all(vault.path().join(READABLE_DIR)).unwrap();
    std::fs::write(vault.path().join(READABLE_DIR).join("other.md"), "not ours").unwrap();

    assert!(store.restore_session(ID).await.is_err());
    assert_eq!(
        std::fs::read_to_string(vault.path().join(READABLE_DIR).join("other.md")).unwrap(),
        "not ours",
        "a failed restore must never merge onto or replace the occupied destination"
    );
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(
        vault
            .path()
            .join(".trash")
            .join(&date)
            .join(READABLE_DIR)
            .join("_meta.json")
            .is_file(),
        "the trash entry must stay for manual recovery"
    );
}

#[tokio::test]
async fn restore_rejects_a_tampered_trash_entry_and_expires_a_vanished_one() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    store.delete_session(ID).await.unwrap();

    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let trashed = vault.path().join(".trash").join(&date).join(READABLE_DIR);
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
    seed_session_at(vault2.path(), READABLE_DIR, ID, "Readable");
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
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    store.delete_session(ID).await.unwrap();

    let cold = SessionStore::new(vault.path().to_path_buf());
    assert!(!cold.restore_session(ID).await.unwrap());
    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    assert!(
        vault
            .path()
            .join(".trash")
            .join(&date)
            .join(READABLE_DIR)
            .is_dir(),
        "the trashed directory must remain for manual recovery"
    );
}

/// Two directories claiming the same id are an explicit ambiguity: reads and writes
/// error instead of picking a winner, delete refuses to guess which one to trash,
/// and rebuild reports the claim while leaving both directories untouched.
#[tokio::test]
async fn duplicate_ids_block_reads_writes_and_delete_rather_than_picking_a_directory() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Copy A");
    seed_session_at(vault.path(), "sessions/Planning copy", ID, "Copy B");
    seed_session_at(
        vault.path(),
        &format!("sessions/{LEGACY_ID}"),
        LEGACY_ID,
        "Healthy",
    );

    assert!(store.read_meta(ID).await.is_err());
    assert!(store.write_note(ID, "nope").await.is_err());
    assert!(store.delete_session(ID).await.is_err());
    assert!(vault.path().join(READABLE_DIR).is_dir());
    assert!(vault.path().join("sessions/Planning copy").is_dir());

    let report = store.rebuild_index().await.unwrap();
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("multiple directories")),
        "{:?}",
        report.errors
    );
    assert_eq!(
        store.session_get(LEGACY_ID).unwrap().meta.title,
        "Healthy",
        "the ambiguity must not hide healthy sessions"
    );
    assert!(store.session_get(ID).is_none());
}

/// An external rename (Finder, sync client) followed by a rebuild keeps the logical
/// index entry alive and redirects subsequent writes to the new directory -- the old
/// path must not be recreated.
#[tokio::test]
async fn external_rename_followed_by_rebuild_preserves_the_entry_and_redirects_writes() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    store.rebuild_index().await.unwrap();
    store.write_note(ID, "before rename").await.unwrap();

    let renamed_dir = "sessions/Work/2026-03-20 — Renamed by hand — 6ba7b8";
    std::fs::rename(
        vault.path().join(READABLE_DIR),
        vault.path().join(renamed_dir),
    )
    .unwrap();
    store.rebuild_index().await.unwrap();

    assert_eq!(
        store.session_get(ID).unwrap().meta.title,
        "Readable",
        "the logical entry must survive the physical rename"
    );
    assert_eq!(
        store.session_dir(ID).await.unwrap(),
        PathBuf::from(renamed_dir)
    );

    store.write_note(ID, "after rename").await.unwrap();
    assert_eq!(
        std::fs::read_to_string(vault.path().join(renamed_dir).join("notes.md")).unwrap(),
        "after rename"
    );
    assert!(
        !vault.path().join(READABLE_DIR).exists(),
        "the write must land only in the renamed directory"
    );
}

#[tokio::test]
async fn rebuild_reports_layout_diagnostics_and_nested_ghosts() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    let corrupt = vault.path().join("sessions/broken");
    std::fs::create_dir_all(&corrupt).unwrap();
    std::fs::write(corrupt.join("_meta.json"), "{ invalid").unwrap();
    let ghost = vault.path().join("sessions/Work/ghost");
    std::fs::create_dir_all(&ghost).unwrap();
    std::fs::write(ghost.join("transcript.json"), "{}").unwrap();

    let report = store.rebuild_index().await.unwrap();

    assert_eq!(report.sessions, 1);
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains(&format!("sessions{}broken", std::path::MAIN_SEPARATOR))),
        "{:?}",
        report.errors
    );
    assert_eq!(
        report
            .ghost_sessions
            .iter()
            .map(Path::new)
            .collect::<Vec<_>>(),
        vec![Path::new("Work/ghost")]
    );
    assert!(store.session_get(ID).is_some());
}

/// Same-content concurrency contract as the legacy layout: patches of different meta
/// fields racing each other against a readable directory must both survive.
#[tokio::test]
async fn concurrent_meta_patches_against_a_readable_directory_both_survive() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Original");
    let store = std::sync::Arc::new(store);

    let a = {
        let store = store.clone();
        async move {
            store
                .update_meta(
                    ID,
                    SessionMetaPatch {
                        title: Some("Renamed".to_string()),
                        ..Default::default()
                    },
                )
                .await
        }
    };
    let b = {
        let store = store.clone();
        async move {
            store
                .update_meta(
                    ID,
                    SessionMetaPatch {
                        tags: Some(vec!["tagged".to_string()]),
                        ..Default::default()
                    },
                )
                .await
        }
    };
    let (ra, rb) = tokio::join!(a, b);
    ra.unwrap();
    rb.unwrap();

    let after = store.read_meta(ID).await.unwrap().unwrap();
    assert_eq!(after.title, "Renamed");
    assert_eq!(after.tags, vec!["tagged".to_string()]);
    assert!(!vault.path().join(format!("sessions/{ID}")).exists());
}

/// The watcher's reverse lookup: a vault-relative path maps to the logical id of the
/// cataloged session directory it sits under, NFC-insensitively, longest prefix wins.
#[tokio::test]
async fn session_id_for_relative_path_resolves_nested_and_nfd_paths() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    seed_session_at(
        vault.path(),
        &format!("sessions/{LEGACY_ID}"),
        LEGACY_ID,
        "Legacy",
    );
    store.rebuild_index().await.unwrap();

    assert_eq!(
        store.session_id_for_relative_path(Path::new(READABLE_DIR)),
        Some(ID.to_string())
    );
    assert_eq!(
        store.session_id_for_relative_path(&Path::new(READABLE_DIR).join("enhanced/doc-1.md")),
        Some(ID.to_string())
    );
    assert_eq!(
        store.session_id_for_relative_path(
            &Path::new("sessions/Work/2026-03-20 — Product planning — 6ba7b8").join("notes.md")
        ),
        Some(ID.to_string())
    );
    assert_eq!(
        store.session_id_for_relative_path(
            &Path::new("sessions").join(LEGACY_ID).join("_meta.json")
        ),
        Some(LEGACY_ID.to_string())
    );
    assert_eq!(
        store.session_id_for_relative_path(Path::new("sessions/Work/unknown dir/_meta.json")),
        None
    );
    assert_eq!(
        store.session_id_for_relative_path(Path::new("sessions")),
        None
    );
}

// -- Phase 4: creation chooses readable directory names --

#[tokio::test]
async fn creating_a_titled_session_yields_a_readable_directory_and_a_full_uuid_id() {
    let (store, vault) = test_store().await;
    let mut m = meta(ID, "Product planning");
    m.created_at = "2026-03-20T08:00:00Z".to_string();
    m.started_at = Some("2026-03-20".to_string());
    store.write_meta(&m).await.unwrap();

    let dir = store.session_dir(ID).await.unwrap();
    let name = dir.file_name().unwrap().to_str().unwrap();
    assert_eq!(name, "2026-03-20 — Product planning — 6ba7b8");
    assert!(vault.path().join(&dir).join("_meta.json").is_file());
    assert!(
        !vault.path().join(format!("sessions/{ID}")).exists(),
        "no UUID-named directory may be created"
    );
    assert_eq!(
        store.read_meta(ID).await.unwrap().unwrap().id,
        ID,
        "the logical id stays the full UUID even though the folder is readable"
    );

    store.write_note(ID, "note").await.unwrap();
    assert!(vault.path().join(&dir).join("notes.md").is_file());
}

#[tokio::test]
async fn creating_an_untitled_session_yields_the_provisional_name() {
    let (store, _vault) = test_store().await;
    let mut m = meta(ID, "");
    m.started_at = Some("2026-03-20".to_string());
    store.write_meta(&m).await.unwrap();

    let dir = store.session_dir(ID).await.unwrap();
    let name = dir.file_name().unwrap().to_str().unwrap();
    assert_eq!(name, "2026-03-20 — Untitled — 6ba7b8");
    assert!(crate::layout_name::is_provisional_untitled_name(name));
}

#[tokio::test]
async fn creation_widens_the_suffix_when_the_first_candidate_is_occupied() {
    let (store, vault) = test_store().await;
    // Another session already owns the 6-char-suffix name for this date+title.
    seed_session_at(
        vault.path(),
        "sessions/2026-03-20 — Standup — 6ba7b8",
        LEGACY_ID,
        "Occupant",
    );

    let mut m = meta(ID, "Standup");
    m.started_at = Some("2026-03-20".to_string());
    store.write_meta(&m).await.unwrap();

    let dir = store.session_dir(ID).await.unwrap();
    assert_eq!(
        dir.file_name().unwrap().to_str().unwrap(),
        "2026-03-20 — Standup — 6ba7b810",
        "the 8-char suffix resolves the collision; nothing is merged or overwritten"
    );
    assert_eq!(
        std::fs::read_dir(vault.path().join("sessions"))
            .unwrap()
            .count(),
        2
    );
}

#[tokio::test]
async fn creation_with_a_legacy_non_uuid_id_uses_a_hashed_hex_suffix() {
    let (store, _vault) = test_store().await;
    let mut m = meta("legacy/id?unsafe", "Notes");
    m.started_at = Some("2026-03-20".to_string());
    // validate_session_id rejects slashes, so use an odd-but-safe legacy id instead.
    m.id = "legacy id 001".to_string();
    store.write_meta(&m).await.unwrap();

    let dir = store.session_dir("legacy id 001").await.unwrap();
    let name = dir.file_name().unwrap().to_str().unwrap();
    let suffix = name.rsplit(" — ").next().unwrap();
    assert_eq!(suffix.len(), 6);
    assert!(suffix.chars().all(|c| c.is_ascii_hexdigit()));
    assert!(
        !name.contains("legacy id 001"),
        "raw id text never lands in the name"
    );
}

#[tokio::test]
async fn rewriting_an_existing_session_meta_keeps_its_directory() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Original");

    store.write_meta(&meta(ID, "Rewritten")).await.unwrap();

    assert_eq!(
        store.session_dir(ID).await.unwrap(),
        PathBuf::from(READABLE_DIR)
    );
    assert_eq!(
        std::fs::read_dir(vault.path().join("sessions/Work"))
            .unwrap()
            .count(),
        1,
        "no second directory may appear for an existing id"
    );
}

/// The recorder can persist transcript/audio for a session before its first meta
/// write (the ghost fallback creates `sessions/<id>`); the meta write must adopt
/// that directory instead of splitting the session across two homes.
#[tokio::test]
async fn first_meta_write_adopts_an_existing_ghost_directory() {
    let (store, vault) = test_store().await;
    store
        .write_transcript(ID, transcript("t1", "early"))
        .await
        .unwrap();
    assert!(
        vault
            .path()
            .join(format!("sessions/{ID}/transcript.json"))
            .is_file()
    );

    store.write_meta(&meta(ID, "Recorded first")).await.unwrap();

    assert_eq!(
        store.session_dir(ID).await.unwrap(),
        PathBuf::from(format!("sessions/{ID}")),
        "the ghost directory is adopted, not orphaned beside a readable sibling"
    );
    assert!(
        vault
            .path()
            .join(format!("sessions/{ID}/_meta.json"))
            .is_file()
    );
    assert_eq!(
        std::fs::read_dir(vault.path().join("sessions"))
            .unwrap()
            .count(),
        1
    );
}

// -- Phase 5: provisional-title reconciliation --

#[tokio::test]
async fn first_title_renames_a_provisional_directory_once_and_only_once() {
    let (store, vault) = test_store().await;
    let mut m = meta(ID, "");
    m.started_at = Some("2026-03-20".to_string());
    store.write_meta(&m).await.unwrap();
    store.write_note(ID, "early note").await.unwrap();
    assert_eq!(
        store
            .session_dir(ID)
            .await
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        "2026-03-20 — Untitled — 6ba7b8"
    );

    store
        .update_meta(
            ID,
            crate::SessionMetaPatch {
                title: Some("Roadmap review".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let dir = store.session_dir(ID).await.unwrap();
    assert_eq!(
        dir.file_name().unwrap().to_str().unwrap(),
        "2026-03-20 — Roadmap review — 6ba7b8"
    );
    assert_eq!(
        std::fs::read_to_string(vault.path().join(&dir).join("notes.md")).unwrap(),
        "early note",
        "contents move with the rename"
    );
    assert!(
        !vault
            .path()
            .join("sessions/2026-03-20 — Untitled — 6ba7b8")
            .exists()
    );

    // Later title edits never rename again.
    store
        .update_meta(
            ID,
            crate::SessionMetaPatch {
                title: Some("Completely different".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.session_dir(ID).await.unwrap(),
        dir,
        "an established (non-provisional) name is stable across title edits"
    );
}

#[tokio::test]
async fn a_title_during_recording_defers_the_rename_until_recording_ends() {
    let (store, vault) = test_store().await;
    let mut m = meta(ID, "");
    m.created_at = "2026-03-20T08:00:00Z".to_string();
    store.write_meta(&m).await.unwrap();
    let provisional = store.session_dir(ID).await.unwrap();

    store
        .mark_recording_started(ID, "2026-03-20T09:00:00.000Z")
        .await
        .unwrap();
    store
        .update_meta(
            ID,
            crate::SessionMetaPatch {
                title: Some("Live standup".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.session_dir(ID).await.unwrap(),
        provisional,
        "renaming while the recorder holds paths into the directory is unsafe"
    );
    assert!(vault.path().join(&provisional).is_dir());

    store
        .mark_recording_ended(ID, "2026-03-20T09:30:00.000Z")
        .await
        .unwrap();
    let dir = store.session_dir(ID).await.unwrap();
    assert!(
        dir.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Live standup"),
        "recording stop reconciles the deferred rename: {dir:?}"
    );
    assert!(!vault.path().join(&provisional).exists());
    assert_eq!(
        store.read_meta(ID).await.unwrap().unwrap().title,
        "Live standup"
    );
}

#[tokio::test]
async fn startup_reconciles_a_provisional_directory_left_by_a_crash() {
    let (store, vault) = test_store().await;
    // A crash after the title write but before the stop-event reconcile: the dir is
    // provisional on disk while its meta already carries a title.
    let mut m = meta(ID, "Crashed mid-recording");
    m.started_at = Some("2026-03-20".to_string());
    seed_session_at(
        vault.path(),
        "sessions/2026-03-20 — Untitled — 6ba7b8",
        ID,
        "Crashed mid-recording",
    );
    let _ = m;

    store.reconcile_provisional_names().await;

    let dir = store.session_dir(ID).await.unwrap();
    assert!(
        dir.file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Crashed mid-recording"),
        "{dir:?}"
    );
    assert!(
        !vault
            .path()
            .join("sessions/2026-03-20 — Untitled — 6ba7b8")
            .exists()
    );
}

#[tokio::test]
async fn an_occupied_final_name_keeps_the_provisional_directory_and_the_title() {
    let (store, vault) = test_store().await;
    let mut m = meta(ID, "");
    m.started_at = Some("2026-03-20".to_string());
    store.write_meta(&m).await.unwrap();
    // Every suffix candidate for the final name is occupied by other directories.
    for suffix in [
        "6ba7b8",
        "6ba7b810",
        "6ba7b8109dad",
        "6ba7b8109dad11d180b400c04fd430c8",
    ] {
        std::fs::create_dir_all(
            vault
                .path()
                .join(format!("sessions/2026-03-20 — Standup — {suffix}")),
        )
        .unwrap();
    }

    store
        .update_meta(
            ID,
            crate::SessionMetaPatch {
                title: Some("Standup".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        store
            .session_dir(ID)
            .await
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap(),
        "2026-03-20 — Untitled — 6ba7b8",
        "no merge, no overwrite: the provisional name stays"
    );
    assert_eq!(
        store.read_meta(ID).await.unwrap().unwrap().title,
        "Standup",
        "the title is user data and survives the failed presentation rename"
    );
}

/// The write journal is re-homed with the rename: repeated normal writes after a
/// provisional-to-final rename must stay recognized as own writes (zero trash).
#[tokio::test]
async fn writes_after_the_reconcile_rename_stay_journal_silent() {
    let (store, vault) = test_store().await;
    let mut m = meta(ID, "");
    m.started_at = Some("2026-03-20".to_string());
    store.write_meta(&m).await.unwrap();
    store.write_note(ID, "before rename").await.unwrap();

    store
        .update_meta(
            ID,
            crate::SessionMetaPatch {
                title: Some("Renamed now".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    store.write_note(ID, "after rename").await.unwrap();
    store.write_note(ID, "after rename again").await.unwrap();

    assert!(
        !vault.path().join(".trash").exists(),
        "own writes across the rename must never be treated as foreign bytes"
    );
}

/// The F1 hole: with one claimant at the canonical legacy `sessions/<id>` path,
/// `find_session`'s fast path can't see the second claimant — after a rebuild has
/// recorded the duplicate, lazy resolution must stay blocked instead of quietly
/// re-adopting the legacy copy and diverging the two.
#[tokio::test]
async fn a_rebuild_recorded_duplicate_blocks_lazy_resolution_of_the_legacy_claimant() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), &format!("sessions/{ID}"), ID, "Canonical");
    seed_session_at(vault.path(), "sessions/Synced copy", ID, "Copy");

    let report = store.rebuild_index().await.unwrap();
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.contains("multiple directories"))
    );
    assert!(store.session_get(ID).is_none());

    assert!(
        store
            .write_note(ID, "must not land anywhere")
            .await
            .is_err(),
        "resolution must not fall back to find_session's legacy fast path"
    );
    assert!(store.read_meta(ID).await.is_err());
    assert!(
        !vault
            .path()
            .join(format!("sessions/{ID}/notes.md"))
            .exists(),
        "no bytes may be written into either claimant"
    );

    // Removing one claimant and rebuilding clears the block.
    std::fs::remove_dir_all(vault.path().join("sessions/Synced copy")).unwrap();
    store.rebuild_index().await.unwrap();
    assert_eq!(store.session_get(ID).unwrap().meta.title, "Canonical");
    store.write_note(ID, "usable again").await.unwrap();
}

/// A permission failure on a personal folder must not make the sessions homed
/// under it look deleted: the prune protects descendants of unreadable dirs.
#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_personal_folder_does_not_prune_its_sessions() {
    use std::os::unix::fs::PermissionsExt;
    let (store, vault) = test_store().await;
    seed_session_at(
        vault.path(),
        "sessions/Work/2026-03-20 — Planning — 6ba7b8",
        ID,
        "Nested",
    );
    store.rebuild_index().await.unwrap();
    assert!(store.session_get(ID).is_some());

    let folder = vault.path().join("sessions/Work");
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000)).unwrap();
    let report = store.rebuild_index().await.unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert!(
        store.session_get(ID).is_some(),
        "a transiently unreadable parent folder must never look like deletion: {:?}",
        report.errors
    );
    assert!(!report.errors.is_empty());
}

// -- cache-only resolution (`session_dir_cached`) ---------------------------------

#[tokio::test]
async fn session_dir_cached_returns_a_validated_hit_without_discovery() {
    let (store, _vault) = test_store().await;
    store.write_meta(&meta(ID, "Planning")).await.unwrap();
    let expected = store.session_dir(ID).await.unwrap();

    let hit = store.session_dir_cached(ID).unwrap();
    assert_eq!(hit, Some(expected));
}

#[tokio::test]
async fn session_dir_cached_misses_when_the_directory_vanished() {
    let (store, vault) = test_store().await;
    store.write_meta(&meta(ID, "Planning")).await.unwrap();
    let dir = vault.path().join(store.session_dir(ID).await.unwrap());
    std::fs::remove_dir_all(&dir).unwrap();

    assert_eq!(store.session_dir_cached(ID).unwrap(), None);
}

#[tokio::test]
async fn session_dir_cached_misses_when_the_cataloged_path_claims_another_id() {
    let (store, vault) = test_store().await;
    store.write_meta(&meta(ID, "Planning")).await.unwrap();
    let dir = vault.path().join(store.session_dir(ID).await.unwrap());
    // An external process replaced the directory's identity out from under the
    // catalog: the stale entry must fall through, never serve the wrong session.
    std::fs::write(
        dir.join("_meta.json"),
        serde_json::to_vec_pretty(&meta(LEGACY_ID, "Impostor")).unwrap(),
    )
    .unwrap();

    assert_eq!(store.session_dir_cached(ID).unwrap(), None);
}

#[tokio::test]
async fn session_dir_cached_tolerates_a_corrupt_meta_at_the_cataloged_path() {
    let (store, vault) = test_store().await;
    store.write_meta(&meta(ID, "Planning")).await.unwrap();
    let relative = store.session_dir(ID).await.unwrap();
    std::fs::write(vault.path().join(&relative).join("_meta.json"), "{ invalid").unwrap();

    // The warm catalog is still the best known home; artifact access keeps its
    // corruption tolerance.
    assert_eq!(store.session_dir_cached(ID).unwrap(), Some(relative));
}

#[tokio::test]
async fn session_dir_cached_errors_on_a_duplicate_claimed_id() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), &format!("sessions/{ID}"), ID, "One");
    seed_session_at(vault.path(), "sessions/Copy", ID, "Two");
    store.rebuild_index().await.unwrap();

    assert!(store.session_dir_cached(ID).is_err());
}

#[tokio::test]
async fn a_cold_scan_warms_the_whole_catalog_so_later_lookups_are_cache_hits() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Readable");
    seed_session_at(vault.path(), "sessions/Other notes", LEGACY_ID, "Other");

    // Cold store: resolving one id pays for a discovery walk...
    assert_eq!(store.session_dir_cached(ID).unwrap(), None, "cold cache");
    store.session_dir(ID).await.unwrap();

    // ...which must warm every healthy location, not just the requested one.
    assert_eq!(
        store.session_dir_cached(LEGACY_ID).unwrap(),
        Some(PathBuf::from("sessions/Other notes"))
    );
}

/// The warmed fs-sync command path performs zero discovery walks: a counting
/// resolver backed by the store proves every per-artifact resolution is a
/// validated catalog hit, so `FsSyncCore`'s discovery fallback never runs.
#[tokio::test]
async fn warmed_fs_sync_resolution_never_falls_back_to_discovery() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let (store, vault) = test_store().await;
    store.write_meta(&meta(ID, "Planning")).await.unwrap();
    let store = Arc::new(store);

    let misses = Arc::new(AtomicUsize::new(0));
    let resolver_store = store.clone();
    let resolver_misses = misses.clone();
    let vault_base = vault.path().to_path_buf();
    let resolver_base = vault_base.clone();
    let core = hypr_fs_sync_core::FsSyncCore::with_resolver(
        vault_base.clone(),
        Arc::new(move |id: &str| {
            let hit = resolver_store
                .session_dir_cached(id)
                .map_err(|e| hypr_fs_sync_core::Error::Path(e.to_string()))?;
            if hit.is_none() {
                resolver_misses.fetch_add(1, Ordering::SeqCst);
            }
            Ok(hit.map(|relative| resolver_base.join(relative)))
        }),
    );

    let saved = core.attachment_save(ID, b"bytes", "file.txt").unwrap();
    assert_eq!(core.attachment_list(ID).unwrap().len(), 1);
    assert_eq!(
        core.attachment_read(ID, &saved.attachment_id).unwrap(),
        b"bytes"
    );
    let resolved = core.resolve_session_dir(ID).unwrap();
    assert_eq!(
        resolved,
        vault_base.join(store.session_dir(ID).await.unwrap())
    );

    assert_eq!(
        misses.load(Ordering::SeqCst),
        0,
        "every warmed resolution must be a catalog hit -- zero discovery fallbacks"
    );
}

// -- physical-location change events (`IndexEntity::Locations`) -------------------

/// Everything the raw change stream saw, via a tap subscribed before the action.
fn drain(rx: &mut crate::index::IndexChangeReceiver) -> Vec<(crate::IndexEntity, Vec<String>)> {
    let mut out = Vec::new();
    while let Ok(change) = rx.try_recv() {
        out.push(change);
    }
    out
}

#[tokio::test]
async fn a_provisional_rename_emits_a_locations_event() {
    let (store, _vault) = test_store().await;
    store.write_meta(&meta(ID, "")).await.unwrap();
    let mut rx = store.subscribe_index_changes();

    store
        .update_meta(
            ID,
            SessionMetaPatch {
                title: Some("Roadmap review".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let changes = drain(&mut rx);
    assert!(
        changes
            .iter()
            .any(|(entity, ids)| *entity == crate::IndexEntity::Locations
                && ids == &vec![ID.to_string()]),
        "the rename must announce the location change: {changes:?}"
    );
}

#[tokio::test]
async fn an_external_rename_rebuild_emits_only_a_locations_event() {
    let (store, vault) = test_store().await;
    store.write_meta(&meta(ID, "Planning")).await.unwrap();
    store.rebuild_index().await.unwrap();
    let from = vault.path().join(store.session_dir(ID).await.unwrap());
    let mut rx = store.subscribe_index_changes();

    // Finder-style rename: content is untouched, only the physical home moves.
    let to = vault.path().join("sessions/Renamed by hand");
    std::fs::rename(&from, &to).unwrap();
    store.rebuild_index().await.unwrap();

    let changes = drain(&mut rx);
    assert_eq!(
        changes,
        vec![(crate::IndexEntity::Locations, vec![ID.to_string()])],
        "unchanged file content must produce no content-entity events"
    );
    assert_eq!(
        store.session_dir_cached(ID).unwrap(),
        Some(PathBuf::from("sessions/Renamed by hand"))
    );
}

#[tokio::test]
async fn an_unchanged_rebuild_emits_no_locations_events() {
    let (store, _vault) = test_store().await;
    store.write_meta(&meta(ID, "Planning")).await.unwrap();
    store.rebuild_index().await.unwrap();
    let mut rx = store.subscribe_index_changes();

    store.rebuild_index().await.unwrap();

    assert_eq!(drain(&mut rx), vec![]);
}

#[tokio::test]
async fn delete_and_restore_both_emit_locations_events() {
    let (store, _vault) = test_store().await;
    store.write_meta(&meta(ID, "Planning")).await.unwrap();
    let mut rx = store.subscribe_index_changes();

    store.delete_session(ID).await.unwrap();
    let deleted_changes = drain(&mut rx);
    assert!(
        deleted_changes
            .iter()
            .any(|(entity, ids)| *entity == crate::IndexEntity::Locations
                && ids.contains(&ID.to_string())),
        "{deleted_changes:?}"
    );

    assert!(store.restore_session(ID).await.unwrap());
    let restored_changes = drain(&mut rx);
    assert!(
        restored_changes
            .iter()
            .any(|(entity, ids)| *entity == crate::IndexEntity::Locations
                && ids.contains(&ID.to_string())),
        "{restored_changes:?}"
    );
}

// -- recording path leases (`prepare_recording` / `release_recording_prepare`) ----

#[tokio::test]
async fn prepare_recording_blocks_a_first_title_rename_until_the_last_lease_drops() {
    let (store, vault) = test_store().await;
    store.write_meta(&meta(ID, "")).await.unwrap();
    let provisional = store.session_dir(ID).await.unwrap();

    let prepared = store.prepare_recording(ID).await.unwrap();
    assert_eq!(prepared, provisional);

    store
        .update_meta(
            ID,
            SessionMetaPatch {
                title: Some("Roadmap review".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.session_dir(ID).await.unwrap(),
        provisional,
        "the rename must stay deferred while a lease is held"
    );
    assert!(vault.path().join(&provisional).is_dir());

    store.release_recording_prepare(ID).await.unwrap();

    let renamed = store.session_dir(ID).await.unwrap();
    assert_ne!(renamed, provisional, "the last release retries the rename");
    let basename = renamed.file_name().unwrap().to_str().unwrap();
    assert!(basename.contains("Roadmap review"), "{basename}");
    assert!(vault.path().join(&renamed).is_dir());
}

#[tokio::test]
async fn releasing_one_of_multiple_leases_does_not_unprotect_the_recording() {
    let (store, _vault) = test_store().await;
    store.write_meta(&meta(ID, "")).await.unwrap();
    let provisional = store.session_dir(ID).await.unwrap();

    // The frontend's prepare and the transcription command's own lease.
    store.prepare_recording(ID).await.unwrap();
    store.prepare_recording(ID).await.unwrap();

    // A failed duplicate start releases only its own reservation.
    store.release_recording_prepare(ID).await.unwrap();

    store
        .update_meta(
            ID,
            SessionMetaPatch {
                title: Some("Still recording".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        store.session_dir(ID).await.unwrap(),
        provisional,
        "the surviving lease must keep the rename deferred"
    );
}

#[tokio::test]
async fn a_successful_stop_clears_every_lease_and_reconciles() {
    let (store, _vault) = test_store().await;
    store.write_meta(&meta(ID, "")).await.unwrap();
    let provisional = store.session_dir(ID).await.unwrap();

    store.prepare_recording(ID).await.unwrap();
    store.note_recording_active(ID); // Started lifecycle: ensure >= 1, never stack
    store.prepare_recording(ID).await.unwrap();
    store
        .update_meta(
            ID,
            SessionMetaPatch {
                title: Some("Sprint sync".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(store.session_dir(ID).await.unwrap(), provisional);

    // Stopped fires after recorder finalization and clears the whole session.
    store
        .mark_recording_ended(ID, "2026-07-31T10:30:00.000Z")
        .await
        .unwrap();

    let renamed = store.session_dir(ID).await.unwrap();
    assert_ne!(renamed, provisional);
    assert!(
        renamed
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("Sprint sync")
    );
}

#[tokio::test]
async fn a_failed_start_round_trip_leaves_no_permanent_lease() {
    let (store, _vault) = test_store().await;
    store.write_meta(&meta(ID, "")).await.unwrap();

    store.prepare_recording(ID).await.unwrap();
    store.release_recording_prepare(ID).await.unwrap();
    // Releasing without a lease is a safe no-op (paired failure cleanup).
    store.release_recording_prepare(ID).await.unwrap();

    store
        .update_meta(
            ID,
            SessionMetaPatch {
                title: Some("After the failure".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(
        store
            .session_dir(ID)
            .await
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap()
            .contains("After the failure"),
        "no leftover lease may keep deferring the rename"
    );
}

// -- startup normalization from one layout snapshot -------------------------------

#[tokio::test]
async fn normalize_startup_layout_migrates_reconciles_and_feeds_the_rebuild() {
    let vault = tempfile::tempdir().unwrap();
    // A legacy UUID-named directory to migrate...
    let legacy = vault.path().join(format!("sessions/{LEGACY_ID}"));
    std::fs::create_dir_all(&legacy).unwrap();
    let mut legacy_meta = meta(LEGACY_ID, "Planning");
    legacy_meta.started_at = Some("2026-03-20".to_string());
    std::fs::write(
        legacy.join("_meta.json"),
        serde_json::to_vec_pretty(&legacy_meta).unwrap(),
    )
    .unwrap();
    std::fs::write(legacy.join("_memo.md"), "legacy note").unwrap();
    // ...and a crash-leftover provisional directory whose meta already has a title.
    let provisional_name = crate::layout_name::format_session_dir_name("2026-04-01", "", "6ba7b8");
    seed_session_at(
        vault.path(),
        &format!("sessions/{provisional_name}"),
        ID,
        "Roadmap review",
    );

    let store = SessionStore::new(vault.path().to_path_buf());
    let layout = store.normalize_startup_layout().await.unwrap();
    assert_eq!(layout.migration.renamed.len(), 1, "{:?}", layout.migration);

    let report = store
        .rebuild_index_from_startup_layout(layout)
        .await
        .unwrap();
    assert_eq!(report.sessions, 2);
    assert!(report.errors.is_empty(), "{:?}", report.errors);

    // Both renames happened on disk and the snapshot fed rebuild the NEW paths:
    // the catalog and the index resolve to them without another scan.
    let migrated = store.session_dir(LEGACY_ID).await.unwrap();
    assert_eq!(
        migrated,
        PathBuf::from("sessions/2026-03-20 — Planning — 550e84")
    );
    assert!(vault.path().join(&migrated).join("_memo.md").is_file());
    let reconciled = store.session_dir(ID).await.unwrap();
    assert_eq!(
        reconciled,
        PathBuf::from("sessions/2026-04-01 — Roadmap review — 6ba7b8")
    );
    assert_eq!(store.session_get(LEGACY_ID).unwrap().meta.title, "Planning");
    assert_eq!(store.session_get(ID).unwrap().meta.title, "Roadmap review");
}

#[tokio::test]
async fn a_failed_migration_keeps_the_source_path_in_the_snapshot() {
    let vault = tempfile::tempdir().unwrap();
    let legacy = vault.path().join(format!("sessions/{LEGACY_ID}"));
    std::fs::create_dir_all(&legacy).unwrap();
    let mut legacy_meta = meta(LEGACY_ID, "Planning");
    legacy_meta.started_at = Some("2026-03-20".to_string());
    std::fs::write(
        legacy.join("_meta.json"),
        serde_json::to_vec_pretty(&legacy_meta).unwrap(),
    )
    .unwrap();
    // Occupy every readable-name candidate so migration has nowhere to go.
    for suffix in crate::layout_name::short_id_candidates(LEGACY_ID) {
        std::fs::create_dir_all(vault.path().join("sessions").join(
            crate::layout_name::format_session_dir_name("2026-03-20", "Planning", &suffix),
        ))
        .unwrap();
    }

    let store = SessionStore::new(vault.path().to_path_buf());
    let layout = store.normalize_startup_layout().await.unwrap();
    assert!(layout.migration.renamed.is_empty());
    assert!(
        layout
            .migration
            .skipped
            .iter()
            .any(|s| s.contains("no collision-free readable name")),
        "{:?}",
        layout.migration
    );

    let report = store
        .rebuild_index_from_startup_layout(layout)
        .await
        .unwrap();
    assert_eq!(report.sessions, 1);
    // The snapshot still points at the untouched source directory.
    assert_eq!(
        store.session_dir(LEGACY_ID).await.unwrap(),
        PathBuf::from(format!("sessions/{LEGACY_ID}"))
    );
    assert_eq!(store.session_get(LEGACY_ID).unwrap().meta.title, "Planning");
}

/// Preserved discovery semantics after folding the ghost walk into discovery: a
/// healthy session nested under a ghost boundary stays indexed, and the ghost is
/// still reported exactly once.
#[tokio::test]
async fn a_session_nested_under_a_ghost_directory_is_still_indexed() {
    let (store, vault) = test_store().await;
    let ghost = vault.path().join("sessions/Work/ghost");
    std::fs::create_dir_all(&ghost).unwrap();
    std::fs::write(ghost.join("transcript.json"), "{}").unwrap();
    seed_session_at(vault.path(), "sessions/Work/ghost/rescued", ID, "Rescued");

    let report = store.rebuild_index().await.unwrap();

    assert_eq!(
        report
            .ghost_sessions
            .iter()
            .map(Path::new)
            .collect::<Vec<_>>(),
        vec![Path::new("Work/ghost")]
    );
    assert_eq!(report.sessions, 1);
    assert_eq!(store.session_get(ID).unwrap().meta.title, "Rescued");
}

/// Not a benchmark gate -- a manually-run measurement of the single-walk layout
/// scan (`cargo test -p vault-write --release -- --ignored discovery_scale`).
#[tokio::test]
#[ignore]
async fn discovery_scale_measurement() {
    for count in [100usize, 1000, 5000] {
        let vault = tempfile::tempdir().unwrap();
        for i in 0..count {
            let id = format!("aaaaaaaa-0000-4000-8000-{i:012}");
            seed_session_at(
                vault.path(),
                &format!("sessions/2026-03-20 — Session {i} — {i:06}"),
                &id,
                &format!("Session {i}"),
            );
        }
        let store = SessionStore::new(vault.path().to_path_buf());
        let started = std::time::Instant::now();
        let layout = store.normalize_startup_layout().await.unwrap();
        let normalize_elapsed = started.elapsed();
        let started = std::time::Instant::now();
        store
            .rebuild_index_from_startup_layout(layout)
            .await
            .unwrap();
        let startup_rebuild = started.elapsed();
        let started = std::time::Instant::now();
        store.rebuild_index().await.unwrap();
        let focus_rebuild = started.elapsed();
        println!(
            "{count} sessions: normalize {normalize_elapsed:?}, startup rebuild {startup_rebuild:?}, focus rebuild {focus_rebuild:?}"
        );
    }
}

#[tokio::test]
async fn prepare_recording_adopts_a_nested_unclaimed_directory_like_the_recorder() {
    let (store, vault) = test_store().await;
    // A moved recorder ghost: meta-less, named exactly for the id, nested in a
    // personal folder. The recorder (fs-sync resolution) adopts it; the prepared
    // hook path must agree instead of pointing at a fresh legacy directory.
    let ghost = vault.path().join("sessions/Work").join(ID);
    std::fs::create_dir_all(&ghost).unwrap();
    std::fs::write(ghost.join("transcript.json"), "{}").unwrap();

    let dir = store.prepare_recording(ID).await.unwrap();
    assert_eq!(dir, PathBuf::from(format!("sessions/Work/{ID}")));
    store.release_recording_prepare(ID).await.unwrap();
}

// -- explicit user-invoked "rename folder to match title" --

#[tokio::test]
async fn explicit_rename_moves_an_established_directory_to_the_current_title() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Renamed since");
    store.write_note(ID, "kept content").await.unwrap();

    let basename = store.rename_session_dir_to_title(ID).await.unwrap();
    assert_eq!(basename, "2026-03-20 — Renamed since — 6ba7b8");

    let dir = store.session_dir(ID).await.unwrap();
    assert_eq!(
        dir,
        PathBuf::from("sessions/Work/2026-03-20 — Renamed since — 6ba7b8"),
        "the date prefix and parent folder are preserved; only the title segment changes"
    );
    assert_eq!(
        std::fs::read_to_string(vault.path().join(&dir).join("notes.md")).unwrap(),
        "kept content"
    );
    assert!(!vault.path().join(READABLE_DIR).exists());
}

#[tokio::test]
async fn explicit_rename_of_an_already_matching_directory_is_a_noop() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Product planning");

    let basename = store.rename_session_dir_to_title(ID).await.unwrap();
    assert_eq!(basename, "2026-03-20 — Product planning — 6ba7b8");
    assert!(vault.path().join(READABLE_DIR).is_dir());
}

#[tokio::test]
async fn explicit_rename_derives_a_date_for_a_legacy_uuid_directory() {
    let (store, vault) = test_store().await;
    let mut m = meta(ID, "Adopted legacy");
    m.started_at = Some("2026-03-20".to_string());
    let legacy = vault.path().join("sessions").join(ID);
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(
        legacy.join("_meta.json"),
        serde_json::to_vec_pretty(&m).unwrap(),
    )
    .unwrap();

    let basename = store.rename_session_dir_to_title(ID).await.unwrap();
    assert_eq!(basename, "2026-03-20 — Adopted legacy — 6ba7b8");
    assert!(!legacy.exists());
}

#[tokio::test]
async fn explicit_rename_is_refused_while_a_recording_lease_is_held() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "Mid-recording title");

    store.prepare_recording(ID).await.unwrap();
    let error = store.rename_session_dir_to_title(ID).await.unwrap_err();
    assert!(
        error.to_string().contains("recording"),
        "unexpected error: {error}"
    );
    assert!(vault.path().join(READABLE_DIR).is_dir());

    store.release_recording_prepare(ID).await.unwrap();
    store.rename_session_dir_to_title(ID).await.unwrap();
    assert_eq!(
        store.session_dir(ID).await.unwrap(),
        PathBuf::from("sessions/Work/2026-03-20 — Mid-recording title — 6ba7b8")
    );
}

#[tokio::test]
async fn explicit_rename_without_a_title_or_onto_an_occupied_target_errors() {
    let (store, vault) = test_store().await;
    seed_session_at(vault.path(), READABLE_DIR, ID, "");
    assert!(store.rename_session_dir_to_title(ID).await.is_err());

    seed_session_at(
        vault.path(),
        "sessions/Work/2026-03-20 — Blocked — 550e84",
        LEGACY_ID,
        "Blocked",
    );
    // Occupy every suffix-widening candidate for the new title.
    let hex = "550e8400e29b41d4a716446655440000";
    for len in [6usize, 8, 12, hex.len()] {
        std::fs::create_dir_all(vault.path().join(format!(
            "sessions/Work/2026-03-20 — Retitled — {}",
            &hex[..len]
        )))
        .unwrap();
    }
    store
        .update_meta(
            LEGACY_ID,
            crate::SessionMetaPatch {
                title: Some("Retitled".to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let error = store
        .rename_session_dir_to_title(LEGACY_ID)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("occupied"),
        "unexpected error: {error}"
    );
}
