use std::fs;
use std::path::Path;

use hypr_vault_write::{SessionMeta, SessionMetaPatch, SessionStore};
use vault_sync::{
    Error, crypto::VaultKey, replica::LocalReplica, scope::Entity, snapshot::Snapshot,
};

fn seed(vault: &Path, value: &str) {
    let session = vault.join("sessions/one");
    fs::create_dir_all(&session).unwrap();
    fs::write(session.join("_meta.json"), serde_json::to_vec(&serde_json::json!({
        "id": "one", "title": value, "created_at": "2026-09-16", "tags": [], "unknown": {"preserve": true}
    })).unwrap()).unwrap();
    fs::write(session.join("notes.md"), value).unwrap();
    fs::write(session.join("audio.mp3"), b"recording").unwrap();
}

#[test]
fn contended_cli_read_does_not_starve_the_writers_blocking_io() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
        .block_on(async {
            let vault = tempfile::tempdir().unwrap();
            seed(vault.path(), "before");
            let guard =
                hypr_vault_read::transaction::VaultTransaction::exclusive(vault.path()).unwrap();
            let path = vault.path().to_path_buf();
            let read = tokio::spawn(async move {
                hypr_agent_access::get_meeting(
                    &path,
                    hypr_agent_access::GetMeetingInput {
                        meeting_id: "one".into(),
                    },
                )
                .await
            });
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            let progress = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                tokio::task::spawn_blocking(|| ()),
            )
            .await;
            drop(guard);
            assert!(
                progress.is_ok(),
                "a contended reader consumed the only blocking worker"
            );
            read.await.unwrap().unwrap();
        });
}

#[tokio::test]
async fn another_stores_deletion_cannot_be_resurrected_by_notes_or_tasks() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path(), "before");
    let first = SessionStore::new(vault.path().into());
    let second = SessionStore::new(vault.path().into());
    second.rebuild_index().await.unwrap();
    first.delete_session("one").await.unwrap();
    assert!(second.write_note("one", "stale edit").await.is_err());
    assert!(
        second
            .replace_tasks("session_raw_note", "one", vec![])
            .await
            .is_err()
    );
    assert!(!vault.path().join("sessions/one").exists());
}

