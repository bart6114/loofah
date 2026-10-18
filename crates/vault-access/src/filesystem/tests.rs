use super::*;
use serde_json::json;

thread_local! {
    static CRASH_AFTER: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

pub(super) fn crash_point() {
    CRASH_AFTER.with(|point| {
        if let Some(remaining) = point.get() {
            assert!(remaining != 0, "simulated process crash");
            point.set(Some(remaining - 1));
        }
    });
}

fn storage(root: &Path) -> FilesystemStorage {
    FilesystemStorage::new(root.join("content"), root.join("state")).unwrap()
}

#[test]
fn interrupted_batch_recovers_and_retries_preserve_the_recorded_outcome() {
    let root = tempfile::tempdir().unwrap();
    let store = storage(root.path());
    fs::create_dir_all(store.content.join("second.md")).unwrap();
    let error = store
        .commit(
            "op1",
            "same",
            &Versions::new(),
            vec![
                Change::Write("first.md".into(), b"first".to_vec()),
                Change::Write("second.md".into(), b"second".to_vec()),
            ],
            json!("saved"),
        )
        .unwrap_err();
    assert_eq!(error.code, "commit_pending");
    assert_eq!(store.read("first.md").unwrap().unwrap(), b"first");
    fs::remove_dir(store.content.join("second.md")).unwrap();
    drop(store);
    let store = storage(root.path());
    assert_eq!(store.read("second.md").unwrap().unwrap(), b"second");
    let reply = store
        .commit(
            "op1",
            "same",
            &Versions::new(),
            vec![Change::Write("first.md".into(), b"wrong".to_vec())],
            json!("wrong"),
        )
        .unwrap();
    assert_eq!(reply.data, "saved");
    assert_eq!(store.read("first.md").unwrap().unwrap(), b"first");
    assert_eq!(
        store
            .commit("op1", "different", &Versions::new(), vec![], json!(null))
            .unwrap_err()
            .code,
        "invalid_request"
    );
}

#[test]
fn stale_replacement_changes_nothing_and_delete_restore_preserves_unknown_files() {
    let root = tempfile::tempdir().unwrap();
    let store = storage(root.path());
    store
        .commit(
            "create",
            "create",
            &Versions::new(),
            vec![Change::Write(
                "sessions/s1/notes.md".into(),
                b"one".to_vec(),
            )],
            json!(null),
        )
        .unwrap();
    let expected = store.versions(&["sessions/s1/notes.md".into()]).unwrap();
    store
        .commit(
            "edit",
            "edit",
            &expected,
            vec![Change::Write(
                "sessions/s1/notes.md".into(),
                b"two".to_vec(),
            )],
            json!(null),
        )
        .unwrap();
    assert_eq!(
        store
            .commit(
                "stale",
                "stale",
                &expected,
                vec![Change::Write(
                    "sessions/s1/notes.md".into(),
                    b"lost".to_vec()
                )],
                json!(null)
            )
            .unwrap_err()
            .code,
        "conflict"
    );
    fs::write(
        store.content.join("sessions/s1/unrecognized.pdf"),
        b"private attachment",
    )
    .unwrap();
    store
        .commit(
            "delete",
            "delete",
            &Versions::new(),
            vec![Change::DeleteSession("s1".into())],
            json!(null),
        )
        .unwrap();
    drop(store);
    let store = storage(root.path());
    assert!(store.deleted("s1").unwrap());
    store
        .commit(
            "restore",
            "restore",
            &Versions::new(),
            vec![Change::RestoreSession("s1".into())],
            json!(true),
        )
        .unwrap();
    assert_eq!(store.read("sessions/s1/notes.md").unwrap().unwrap(), b"two");
    assert_eq!(
        fs::read(store.content.join("sessions/s1/unrecognized.pdf")).unwrap(),
        b"private attachment"
    );
}

#[test]
fn exclusive_root_ownership_and_unsafe_paths_are_rejected() {
    let root = tempfile::tempdir().unwrap();
    let _owner = RootLock::acquire(root.path()).unwrap();
    assert!(RootLock::acquire(root.path()).is_err());
    for key in ["../secret", "/secret", "a/../../secret", "a\\secret"] {
        assert!(safe_path(root.path(), key).is_err());
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/tmp", root.path().join("link")).unwrap();
        assert!(safe_path(root.path(), "link/secret").is_err());
    }
}

#[test]
fn every_interrupted_write_boundary_recovers_a_whole_transaction_and_one_receipt() {
    let mut boundaries = 0;
    for crash_after in 0..100 {
        let root = tempfile::tempdir().unwrap();
        let store = storage(root.path());
        atomic_write(&store.content.join("note.md"), b"original").unwrap();
        atomic_write(&store.content.join("legacy.md"), b"legacy").unwrap();
        atomic_write(
            &store.content.join("sessions/deleted/unknown.bin"),
            b"keep deleted",
        )
        .unwrap();
        atomic_write(
            &store.state.join("trash/restored/unknown.bin"),
            b"keep restored",
        )
        .unwrap();
        let source = root.path().join("upload");
        atomic_write(&source, b"uploaded bytes").unwrap();
        let expected = store
            .versions(&["note.md".into(), "legacy.md".into()])
            .unwrap();
        let changes = || {
            vec![
                Change::Write("note.md".into(), b"updated".to_vec()),
                Change::Remove("legacy.md".into()),
                Change::Publish(
                    "attachment.bin".into(),
                    source.clone(),
                    crate::digest(b"uploaded bytes"),
                ),
                Change::DeleteSession("deleted".into()),
                Change::RestoreSession("restored".into()),
            ]
        };
        CRASH_AFTER.with(|point| point.set(Some(crash_after)));
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store
                .commit(
                    "batch",
                    "fingerprint",
                    &expected,
                    changes(),
                    json!({"saved":true}),
                )
                .unwrap()
        }));
        CRASH_AFTER.with(|point| point.set(None));
        let committed = store.state.join("journal/batch/transaction.json").exists();
        drop(store);
        let store = storage(root.path());
        assert_eq!(
            store.read("note.md").unwrap().unwrap(),
            if committed {
                &b"updated"[..]
            } else {
                &b"original"[..]
            }
        );
        assert_eq!(
            store.read("legacy.md").unwrap(),
            (!committed).then(|| b"legacy".to_vec())
        );
        assert_eq!(
            store.read("attachment.bin").unwrap(),
            committed.then(|| b"uploaded bytes".to_vec())
        );
        assert_eq!(store.deleted("deleted").unwrap(), committed);
        assert_eq!(store.deleted("restored").unwrap(), !committed);
        let deleted = if committed {
            store.state.join("trash/deleted")
        } else {
            store.content.join("sessions/deleted")
        };
        let restored = if committed {
            store.content.join("sessions/restored")
        } else {
            store.state.join("trash/restored")
        };
        assert_eq!(
            fs::read(deleted.join("unknown.bin")).unwrap(),
            b"keep deleted"
        );
        assert_eq!(
            fs::read(restored.join("unknown.bin")).unwrap(),
            b"keep restored"
        );
        assert_eq!(fs::read(&source).unwrap(), b"uploaded bytes");
        let reply = store
            .commit(
                "batch",
                "fingerprint",
                &expected,
                changes(),
                json!({"saved":true}),
            )
            .unwrap();
        assert_eq!(reply.data, json!({"saved":true}));
        assert_eq!(
            reply.versions["attachment.bin"],
            Some(crate::digest(b"uploaded bytes"))
        );
        assert_eq!(reply.versions["legacy.md"], None);
        store.recover().unwrap();
        assert_eq!(store.read("note.md").unwrap().unwrap(), b"updated");
        assert_eq!(
            store
                .operation("batch", Some("fingerprint"))
                .unwrap()
                .unwrap()
                .data,
            reply.data
        );
        if result.is_ok() {
            boundaries = crash_after;
            break;
        }
    }
    assert!(
        boundaries >= 15,
        "all staging, publish, archive, rename and receipt boundaries must be exercised"
    );
}

