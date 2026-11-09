use super::*;
use std::{
    fs::File,
    io::Read,
    path::PathBuf,
    sync::{Condvar, Mutex},
};

#[cfg(unix)]
use std::io::Write;

type Gate = Arc<(Mutex<bool>, Condvar)>;

struct Release(Gate);

impl Release {
    fn release(&self) {
        *self.0.0.lock().unwrap() = true;
        self.0.1.notify_all();
    }
}
impl Drop for Release {
    fn drop(&mut self) {
        self.release();
    }
}

fn gate() -> (Release, Gate) {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    (Release(gate.clone()), gate)
}

fn wait(gate: &Gate) {
    let mut released = gate.0.lock().unwrap();
    while !*released {
        released = gate.1.wait(released).unwrap();
    }
}

async fn seed(root: &std::path::Path) {
    let store = SessionStore::new(root.join("vault"));
    let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"Preserve title","created_at":"2026-11-08T10:00:00Z","tags":[]})).unwrap();
    store
        .create_note_session(&meta, "Preserve the user's note.")
        .await
        .unwrap();
}

async fn open(root: &std::path::Path) -> VaultRuntime {
    VaultRuntime::open_with_changes(
        "vault".into(),
        &root.join("vault"),
        &root.join("cache"),
        &root.join("operations"),
        tokio::sync::mpsc::unbounded_channel().1,
    )
    .await
    .unwrap()
}

fn unlocked(root: &std::path::Path) -> bool {
    let file = File::options()
        .read(true)
        .write(true)
        .open(root.join("vault/.loofah-vault.lock"))
        .unwrap();
    file.try_lock().is_ok()
}

async fn until_unlocked(root: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while !unlocked(root) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

fn blocked_reader(
    path: PathBuf,
) -> (
    tokio::task::JoinHandle<()>,
    Release,
    tokio::sync::oneshot::Receiver<()>,
) {
    let (release, gate) = gate();
    let (started, received) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        tokio::task::spawn_blocking(move || {
            let mut file = File::open(path).unwrap();
            let mut first = [0];
            file.read_exact(&mut first).unwrap();
            let _ = started.send(());
            wait(&gate);
            let mut rest = Vec::new();
            file.read_to_end(&mut rest).unwrap();
        })
        .await
        .unwrap();
    });
    (task, release, received)
}

#[tokio::test]
async fn dropping_runtime_drains_started_blocking_io_before_releasing_the_real_file_lease() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path()).await;
    let mut runtime = open(root.path()).await;
    let startup = runtime.context.startup.clone();
    let weak = Arc::downgrade(&runtime.context);
    let (reader, release, started) =
        blocked_reader(root.path().join("vault/sessions/note/notes.md"));
    runtime.workers.push(reader);
    tokio::time::timeout(Duration::from_secs(5), started)
        .await
        .unwrap()
        .unwrap();
    drop(runtime);
    assert!(!unlocked(root.path()));
    assert!(weak.upgrade().is_some());
    assert_eq!(startup.snapshot().phase, StartupPhase::Stopping);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), startup.wait_until_stopped())
            .await
            .is_err()
    );
    release.release();
    tokio::time::timeout(Duration::from_secs(10), startup.wait_until_stopped())
        .await
        .unwrap()
        .unwrap();
    until_unlocked(root.path()).await;
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn deadline_reports_pending_work_without_aborting_actual_io_or_claiming_stopped() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path()).await;
    let mut runtime = open(root.path()).await;
    let startup = runtime.context.startup.clone();
    let (reader, release, started) =
        blocked_reader(root.path().join("vault/sessions/note/notes.md"));
    runtime.workers.push(reader);
    started.await.unwrap();
    let outcome = runtime
        .shutdown_with_deadline(Duration::from_millis(20))
        .await;
    assert!(
        matches!(outcome, Err(StoreError::Io(ref message)) if message.ends_with("shutdown deadline"))
    );
    assert_eq!(startup.snapshot().phase, StartupPhase::Stopping);
    assert!(!unlocked(root.path()));
    release.release();
    tokio::time::timeout(Duration::from_secs(10), startup.wait_until_stopped())
        .await
        .unwrap()
        .unwrap();
    until_unlocked(root.path()).await;
}

