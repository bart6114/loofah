use super::*;
use serde_json::json;

fn fixture() -> (tempfile::TempDir, Arc<ConnectionProfiles>) {
    let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
    let store = ConnectionProfiles::new(root.path().join("profiles")).unwrap();
    (root, store)
}
async fn remote(store: &Arc<ConnectionProfiles>, name: &str) -> DesktopStartupSelection {
    let lease = store.create(name, "https://company.example").await.unwrap();
    let profile = lease.profile().await;
    DesktopStartupSelection::Remote {
        profile_name: profile.name,
        profile_id: profile.id,
    }
}
async fn save(
    store: &Arc<ConnectionProfiles>,
    selection: DesktopStartupSelection,
) -> DesktopStartupSnapshot {
    let before = store.desktop_startup().unwrap();
    let DesktopStartupSaveOutcome::Saved(saved) = store
        .save_desktop_startup(SaveDesktopStartupRequest {
            selection,
            expected_revision: before.revision,
        })
        .await
        .unwrap()
    else {
        panic!("saved")
    };
    saved
}

#[test]
fn absent_startup_is_local_without_creating_device_or_vault_directories() {
    let (root, store) = fixture();
    let snapshot = store.desktop_startup().unwrap();
    assert_eq!(
        snapshot,
        DesktopStartupSnapshot {
            selection: DesktopStartupSelection::Local {},
            revision: ABSENT.into()
        }
    );
    assert!(store.desktop_startup_profile(&snapshot).unwrap().is_none());
    assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
}

#[tokio::test]
async fn startup_remote_binding_survives_restart_and_explicit_local_keeps_connection_bytes() {
    let (_root, store) = fixture();
    let selected = remote(&store, "company").await;
    let profile_bytes = std::fs::read(store.path("company")).unwrap();
    let saved = save(&store, selected.clone()).await;
    let raw = std::fs::read(store.desktop_startup_path()).unwrap();
    assert!(
        !String::from_utf8(raw.clone())
            .unwrap()
            .contains("credential")
    );
    let restarted = ConnectionProfiles::new(store.root.clone()).unwrap();
    assert_eq!(restarted.desktop_startup().unwrap(), saved);
    let profile = restarted.desktop_startup_profile(&saved).unwrap().unwrap();
    assert_eq!(profile.server, "https://company.example/");
    assert_eq!(profile.name, "company");
    let local = save(&restarted, DesktopStartupSelection::Local {}).await;
    assert!(restarted.desktop_startup_profile(&local).unwrap().is_none());
    assert_eq!(std::fs::read(store.path("company")).unwrap(), profile_bytes);
    assert_eq!(restarted.list().await.unwrap().len(), 1);
}

#[tokio::test]
async fn checked_selection_conflicts_without_overwriting_and_noop_preserves_original_bytes() {
    let (_root, store) = fixture();
    let before = store.desktop_startup().unwrap();
    let selected = remote(&store, "company").await;
    let saved = save(&store, selected.clone()).await;
    let bytes = std::fs::read(store.desktop_startup_path()).unwrap();
    assert_eq!(
        store
            .save_desktop_startup(SaveDesktopStartupRequest {
                selection: DesktopStartupSelection::Local {},
                expected_revision: before.revision
            })
            .await
            .unwrap(),
        DesktopStartupSaveOutcome::Conflict(saved.clone())
    );
    assert_eq!(std::fs::read(store.desktop_startup_path()).unwrap(), bytes);
    let spaced = format!("\n{}\n", String::from_utf8(bytes).unwrap());
    std::fs::write(store.desktop_startup_path(), &spaced).unwrap();
    let same = save(&store, selected).await;
    assert_eq!(same.revision, hash(spaced.as_bytes()));
    assert_eq!(
        std::fs::read_to_string(store.desktop_startup_path()).unwrap(),
        spaced
    );
}