#[test]
fn recovery_can_itself_be_interrupted_without_losing_archived_files() {
    for crash_after in 0..10 {
        let root = tempfile::tempdir().unwrap();
        let store = storage(root.path());
        atomic_write(&store.content.join("legacy.md"), b"original").unwrap();
        atomic_write(
            &store.content.join("sessions/s1/unknown.bin"),
            b"attachment",
        )
        .unwrap();
        let directory = store.state.join("journal/recover");
        create_dir_all(&directory).unwrap();
        atomic_write(&directory.join("0"), b"new").unwrap();
        atomic_json(
            &directory.join("transaction.json"),
            &Transaction {
                id: "recover".into(),
                fingerprint: "same".into(),
                complete: false,
                actions: vec![
                    Action::Remove {
                        key: "legacy.md".into(),
                    },
                    Action::DeleteSession { id: "s1".into() },
                    Action::Write {
                        key: "new.md".into(),
                        staged: "0".into(),
                    },
                ],
                reply: Reply {
                    data: json!(true),
                    versions: Versions::new(),
                },
            },
        )
        .unwrap();
        CRASH_AFTER.with(|point| point.set(Some(crash_after)));
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.recover()));
        CRASH_AFTER.with(|point| point.set(None));
        drop(store);
        let store = storage(root.path());
        assert_eq!(store.read("new.md").unwrap().unwrap(), b"new");
        assert_eq!(store.read("legacy.md").unwrap(), None);
        assert_eq!(
            fs::read(store.state.join("trash-files/recover/legacy.md")).unwrap(),
            b"original"
        );
        assert_eq!(
            fs::read(store.state.join("trash/s1/unknown.bin")).unwrap(),
            b"attachment"
        );
    }
}

#[test]
fn corrupt_journals_fail_closed_and_conflicting_restore_never_overwrites_content() {
    let root = tempfile::tempdir().unwrap();
    let store = storage(root.path());
    atomic_write(&store.content.join("sessions/s1/notes.md"), b"live").unwrap();
    atomic_write(&store.state.join("trash/s1/notes.md"), b"archived").unwrap();
    assert_eq!(
        store
            .commit(
                "restore",
                "restore",
                &Versions::new(),
                vec![Change::RestoreSession("s1".into())],
                Value::Null
            )
            .unwrap_err()
            .code,
        "commit_pending"
    );
    assert_eq!(
        store.read("sessions/s1/notes.md").unwrap().unwrap(),
        b"live"
    );
    assert_eq!(
        fs::read(store.state.join("trash/s1/notes.md")).unwrap(),
        b"archived"
    );
    assert!(FilesystemStorage::new(store.content.clone(), store.state.clone()).is_err());
    atomic_write(
        &store.state.join("journal/broken/transaction.json"),
        b"{broken",
    )
    .unwrap();
    assert!(store.recover().is_err());
    assert_eq!(
        store.read("sessions/s1/notes.md").unwrap().unwrap(),
        b"live"
    );
}