#[tokio::test]
async fn independent_stores_do_not_lose_concurrent_metadata_updates() {
    let vault = tempfile::tempdir().unwrap();
    seed(vault.path(), "before");
    let first = SessionStore::new(vault.path().into());
    let second = SessionStore::new(vault.path().into());
    for index in 0..20 {
        let (a, b) = tokio::join!(
            first.update_meta(
                "one",
                SessionMetaPatch {
                    title: Some(format!("title-{index}")),
                    ..Default::default()
                }
            ),
            second.update_meta(
                "one",
                SessionMetaPatch {
                    folder: Some(format!("folder-{index}")),
                    ..Default::default()
                }
            ),
        );
        a.unwrap();
        b.unwrap();
        let meta: SessionMeta = serde_json::from_slice(
            &fs::read(vault.path().join("sessions/one/_meta.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(meta.title, format!("title-{index}"));
        assert_eq!(meta.folder, Some(format!("folder-{index}")));
        assert_eq!(meta.extra["unknown"]["preserve"], true);
    }
}

#[tokio::test]
async fn recording_lease_prevents_capture_by_another_client_without_transient_files() {
    let vault = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    seed(vault.path(), "before");
    let recording = SessionStore::new(vault.path().into());
    recording.prepare_recording("one").await.unwrap();
    let other = SessionStore::new(vault.path().into());
    assert!(other.freeze_for_vault_move().await.is_err());
    assert!(matches!(
        Snapshot::capture(vault.path(), Entity::Session("one".into()), state.path()),
        Err(Error::Recording)
    ));
    recording.release_recording_prepare("one").await.unwrap();
    Snapshot::capture(vault.path(), Entity::Session("one".into()), state.path()).unwrap();
}

#[tokio::test]
async fn cli_reads_and_desktop_index_observe_a_complete_apply() {
    let vault = tempfile::tempdir().unwrap();
    let remote = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    seed(vault.path(), "before");
    seed(remote.path(), "after");
    let store = SessionStore::new(vault.path().into());
    store.rebuild_index().await.unwrap();
    let entity = Entity::Session("one".into());
    let baseline = Snapshot::capture(vault.path(), entity.clone(), state.path()).unwrap();
    let incoming = Snapshot::capture(remote.path(), entity, state.path()).unwrap();
    #[cfg(unix)]
    let recording_inode = {
        use std::os::unix::fs::MetadataExt;
        fs::metadata(vault.path().join("sessions/one/audio.mp3"))
            .unwrap()
            .ino()
    };
    let replica = LocalReplica::open(vault.path(), state.path()).unwrap();
    let apply_store = store.clone();
    let apply = tokio::spawn(async move {
        replica
            .apply_to_store(&apply_store, baseline.manifest().clone(), incoming)
            .await
    });
    for _ in 0..20 {
        let meeting = hypr_agent_access::get_meeting(
            vault.path(),
            hypr_agent_access::GetMeetingInput {
                meeting_id: "one".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(meeting.title, meeting.note.unwrap().markdown);
    }
    apply.await.unwrap().unwrap();
    let indexed = store.session_get("one").unwrap();
    assert_eq!(indexed.meta.title, "after");
    assert_eq!(indexed.note_markdown.as_deref(), Some("after"));
    assert_eq!(
        store.read_meta("one").await.unwrap().unwrap().title,
        "after"
    );
    assert_eq!(store.read_note("one").await.unwrap().unwrap(), "after");
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(
            fs::metadata(vault.path().join("sessions/one/audio.mp3"))
                .unwrap()
                .ino(),
            recording_inode
        );
    }
}

#[test]
fn note_edit_reuses_recording_object_and_manifest_is_bound_to_revision_and_vault() {
    let vault = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    seed(vault.path(), "before");
    let root = VaultKey::generate(uuid::Uuid::new_v4()).unwrap();
    let entity = Entity::Session("one".into());
    let before = Snapshot::capture(vault.path(), entity.clone(), state.path()).unwrap();
    let first = root.prepare_version(&before, None, state.path()).unwrap();
    let first_manifest = root
        .open_manifest(first.revision, &first.encrypted_manifest)
        .unwrap();
    fs::write(vault.path().join("sessions/one/notes.md"), b"edited").unwrap();
    let after = Snapshot::capture(vault.path(), entity, state.path()).unwrap();
    let second = root
        .prepare_version(&after, Some(&first_manifest), state.path())
        .unwrap();
    assert_eq!(second.new_objects.len(), 1);
    assert_eq!(
        first_manifest.objects["audio.mp3"],
        second.manifest.objects["audio.mp3"]
    );
    assert_eq!(
        first_manifest.objects["_meta.json"],
        second.manifest.objects["_meta.json"]
    );
    assert_ne!(
        first_manifest.objects["notes.md"].object,
        second.manifest.objects["notes.md"].object
    );
    assert!(
        root.open_manifest(uuid::Uuid::new_v4(), &first.encrypted_manifest)
            .is_err()
    );
    let another = VaultKey::generate(uuid::Uuid::new_v4()).unwrap();
    assert!(
        another
            .open_manifest(first.revision, &first.encrypted_manifest)
            .is_err()
    );
    let restored = root
        .restore_snapshot(
            second.revision,
            &second.encrypted_manifest,
            &second.manifest.membership(),
            state.path(),
            |object| {
                let source = second
                    .new_objects
                    .get(&object)
                    .or_else(|| first.new_objects.get(&object))
                    .unwrap();
                Ok(fs::File::open(source)?)
            },
        )
        .unwrap();
    assert_eq!(restored.manifest(), after.manifest());
    assert_eq!(
        fs::read(restored.directory().join("notes.md")).unwrap(),
        b"edited"
    );
    assert!(
        root.restore_snapshot(
            second.revision,
            &second.encrypted_manifest,
            &Default::default(),
            state.path(),
            |_| panic!("must reject membership before fetching")
        )
        .is_err()
    );
}
