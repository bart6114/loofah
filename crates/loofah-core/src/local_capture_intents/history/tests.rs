use super::*;
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

static FAULTS: OnceLock<Mutex<HashMap<PathBuf, String>>> = OnceLock::new();
pub(super) fn checkpoint(path: &Path, stage: &str) -> Result<(), ApplicationError> {
    let block = {
        let mut blocks = BLOCKS.get_or_init(Default::default).lock().unwrap();
        if blocks
            .get(path)
            .is_some_and(|(wanted, _, _)| wanted == stage)
        {
            blocks.remove(path)
        } else {
            None
        }
    };
    if let Some((_, entered, release)) = block {
        let _ = entered.send(());
        release.recv().map_err(review)?;
    }
    let mut faults = FAULTS.get_or_init(Default::default).lock().unwrap();
    if faults.get(path).is_some_and(|wanted| wanted == stage) {
        faults.remove(path);
        return Err(review("Injected Local history interruption"));
    }
    Ok(())
}
pub(super) fn record() -> Record {
    Record {
        version: 2,
        vault_id: "original-vault".into(),
        status: LocalCaptureStatus {
            declaration: LocalCaptureDeclaration {
                request_id: uuid::Uuid::new_v4().to_string(),
                session_id: "original-session".into(),
                append: false,
                batch_requested: false,
                transcription: None,
                summary: None,
            },
            state: LocalCaptureState::NotStarted,
            transcription_owner: TRANSCRIPTION_OWNER.into(),
            summary_owner: SUMMARY_OWNER.into(),
            transcript_id: None,
            summary_job_id: None,
            message: None,
        },
        had_transcript_before: false,
        session_created_at: "2026-11-08T10:00:00Z".into(),
        summary: None,
        physical: None,
        persistence: None,
        source: None,
        inference_dispatched: false,
        result_sha256: None,
        published_revision: None,
        cancel_requested: false,
        processing_drained: false,
        inference_drained: false,
        summary_dispatch_pending: false,
        summary_drained: false,
        resolution: None,
    }
}
fn root() -> tempfile::TempDir {
    tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap()
}

#[test]
fn archive_interruptions_recover_exact_receipt_bytes_and_index_once() {
    for stage in ["intent", "locator", "move", "page", "head"] {
        let root = root();
        let active = root.path().join("local-capture-intents");
        let record = record();
        let request = &record.status.declaration.request_id;
        journal::persist(&active, &record, true).unwrap();
        let raw = fs::read(active.join(request).join("intent.json")).unwrap();
        fs::write(
            active.join(request).join("unknown-private-file"),
            b"preserve unknown bytes",
        )
        .unwrap();
        let history = History::open(&active, "original-vault").unwrap();
        FAULTS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(history.directory.clone(), stage.into());
        assert!(history.archive(&record).is_err());
        let reopened = History::open(&active, "original-vault").unwrap();
        let found = reopened.read(request).unwrap().unwrap();
        assert_eq!(fs::read(found.directory.join("intent.json")).unwrap(), raw);
        assert_eq!(
            fs::read(found.directory.join("unknown-private-file")).unwrap(),
            b"preserve unknown bytes"
        );
        assert!(!active.join(request).exists());
        assert_eq!(reopened.head().unwrap().total, 1);
        assert_eq!(reopened.page(None, None, 10).unwrap().items.len(), 1);
        reopened.archive(&record).unwrap();
        assert_eq!(reopened.head().unwrap().total, 1);
        let mut changed = record.clone();
        changed.status.declaration.append = true;
        assert!(reopened.archive(&changed).is_err());
        assert_eq!(fs::read(found.directory.join("intent.json")).unwrap(), raw);
    }
}