#[tokio::test]
async fn cancelling_shutdown_waiter_keeps_the_owned_drain_and_lease_alive() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path()).await;
    let mut runtime = open(root.path()).await;
    let startup = runtime.context.startup.clone();
    let (reader, release, started) =
        blocked_reader(root.path().join("vault/sessions/note/notes.md"));
    runtime.workers.push(reader);
    started.await.unwrap();
    let waiter = tokio::spawn(runtime.shutdown());
    tokio::time::timeout(Duration::from_secs(5), async {
        while startup.snapshot().phase != StartupPhase::Stopping {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    waiter.abort();
    assert!(waiter.await.unwrap_err().is_cancelled());
    assert!(!unlocked(root.path()));
    release.release();
    tokio::time::timeout(Duration::from_secs(10), startup.wait_until_stopped())
        .await
        .unwrap()
        .unwrap();
    until_unlocked(root.path()).await;
}

#[tokio::test]
async fn failed_transcript_flush_keeps_words_and_ownership_until_a_real_retry_succeeds() {
    for drop_owner in [false, true] {
        let root = tempfile::tempdir().unwrap();
        seed(root.path()).await;
        let runtime = open(root.path()).await;
        let startup = runtime.context.startup.clone();
        let weak = Arc::downgrade(&runtime.context);
        let transcript = root.path().join("vault/sessions/note/transcript.json");
        std::fs::create_dir(&transcript).unwrap();
        let delta = serde_json::from_value(serde_json::json!({"transcript_id":"buffered","started_at_ms":1000,"new_words":[{"id":"word","text":"Retain these words","start_ms":0,"end_ms":100,"channel":0}],"replaced_ids":[],"new_hints":[]})).unwrap();
        runtime
            .context
            .store
            .append_transcript("note", delta)
            .await
            .unwrap();
        assert!(runtime.context.store.flush_all().await.is_err());
        if drop_owner {
            drop(runtime);
        } else {
            assert!(
                matches!(runtime.shutdown_with_deadline(Duration::from_millis(20)).await, Err(StoreError::Io(ref message)) if message == "vault flush exceeded shutdown deadline")
            );
        }
        assert_eq!(startup.snapshot().phase, StartupPhase::Stopping);
        assert!(weak.upgrade().is_some());
        assert!(!unlocked(root.path()));
        assert!(transcript.is_dir());
        std::fs::remove_dir(&transcript).unwrap();
        tokio::time::timeout(Duration::from_secs(10), startup.wait_until_stopped())
            .await
            .unwrap()
            .unwrap();
        until_unlocked(root.path()).await;
        let raw = std::fs::read_to_string(transcript).unwrap();
        let file: hypr_vault_read::TranscriptJson = serde_json::from_str(&raw).unwrap();
        assert_eq!(file.transcripts.len(), 1);
        assert_eq!(file.transcripts[0].words.len(), 1);
        assert_eq!(file.transcripts[0].words[0].text, "Retain these words");
        assert!(weak.upgrade().is_none());
    }
}

#[cfg(unix)]
struct PipeRead {
    release: Release,
    started: tokio::sync::oneshot::Receiver<()>,
    completed: tokio::sync::oneshot::Receiver<()>,
    writer: std::thread::JoinHandle<()>,
}

#[cfg(unix)]
fn blocked_transcript(transcript: PathBuf) -> PipeRead {
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&transcript)
            .status()
            .unwrap()
            .success()
    );
    let (release, gate) = gate();
    let (entered, started) = tokio::sync::oneshot::channel();
    let (finished, completed) = tokio::sync::oneshot::channel();
    let writer = std::thread::spawn(move || {
        let bytes = br#"{"transcripts":[{"id":"transcript","session_id":"note","started_at":1000,"words":[{"id":"word","text":"After blocked read","start_ms":0,"end_ms":100,"channel":0}]}]}"#;
        let mut pipe = File::options().write(true).open(&transcript).unwrap();
        pipe.write_all(&bytes[..1]).unwrap();
        let _ = entered.send(());
        wait(&gate);
        // The in-progress read owns the pipe descriptor; subsequent startup/search
        // reads must see a regular file rather than wait for another writer.
        std::fs::remove_file(&transcript).unwrap();
        std::fs::write(&transcript, bytes).unwrap();
        pipe.write_all(&bytes[1..]).unwrap();
        drop(pipe);
        let _ = finished.send(());
    });
    PipeRead {
        release,
        started,
        completed,
        writer,
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cancelled_cold_open_retains_the_lease_until_actual_canonical_startup_io_finishes() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path()).await;
    let PipeRead {
        release,
        started,
        completed,
        writer,
    } = blocked_transcript(root.path().join("vault/sessions/note/transcript.json"));
    let path = root.path().to_path_buf();
    let waiter = tokio::spawn(async move { open(&path).await });
    tokio::time::timeout(Duration::from_secs(10), started)
        .await
        .unwrap()
        .unwrap();
    assert!(!unlocked(root.path()));
    waiter.abort();
    let Err(cancelled) = waiter.await else {
        panic!("cold open completed before cancellation")
    };
    assert!(cancelled.is_cancelled());
    assert!(!unlocked(root.path()));
    release.release();
    tokio::time::timeout(Duration::from_secs(10), completed)
        .await
        .unwrap()
        .unwrap();
    writer.join().unwrap();
    until_unlocked(root.path()).await;
    assert_eq!(
        std::fs::read_to_string(root.path().join("vault/sessions/note/notes.md")).unwrap(),
        "Preserve the user's note."
    );
    assert!(root.path().join("vault/AGENTS.md").is_file());
}