#[tokio::test]
async fn deleted_or_recreated_remote_never_resolves_to_local_and_can_be_explicitly_unselected() {
    let (_root, store) = fixture();
    let selected = remote(&store, "company").await;
    let saved = save(&store, selected).await;
    let bytes = std::fs::read(store.desktop_startup_path()).unwrap();
    let lease = store.open("company").await.unwrap();
    lease.remove().await.unwrap();
    drop(lease);
    assert_eq!(
        store.desktop_startup_profile(&saved).unwrap_err().code,
        ErrorCode::NotFound
    );
    assert_eq!(store.desktop_startup().unwrap(), saved);
    let replacement = remote(&store, "company").await;
    assert_ne!(replacement, saved.selection);
    assert_eq!(
        store.desktop_startup_profile(&saved).unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(std::fs::read(store.desktop_startup_path()).unwrap(), bytes);
    assert!(matches!(
        save(&store, DesktopStartupSelection::Local {})
            .await
            .selection,
        DesktopStartupSelection::Local {}
    ));
}

#[tokio::test]
async fn malformed_unknown_and_future_startup_records_are_kept_and_never_fall_back() {
    let (_root, store) = fixture();
    store.prepare().unwrap();
    let id = uuid::Uuid::new_v4().to_string();
    for bytes in [
        "not JSON".to_string(),
        json!({"schema_version":2,"selection":{"kind":"local"}}).to_string(),
        json!({"schema_version":1,"selection":{"kind":"local","future":true}}).to_string(),
        json!({"schema_version":1,"selection":{"kind":"local"},"future":true}).to_string(),
        json!({"schema_version":1,"selection":{"kind":"remote","profile_name":"company","profile_id":"invalid"}}).to_string(),
        json!({"schema_version":1,"selection":{"kind":"future","profile_id":id}}).to_string(),
        "{\"schema_version\":1,\"schema_version\":1,\"selection\":{\"kind\":\"local\"}}".to_string(),
        "{\"schema_version\":1,\"selection\":{\"kind\":\"local\"},\"selection\":{\"kind\":\"local\"}}".to_string(),
        " ".repeat(STARTUP_LIMIT + 1),
    ] {
        std::fs::write(store.desktop_startup_path(), &bytes).unwrap();
        assert!(store.desktop_startup().is_err(), "{bytes}");
        assert!(store.save_desktop_startup(SaveDesktopStartupRequest { selection: DesktopStartupSelection::Local {}, expected_revision: hash(bytes.as_bytes()) }).await.is_err());
        assert_eq!(std::fs::read_to_string(store.desktop_startup_path()).unwrap(), bytes);
    }
}

#[tokio::test]
async fn invalid_input_precedes_private_directory_creation() {
    for (selection, revision) in [
        (
            DesktopStartupSelection::Remote {
                profile_name: "../company".into(),
                profile_id: uuid::Uuid::new_v4().to_string(),
            },
            ABSENT,
        ),
        (
            DesktopStartupSelection::Remote {
                profile_name: "company".into(),
                profile_id: "not-a-uuid".into(),
            },
            ABSENT,
        ),
        (DesktopStartupSelection::Local {}, ""),
    ] {
        let (root, store) = fixture();
        let error = store
            .save_desktop_startup(SaveDesktopStartupRequest {
                selection,
                expected_revision: revision.into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
        assert!(std::fs::read_dir(root.path()).unwrap().next().is_none());
    }
}

#[tokio::test]
async fn duplicate_known_connection_fields_are_not_normalized_by_startup_resolution() {
    let (_root, store) = fixture();
    let selected = remote(&store, "company").await;
    let saved = save(&store, selected).await;
    let original = std::fs::read_to_string(store.path("company")).unwrap();
    let malformed = original.replacen(
        "\"name\": \"company\"",
        "\"name\": \"company\", \"name\": \"company\"",
        1,
    );
    assert_ne!(malformed, original);
    std::fs::write(store.path("company"), &malformed).unwrap();
    assert!(store.desktop_startup_profile(&saved).is_err());
    assert!(store.open("company").await.is_err());
    assert_eq!(
        std::fs::read_to_string(store.path("company")).unwrap(),
        malformed
    );
}

#[cfg(unix)]
#[tokio::test]
async fn startup_symlinks_do_not_redirect_reads_or_writes() {
    use std::os::unix::fs::symlink;
    let (root, store) = fixture();
    store.prepare().unwrap();
    let external = root.path().join("outside.json");
    let bytes = json!({"schema_version":1,"selection":{"kind":"local"}}).to_string();
    std::fs::write(&external, &bytes).unwrap();
    symlink(&external, store.desktop_startup_path()).unwrap();
    assert!(store.desktop_startup().is_err());
    assert!(
        store
            .save_desktop_startup(SaveDesktopStartupRequest {
                selection: DesktopStartupSelection::Local {},
                expected_revision: hash(bytes.as_bytes())
            })
            .await
            .is_err()
    );
    assert_eq!(std::fs::read_to_string(external).unwrap(), bytes);
}

#[tokio::test]
async fn unchanged_startup_can_be_saved_while_its_profile_is_leased_by_a_live_connection() {
    let (_root, store) = fixture();
    let lease = store
        .create("company", "https://company.example")
        .await
        .unwrap();
    let profile = lease.profile().await;
    drop(lease);
    let DesktopStartupSaveOutcome::Saved(saved) = store
        .save_desktop_startup(SaveDesktopStartupRequest {
            selection: DesktopStartupSelection::Remote {
                profile_name: profile.name,
                profile_id: profile.id,
            },
            expected_revision: ABSENT.into(),
        })
        .await
        .unwrap()
    else {
        panic!("new selection conflicted");
    };
    let lease = store.open("company").await.unwrap();
    let outcome = store
        .save_desktop_startup(SaveDesktopStartupRequest {
            selection: saved.selection.clone(),
            expected_revision: saved.revision.clone(),
        })
        .await
        .unwrap();
    assert_eq!(outcome, DesktopStartupSaveOutcome::Saved(saved));
    assert_eq!(lease.profile().await.name, "company");
}

#[tokio::test]
async fn caller_cancellation_does_not_release_an_admitted_selection_write() {
    let (_root, store) = fixture();
    let lease = store
        .create("company", "https://company.example")
        .await
        .unwrap();
    let profile = lease.profile().await;
    let selection = DesktopStartupSelection::Remote {
        profile_name: profile.name,
        profile_id: profile.id,
    };
    let worker = store.clone();
    let requested = selection.clone();
    let task = tokio::spawn(async move {
        worker
            .save_desktop_startup(SaveDesktopStartupRequest {
                selection: requested,
                expected_revision: ABSENT.into(),
            })
            .await
    });
    let lock_path = store.root.join(".locks/desktop-startup.lock");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(probe) = File::options().read(true).write(true).open(&lock_path) {
                if matches!(probe.try_lock(), Err(std::fs::TryLockError::WouldBlock)) {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(store.desktop_startup().unwrap().revision, ABSENT);
    drop(lease);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if store.desktop_startup().unwrap().selection == selection {
                let probe = File::options()
                    .read(true)
                    .write(true)
                    .open(&lock_path)
                    .unwrap();
                if probe.try_lock().is_ok() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        store
            .desktop_startup_profile(&store.desktop_startup().unwrap())
            .unwrap()
            .is_some()
    );
}