#[test]
fn opening_history_does_not_read_archived_bodies_and_point_reads_validate_them() {
    let root = root();
    let active = root.path().join("local-capture-intents");
    let record = record();
    journal::persist(&active, &record, true).unwrap();
    let history = History::open(&active, "original-vault").unwrap();
    history.archive(&record).unwrap();
    let path = history
        .read(&record.status.declaration.request_id)
        .unwrap()
        .unwrap()
        .directory
        .join("intent.json");
    fs::write(&path, b"malformed archived receipt").unwrap();
    let reopened = History::open(&active, "original-vault").unwrap();
    assert!(
        reopened
            .read(&record.status.declaration.request_id)
            .is_err()
    );
    assert!(reopened.page(None, None, 10).is_err());
    assert!(History::open(&active, "different-vault").is_err());
}

#[test]
fn legacy_and_ambiguous_work_never_acquire_invented_drain_proofs() {
    let mut legacy = record();
    legacy.version = 1;
    assert!(eligible(&legacy));
    legacy.status.state = LocalCaptureState::Succeeded;
    assert!(!eligible(&legacy));
    legacy.version = 2;
    assert!(!eligible(&legacy));
    legacy.processing_drained = true;
    legacy.physical = Some(Ok(()));
    legacy.persistence = Some(Ok(LocalCapturePersistence {
        live_transcription_active: true,
        needs_batch_repair: false,
        new_transcript_id: None,
    }));
    assert!(eligible(&legacy));
    legacy.status.state = LocalCaptureState::Review;
    assert!(!eligible(&legacy));
    legacy.status.state = LocalCaptureState::Conflict;
    assert!(!eligible(&legacy));
    legacy.status.state = LocalCaptureState::Succeeded;
    legacy.inference_dispatched = true;
    assert!(!eligible(&legacy));
    legacy.inference_drained = true;
    legacy.summary_dispatch_pending = true;
    assert!(!eligible(&legacy));
}

#[test]
fn conflicting_archive_destinations_and_missing_move_inputs_are_never_overwritten() {
    for conflict in [true, false] {
        let root = root();
        let active = root.path().join("local-capture-intents");
        let record = record();
        journal::persist(&active, &record, true).unwrap();
        let history = History::open(&active, "original-vault").unwrap();
        FAULTS
            .get_or_init(Default::default)
            .lock()
            .unwrap()
            .insert(history.directory.clone(), "intent".into());
        assert!(history.archive(&record).is_err());
        let target = history
            .receipt_directory(&record.status.declaration.request_id)
            .unwrap();
        if conflict {
            fs::create_dir_all(&target).unwrap();
            fs::write(target.join("foreign"), b"untouched").unwrap();
        } else {
            let original = active.join(&record.status.declaration.request_id);
            fs::rename(original, root.path().join("operator-retained")).unwrap();
        }
        assert!(History::open(&active, "original-vault").is_err());
        if conflict {
            assert_eq!(fs::read(target.join("foreign")).unwrap(), b"untouched");
        } else {
            assert!(root.path().join("operator-retained/intent.json").exists());
        }
    }
}

#[cfg(unix)]
#[test]
fn archive_refuses_symlinks_and_shared_files_without_moving_any_bytes() {
    for symlink in [true, false] {
        let root = root();
        let active = root.path().join("local-capture-intents");
        let record = record();
        let request = &record.status.declaration.request_id;
        journal::persist(&active, &record, true).unwrap();
        fs::write(root.path().join("foreign"), b"outside archive").unwrap();
        let target = active.join(request).join("unknown");
        if symlink {
            std::os::unix::fs::symlink(root.path().join("foreign"), &target).unwrap();
        } else {
            fs::hard_link(root.path().join("foreign"), &target).unwrap();
        }
        let history = History::open(&active, "original-vault").unwrap();
        assert!(history.archive(&record).is_err());
        assert!(active.join(request).join("intent.json").exists());
        assert_eq!(
            fs::read(root.path().join("foreign")).unwrap(),
            b"outside archive"
        );
        assert!(history.read(request).unwrap().is_none());
    }
}