#[cfg(unix)]
#[tokio::test]
async fn stop_aware_watcher_finishes_actual_active_store_io_before_returning() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path()).await;
    let PipeRead {
        release,
        started,
        completed,
        writer,
    } = blocked_transcript(root.path().join("vault/sessions/note/transcript.json"));
    let store = Arc::new(SessionStore::new(root.path().join("vault")));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let stop = CancellationToken::new();
    let task = tokio::spawn(crate::vault_watch::run_source_until_stopped(
        store.clone(),
        crate::vault_watch::ChangeSource::Unbounded(rx),
        stop.clone(),
    ));
    tx.send("sessions/note".into()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), started)
        .await
        .unwrap()
        .unwrap();
    stop.cancel();
    assert!(!task.is_finished());
    release.release();
    tokio::time::timeout(Duration::from_secs(10), completed)
        .await
        .unwrap()
        .unwrap();
    writer.join().unwrap();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap();
    assert!(store.session_has_transcript("note"));
}

#[cfg(unix)]
#[tokio::test]
async fn trusted_operation_root_alias_resolves_once_but_private_descendant_symlinks_still_gate_recordings()
 {
    use std::os::unix::fs::symlink;
    for unsafe_child in [false, true] {
        let root = tempfile::tempdir().unwrap();
        seed(root.path()).await;
        std::fs::write(
            root.path().join("vault/sessions/note/audio.wav"),
            b"Recording bytes",
        )
        .unwrap();
        let trusted = root.path().join("trusted-operations");
        std::fs::create_dir(&trusted).unwrap();
        let alias = root.path().join("operation-alias");
        symlink(&trusted, &alias).unwrap();
        if unsafe_child {
            let outside = root.path().join("outside");
            std::fs::create_dir(&outside).unwrap();
            symlink(&outside, trusted.join("recording-publications")).unwrap();
        }
        let runtime = VaultRuntime::open_with_changes(
            "vault".into(),
            &root.path().join("vault"),
            &root.path().join("cache"),
            &alias,
            tokio::sync::mpsc::unbounded_channel().1,
        )
        .await
        .unwrap();
        let recording = runtime.context.audio_metadata("note").await;
        if unsafe_child {
            assert!(
                matches!(recording, Err(ref cause) if cause.code == hypr_loofah_protocol::ErrorCode::Unavailable)
            );
            assert_eq!(
                std::fs::read_dir(root.path().join("outside"))
                    .unwrap()
                    .count(),
                0
            );
        } else {
            assert!(recording.unwrap().is_some());
        }
        runtime.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn acquired_lease_runtime_reuses_original_descriptor_and_preserves_local_content_limits() {
    let root = tempfile::tempdir().unwrap();
    seed(root.path()).await;
    let lease =
        match hypr_storage::vault::VaultLease::try_acquire_existing(&root.path().join("vault"))
            .unwrap()
        {
            hypr_storage::vault::LeaseAttempt::Acquired(lease) => lease,
            _ => panic!("unexpected owner"),
        };
    let original = lease.pin();
    let weak = Arc::downgrade(&original);
    let runtime = VaultRuntime::open_leased(
        "vault".into(),
        lease,
        &root.path().join("cache"),
        &root.path().join("operations"),
        hypr_vault_write::ContentLimits::unlimited(),
    )
    .await
    .unwrap();
    assert!(Arc::ptr_eq(
        &runtime.context.lease_pin().unwrap(),
        &original
    ));
    let mut note = runtime.context.read_note("note").await.unwrap();
    let large = "x".repeat(hypr_vault_write::ContentLimits::default().note_write_bytes + 1);
    let saved = runtime
        .context
        .save_note(
            "note",
            hypr_loofah_protocol::SaveNoteRequest {
                markdown: large.clone(),
                title: None,
                expected_revision: std::mem::take(&mut note.revision),
            },
        )
        .await
        .unwrap();
    assert!(
        matches!(saved,hypr_loofah_protocol::NoteSaveOutcome::Saved(snapshot) if snapshot.markdown.as_deref()==Some(large.as_str()))
    );
    drop(original);
    runtime
        .shutdown_with_deadline(Duration::from_secs(10))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while weak.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