#[test]
fn history_pagination_is_snapshot_bound_and_does_not_adopt_later_archives() {
    let root = root();
    let active = root.path().join("local-capture-intents");
    let history = History::open(&active, "original-vault").unwrap();
    let mut original = Vec::new();
    for _ in 0..5 {
        let record = record();
        journal::persist(&active, &record, true).unwrap();
        history.archive(&record).unwrap();
        original.push(record.status.declaration.request_id);
    }
    let first = history.page(Some("original-session"), None, 2).unwrap();
    assert_eq!(
        first
            .items
            .iter()
            .map(|item| item.status.declaration.request_id.clone())
            .collect::<Vec<_>>(),
        original[3..].iter().rev().cloned().collect::<Vec<_>>()
    );
    let later = record();
    journal::persist(&active, &later, true).unwrap();
    history.archive(&later).unwrap();
    let second = history
        .page(Some("original-session"), first.next_cursor.as_deref(), 100)
        .unwrap();
    assert_eq!(second.items.len(), 3);
    assert!(second.next_cursor.is_none());
    assert!(
        !second
            .items
            .iter()
            .any(|item| item.status.declaration.request_id == later.status.declaration.request_id)
    );
    assert!(
        history
            .page(Some("different-session"), first.next_cursor.as_deref(), 10)
            .is_err()
    );
    assert!(history.page(None, Some("{}"), 10).is_err());
    assert!(history.page(None, None, 101).is_err());
}

#[test]
fn archived_point_reads_reject_valid_json_mutation_and_changed_sequence_locator() {
    let root = root();
    let active = root.path().join("local-capture-intents");
    let history = History::open(&active, "original-vault").unwrap();
    let first = record();
    journal::persist(&active, &first, true).unwrap();
    history.archive(&first).unwrap();
    let second = record();
    journal::persist(&active, &second, true).unwrap();
    history.archive(&second).unwrap();
    let directory = history
        .receipt_directory(&first.status.declaration.request_id)
        .unwrap();
    let original = fs::read(directory.join("intent.json")).unwrap();
    let mut changed = first.clone();
    changed.status.declaration.append = true;
    fs::write(
        directory.join("intent.json"),
        serde_json::to_vec(&changed).unwrap(),
    )
    .unwrap();
    assert!(history.read(&first.status.declaration.request_id).is_err());
    fs::write(directory.join("intent.json"), original).unwrap();
    let mut proof: Pending = read_json(&directory.join(PROOF), 4096).unwrap();
    proof.sequence = 1;
    save(&directory.join(PROOF), &proof).unwrap();
    assert!(history.read(&first.status.declaration.request_id).is_err());
}

#[test]
fn empty_history_reads_do_not_migrate_legacy_private_layout() {
    let root = root();
    let active = root.path().join("local-capture-intents");
    let mut legacy = record();
    legacy.version = 1;
    journal::persist(&active, &legacy, true).unwrap();
    let path = active
        .join(&legacy.status.declaration.request_id)
        .join("intent.json");
    let before = fs::read(&path).unwrap();
    let history = History::open(&active, "original-vault").unwrap();
    assert!(!active.join("history").exists());
    assert!(history.page(None, None, 25).unwrap().items.is_empty());
    assert!(
        history
            .read(&legacy.status.declaration.request_id)
            .unwrap()
            .is_none()
    );
    assert!(!active.join("history").exists());
    assert_eq!(fs::read(path).unwrap(), before);
}

type Block = (
    String,
    tokio::sync::oneshot::Sender<()>,
    std::sync::mpsc::Receiver<()>,
);
static BLOCKS: OnceLock<Mutex<HashMap<PathBuf, Block>>> = OnceLock::new();
pub(super) fn pause(
    path: &Path,
    stage: &str,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    std::sync::mpsc::Sender<()>,
) {
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    BLOCKS
        .get_or_init(Default::default)
        .lock()
        .unwrap()
        .insert(path.into(), (stage.into(), entered, released));
    (waiting, release)
}
