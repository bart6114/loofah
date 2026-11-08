use super::*;
use std::{
    io::Cursor,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

fn wav() -> Vec<u8> {
    let samples = [0.0f32, 0.125, -0.5, 0.75];
    let mut out = b"RIFF".to_vec();
    out.extend_from_slice(&(36 + samples.len() as u32 * 4).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&3u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&16_000u32.to_le_bytes());
    out.extend_from_slice(&64_000u32.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(samples.len() as u32 * 4).to_le_bytes());
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn request(bytes: &[u8]) -> RecordingPublicationRequest {
    RecordingPublicationRequest {
        vault_id: "vault".into(),
        namespace: "member".into(),
        request_id: uuid::Uuid::from_u128(7),
        session_id: "note".into(),
        source_binding: "{\"capture\":\"immutable identity and verified source seal\"}".into(),
        expected: RecordingExpectation::Absent,
        artifact_sha256: crate::sha256(bytes),
        artifact_size_bytes: bytes.len() as u64,
        layout: hypr_vault_read::AudioLayout::Mixed,
        first_audio_observed_at_ms: 1_799_995_200_123,
        capture_ended_at_ms: Some(1_799_995_210_999),
    }
}

struct Fixture {
    root: tempfile::TempDir,
    store: SessionStore,
    operations: PathBuf,
    directory: PathBuf,
    request: RecordingPublicationRequest,
    bytes: Vec<u8>,
}
impl Fixture {
    async fn new() -> Self {
        // Receipt roots require real ancestors; macOS aliases its system temp directory.
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let store = SessionStore::new(root.path().join("vault"));
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Before","created_at":"2026-11-06T10:00:00Z","tags":[],"custom":{"nested":{"keep":true}},"audio":{"custom":{"keep":"audio"}},"tag_suggestions":{"items":[],"dismissed":[],"custom":{"keep":"nested"}}})).unwrap();
        store.create_note_session(&meta, "User note").await.unwrap();
        // The typed creation path predates raw nested-field preservation.
        let directory = root.path().join("vault/sessions/note");
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join("_meta.json")).unwrap()).unwrap();
        raw["tag_suggestions"]["custom"] = serde_json::json!({"keep":"nested"});
        std::fs::write(
            directory.join("_meta.json"),
            serde_json::to_vec_pretty(&raw).unwrap(),
        )
        .unwrap();
        let operations = root.path().join("runtime/operations");
        let bytes = wav();
        let request = request(&bytes);
        Self {
            root,
            store,
            operations,
            directory,
            request,
            bytes,
        }
    }
    async fn prepare(&self) -> RecordingPublicationPlan {
        self.store
            .prepare_recording_publication(
                &self.operations,
                self.request.clone(),
                Cursor::new(self.bytes.clone()),
            )
            .await
            .unwrap()
    }
    async fn status(&self) -> RecordingPublicationPlan {
        self.store
            .prepare_recording_publication_status(&self.operations, self.request.clone())
            .await
            .unwrap()
            .unwrap()
    }
    fn raw(&self) -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(self.directory.join("_meta.json")).unwrap()).unwrap()
    }
}

#[tokio::test]
async fn publishes_exact_bytes_preserves_unknown_fields_and_attachments_and_replays() {
    let f = Fixture::new().await;
    for name in ["photo.wav", "audio-user.mp3", ".user", "notes.md"] {
        std::fs::write(f.directory.join(name), b"Owned by user").unwrap();
    }
    std::fs::create_dir(f.directory.join("attachments")).unwrap();
    std::fs::write(f.directory.join("attachments/image.png"), b"attachment").unwrap();
    std::fs::create_dir(f.directory.join("audio")).unwrap();
    for base in [&f.directory, &f.directory.join("audio")] {
        for name in ["audio.peaks.json", "audio.peaks.json.tmp"] {
            std::fs::write(base.join(name), b"stale").unwrap();
        }
    }
    assert_eq!(
        f.prepare().await.commit().await.unwrap(),
        f.request.published()
    );
    assert_eq!(
        std::fs::read(f.directory.join("audio.wav")).unwrap(),
        f.bytes
    );
    let raw = f.raw();
    assert_eq!(raw["custom"]["nested"]["keep"], true);
    assert_eq!(raw["audio"]["custom"]["keep"], "audio");
    assert_eq!(raw["tag_suggestions"]["custom"]["keep"], "nested");
    assert_eq!(raw["audio"]["source"], "recording");
    assert_eq!(raw["audio"]["layout"], "mixed");
    assert_eq!(
        raw["started_at"],
        io::timestamp(f.request.first_audio_observed_at_ms).unwrap()
    );
    for name in ["photo.wav", "audio-user.mp3", ".user", "notes.md"] {
        assert_eq!(
            std::fs::read(f.directory.join(name)).unwrap(),
            b"Owned by user"
        );
    }
    assert_eq!(
        std::fs::read(f.directory.join("attachments/image.png")).unwrap(),
        b"attachment"
    );
    for base in [&f.directory, &f.directory.join("audio")] {
        assert!(!base.join("audio.peaks.json").exists());
        assert!(!base.join("audio.peaks.json.tmp").exists());
    }
    assert_eq!(
        f.status().await.commit().await.unwrap(),
        f.request.published()
    );
    assert_eq!(f.store.session_list()[0].meta.id, "note");
    let _ = &f.root;
}

#[tokio::test]
async fn ordinary_metadata_edits_after_staging_survive_and_existing_start_is_preserved() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    f.store
        .update_meta(
            "note",
            crate::SessionMetaPatch {
                title: Some("Edited during upload".into()),
                started_at: Some("2020-01-01T00:00:00Z".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(plan.commit().await.unwrap(), f.request.published());
    assert_eq!(f.raw()["title"], "Edited during upload");
    assert_eq!(f.raw()["started_at"], "2020-01-01T00:00:00Z");
}

#[tokio::test]
async fn every_canonical_legacy_recording_and_recording_transient_blocks_absent_publication() {
    for name in [
        "audio.mp3",
        "audio.wav",
        "audio.ogg",
        "audio_mic.wav",
        "audio_spk.wav",
        "audio.mp3.tmp",
        "audio.wav.tmp",
        "audio.ogg.tmp",
    ] {
        for legacy in [false, true] {
            let f = Fixture::new().await;
            let base = if legacy {
                f.directory.join("audio")
            } else {
                f.directory.clone()
            };
            std::fs::create_dir_all(&base).unwrap();
            std::fs::write(base.join(name), b"Baseline").unwrap();
            let before = f.raw();
            assert_eq!(
                f.prepare().await.commit().await.unwrap(),
                RecordingPublication::Changed,
                "{name}/{legacy}"
            );
            assert_eq!(std::fs::read(base.join(name)).unwrap(), b"Baseline");
            assert_eq!(f.raw(), before);
            assert!(!f.operations.exists());
        }
    }
}

#[tokio::test]
async fn changed_request_cannot_replay_or_overwrite_a_publication() {
    let f = Fixture::new().await;
    f.prepare().await.commit().await.unwrap();
    for field in 0..5 {
        let mut other = f.request.clone();
        match field {
            0 => other.source_binding.push('x'),
            1 => other.first_audio_observed_at_ms += 1,
            2 => other.layout = hypr_vault_read::AudioLayout::MicSystem,
            3 => other.artifact_sha256 = "0".repeat(64),
            _ => other.artifact_size_bytes += 1,
        };
        assert!(
            f.store
                .prepare_recording_publication_status(&f.operations, other)
                .await
                .is_err()
        );
    }
    assert_eq!(
        std::fs::read(f.directory.join("audio.wav")).unwrap(),
        f.bytes
    );
}

#[tokio::test]
async fn missing_status_does_not_create_a_session_or_a_receipt() {
    let f = Fixture::new().await;
    assert!(
        f.store
            .prepare_recording_publication_status(&f.operations, f.request.clone())
            .await
            .unwrap()
            .is_none()
    );
    assert!(!f.operations.exists());
    f.store.delete_session("note").await.unwrap();
    assert_eq!(
        f.prepare().await.commit().await.unwrap(),
        RecordingPublication::Changed
    );
    assert!(!f.directory.exists());
}

#[tokio::test]
async fn malformed_lengths_checksums_and_times_never_change_canonical_files() {
    for change in 0..7 {
        let f = Fixture::new().await;
        let mut req = f.request.clone();
        match change {
            0 => req.artifact_sha256 = "0".repeat(64),
            1 => req.artifact_size_bytes -= 1,
            2 => req.artifact_size_bytes += 1,
            3 => req.first_audio_observed_at_ms = 9_007_199_254_740_992,
            4 => req.capture_ended_at_ms = Some(0),
            5 => req.artifact_size_bytes = u64::MAX,
            _ => req.source_binding = "x".repeat(8193),
        };
        assert!(
            f.store
                .prepare_recording_publication(&f.operations, req, Cursor::new(f.bytes.clone()))
                .await
                .is_err()
        );
        assert!(!f.directory.join("audio.wav").exists());
        assert!(!f.operations.exists());
    }
}

#[tokio::test]
async fn active_recording_pending_import_and_late_activity_are_busy_without_mutation() {
    for mode in 0..3 {
        let f = Fixture::new().await;
        let prepared = if mode == 2 {
            Some(f.prepare().await)
        } else {
            None
        };
        if mode == 1 {
            f.store.begin_audio_import("note").await.unwrap();
        } else {
            f.store.note_recording_active("note");
        }
        let plan = match prepared {
            Some(p) => p,
            None => f.prepare().await,
        };
        assert_eq!(plan.commit().await.unwrap(), RecordingPublication::Busy);
        assert!(!f.directory.join("audio.wav").exists());
        assert!(!f.operations.exists());
    }
}

#[tokio::test]
async fn interruptions_at_each_durable_boundary_recover_exactly_and_keep_capture_source() {
    for boundary in 1..=5 {
        let f = Fixture::new().await;
        let source = f.root.path().join("capture-source.wav");
        std::fs::write(&source, &f.bytes).unwrap();
        let plan = f
            .store
            .prepare_recording_publication(
                &f.operations,
                f.request.clone(),
                std::fs::File::open(&source).unwrap(),
            )
            .await
            .unwrap();
        assert!(
            plan.commit_with(move |at| if at == boundary {
                Err(StoreError::Io("injected interruption".into()))
            } else {
                Ok(())
            })
            .await
            .is_err()
        );
        let reopened = SessionStore::new(f.root.path().join("vault"));
        let status = reopened
            .prepare_recording_publication_status(&f.operations, f.request.clone())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            status.commit().await.unwrap(),
            f.request.published(),
            "boundary {boundary}"
        );
        assert_eq!(std::fs::read(&source).unwrap(), f.bytes);
        assert_eq!(
            std::fs::read(f.directory.join("audio.wav")).unwrap(),
            f.bytes
        );
    }
}

#[tokio::test]
async fn pending_intent_never_overwrites_diverged_metadata_or_audio() {
    for audio in [false, true] {
        let f = Fixture::new().await;
        assert!(
            f.prepare()
                .await
                .commit_with(|at| if at == 2 {
                    Err(StoreError::Io("stop after move".into()))
                } else {
                    Ok(())
                })
                .await
                .is_err()
        );
        if audio {
            std::fs::write(f.directory.join("audio.wav"), b"External replacement").unwrap();
        } else {
            let mut raw = f.raw();
            raw["title"] = "Edit after intent".into();
            std::fs::write(
                f.directory.join("_meta.json"),
                serde_json::to_vec(&raw).unwrap(),
            )
            .unwrap();
        }
        let before = f.raw();
        let bytes = std::fs::read(f.directory.join("audio.wav")).unwrap();
        assert_eq!(
            f.status().await.commit().await.unwrap(),
            RecordingPublication::Uncertain
        );
        assert_eq!(f.raw(), before);
        assert_eq!(std::fs::read(f.directory.join("audio.wav")).unwrap(), bytes);
    }
}

#[tokio::test]
async fn external_metadata_edit_after_intent_is_not_overwritten_by_the_same_commit() {
    let f = Fixture::new().await;
    let path = f.directory.join("_meta.json");
    let mut changed = f.raw();
    changed["title"] = "External edit after intent".into();
    changed["custom"]["after_intent"] = serde_json::json!({"preserve":[1,true]});
    let expected = serde_json::to_vec_pretty(&changed).unwrap();
    let bytes = expected.clone();
    assert_eq!(
        f.prepare()
            .await
            .commit_with(move |at| {
                if at == 2 {
                    std::fs::write(&path, &bytes).map_err(io::io_error)?;
                }
                Ok(())
            })
            .await
            .unwrap(),
        RecordingPublication::Uncertain
    );
    assert_eq!(
        std::fs::read(f.directory.join("_meta.json")).unwrap(),
        expected
    );
    assert_eq!(
        std::fs::read(f.directory.join("audio.wav")).unwrap(),
        f.bytes
    );
    assert_eq!(
        f.status().await.commit().await.unwrap(),
        RecordingPublication::Uncertain
    );
    assert_eq!(
        std::fs::read(f.directory.join("_meta.json")).unwrap(),
        expected
    );
}

#[tokio::test]
async fn committed_receipt_does_not_resurrect_deleted_or_changed_audio_or_rewrite_metadata() {
    let f = Fixture::new().await;
    f.prepare().await.commit().await.unwrap();
    let original = f.raw();
    std::fs::remove_file(f.directory.join("audio.wav")).unwrap();
    assert_eq!(
        f.status().await.commit().await.unwrap(),
        RecordingPublication::Uncertain
    );
    assert!(!f.directory.join("audio.wav").exists());
    assert_eq!(f.raw(), original);
}

struct BlockingSource {
    bytes: Cursor<Vec<u8>>,
    entered: Option<tokio::sync::oneshot::Sender<()>>,
    release: std::sync::mpsc::Receiver<()>,
}
impl Read for BlockingSource {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if let Some(entered) = self.entered.take() {
            let _ = entered.send(());
            self.release.recv().unwrap();
        }
        self.bytes.read(out)
    }
}

#[tokio::test]
async fn abandoned_preparation_retains_leases_without_blocking_ordinary_notes() {
    let f = Fixture::new().await;
    let (entered, wait) = tokio::sync::oneshot::channel();
    let (release, blocked) = std::sync::mpsc::channel();
    let store = f.store.clone();
    let operations = f.operations.clone();
    let req = f.request.clone();
    let bytes = f.bytes.clone();
    let caller = tokio::spawn(async move {
        store
            .prepare_recording_publication(
                &operations,
                req,
                BlockingSource {
                    bytes: Cursor::new(bytes),
                    entered: Some(entered),
                    release: blocked,
                },
            )
            .await
    });
    wait.await.unwrap();
    caller.abort();
    assert!(f.store.lock_session_audio("note").await.is_err());
    assert!(f.store.freeze_for_vault_move().await.is_err());
    tokio::time::timeout(
        Duration::from_secs(2),
        f.store.write_note("note", "Typed during hashing"),
    )
    .await
    .unwrap()
    .unwrap();
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.store.lock_session_audio("note").await.is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.prepare().await.commit().await.unwrap(),
        f.request.published()
    );
}

#[tokio::test]
async fn cancelled_commit_owns_finalization_until_receipt_finishes() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (entered, wait) = tokio::sync::oneshot::channel();
    let entered = std::sync::Mutex::new(Some(entered));
    let (release, blocked) = std::sync::mpsc::channel();
    let blocked = std::sync::Mutex::new(blocked);
    let count = Arc::new(AtomicUsize::new(0));
    let completed = count.clone();
    let task = tokio::spawn(plan.commit_with(move |at| {
        if at == 2 {
            entered.lock().unwrap().take().unwrap().send(()).unwrap();
            blocked.lock().unwrap().recv().unwrap();
        }
        if at == 5 {
            completed.fetch_add(1, Ordering::SeqCst);
        }
        Ok(())
    }));
    wait.await.unwrap();
    task.abort();
    assert!(
        f.store
            .audio_operations
            .lock()
            .unwrap()
            .get("note")
            .unwrap()
            .try_lock()
            .is_err()
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while count.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if f.store.lock_session_audio("note").await.is_ok() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        f.status().await.commit().await.unwrap(),
        f.request.published()
    );
}

#[tokio::test]
async fn edits_during_final_checksum_survive_and_owned_field_divergence_remains_uncertain() {
    for owned in [false, true] {
        let f = Fixture::new().await;
        let plan = f.prepare().await;
        let (entered, wait) = tokio::sync::oneshot::channel();
        let entered = std::sync::Mutex::new(Some(entered));
        let (release, blocked) = std::sync::mpsc::channel();
        let blocked = std::sync::Mutex::new(blocked);
        let task = tokio::spawn(plan.commit_with(move |at| {
            if at == 6 {
                entered.lock().unwrap().take().unwrap().send(()).unwrap();
                blocked.lock().unwrap().recv().unwrap();
            }
            Ok(())
        }));
        wait.await.unwrap();
        f.store
            .update_meta(
                "note",
                crate::SessionMetaPatch {
                    title: Some("Edit during verification".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        if owned {
            let mut raw = f.raw();
            raw["audio"]["layout"] = "mic_system".into();
            std::fs::write(
                f.directory.join("_meta.json"),
                serde_json::to_vec(&raw).unwrap(),
            )
            .unwrap();
        }
        release.send(()).unwrap();
        assert_eq!(
            task.await.unwrap().unwrap(),
            if owned {
                RecordingPublication::Uncertain
            } else {
                f.request.published()
            }
        );
        assert_eq!(f.raw()["title"], "Edit during verification");
        assert_eq!(
            f.status().await.commit().await.unwrap(),
            if owned {
                RecordingPublication::Uncertain
            } else {
                f.request.published()
            }
        );
    }
}

#[tokio::test]
async fn metadata_written_recovery_accepts_unrelated_edits_but_does_not_invent_an_end() {
    let mut f = Fixture::new().await;
    f.request.capture_ended_at_ms = None;
    assert!(
        f.prepare()
            .await
            .commit_with(|at| if at == 4 {
                Err(StoreError::Io("lost metadata completion".into()))
            } else {
                Ok(())
            })
            .await
            .is_err()
    );
    f.store
        .update_meta(
            "note",
            crate::SessionMetaPatch {
                title: Some("After metadata".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        f.status().await.commit().await.unwrap(),
        f.request.published()
    );
    assert_eq!(f.raw()["title"], "After metadata");
    assert!(
        f.raw()
            .get("ended_at")
            .is_none_or(serde_json::Value::is_null)
    );
}

struct FailAfterPrefix {
    bytes: Cursor<Vec<u8>>,
    fail: bool,
}
impl Read for FailAfterPrefix {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.fail {
            return Err(std::io::Error::other("injected source interruption"));
        }
        self.fail = true;
        let len = out.len().min(13);
        self.bytes.read(&mut out[..len])
    }
}

#[tokio::test]
async fn interrupted_partial_preparation_restarts_only_from_the_new_full_reader() {
    let f = Fixture::new().await;
    assert!(
        f.store
            .prepare_recording_publication(
                &f.operations,
                f.request.clone(),
                FailAfterPrefix {
                    bytes: Cursor::new(f.bytes.clone()),
                    fail: false
                }
            )
            .await
            .is_err()
    );
    assert!(!f.operations.exists());
    assert!(!f.directory.join("audio.wav").exists());
    assert_eq!(
        f.prepare().await.commit().await.unwrap(),
        f.request.published()
    );
    assert_eq!(
        std::fs::read(f.directory.join("audio.wav")).unwrap(),
        f.bytes
    );
}

#[tokio::test]
async fn metadata_quota_failure_never_creates_a_canonical_artifact_or_intent() {
    let f = Fixture::new().await;
    let limits = crate::ContentLimits {
        metadata_bytes: std::fs::metadata(f.directory.join("_meta.json"))
            .unwrap()
            .len() as usize,
        ..Default::default()
    };
    let store = SessionStore::with_content_limits(f.root.path().join("vault"), limits);
    let plan = store
        .prepare_recording_publication(
            &f.operations,
            f.request.clone(),
            Cursor::new(f.bytes.clone()),
        )
        .await
        .unwrap();
    assert!(matches!(
        plan.commit().await,
        Err(StoreError::ResourceLimit(_))
    ));
    assert!(!f.operations.exists());
    assert!(!f.directory.join("audio.wav").exists());
}

#[test]
fn publication_crash_child() {
    let Some(root) = std::env::var_os("LOOFAH_PUBLICATION_CHILD_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let boundary: u8 = std::env::var("LOOFAH_PUBLICATION_CHILD_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let store = SessionStore::new(root.join("vault"));
        let bytes = wav();
        let plan = store
            .prepare_recording_publication(
                &root.join("runtime/operations"),
                request(&bytes),
                Cursor::new(bytes),
            )
            .await
            .unwrap();
        plan.commit_with(move |at| {
            if at == boundary {
                let path = root.join("child-boundary");
                std::fs::write(&path, [at]).unwrap();
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(path)
                    .unwrap()
                    .sync_all()
                    .unwrap();
                loop {
                    std::thread::park();
                }
            }
            Ok(())
        })
        .await
        .unwrap();
    });
}

#[tokio::test]
async fn killed_subprocess_recovery_reopens_each_intent_and_canonical_boundary() {
    for boundary in 1..=6 {
        let f = Fixture::new().await;
        let root = f.root.path().to_path_buf();
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "recording_publish::tests::publication_crash_child",
                "--nocapture",
            ])
            .env("LOOFAH_PUBLICATION_CHILD_ROOT", &root)
            .env("LOOFAH_PUBLICATION_CHILD_BOUNDARY", boundary.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let ready = tokio::time::timeout(Duration::from_secs(10), async {
            while !root.join("child-boundary").exists() {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!("child exited before boundary {boundary}: {status}");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        child.kill().unwrap();
        child.wait().unwrap();
        ready.unwrap();
        assert_eq!(
            f.status().await.commit().await.unwrap(),
            f.request.published(),
            "boundary {boundary}"
        );
        assert_eq!(
            std::fs::read(f.directory.join("audio.wav")).unwrap(),
            f.bytes
        );
    }
}

#[tokio::test]
async fn replaced_stage_or_late_legacy_candidate_is_never_published() {
    for legacy in [false, true] {
        let f = Fixture::new().await;
        let plan = f.prepare().await;
        let plan = if legacy {
            plan.recheck().await.unwrap()
        } else {
            plan
        };
        if legacy {
            std::fs::create_dir(f.directory.join("audio")).unwrap();
            std::fs::write(f.directory.join("audio/audio.ogg"), b"arrived late").unwrap();
        } else {
            std::fs::write(plan.stage.as_ref().unwrap().path(), b"Changed stage").unwrap();
        }
        assert_eq!(plan.commit().await.unwrap(), RecordingPublication::Changed);
        assert!(!f.directory.join("audio.wav").exists());
    }
}

#[tokio::test]
async fn corrupt_future_duplicate_and_oversized_receipts_fail_closed() {
    for corrupt in 0..4 {
        let f = Fixture::new().await;
        f.prepare().await.commit().await.unwrap();
        let path = io::receipt_path(&f.operations, &f.request);
        let original = std::fs::read_to_string(&path).unwrap();
        let bytes = match corrupt {
            0 => "{".into(),
            1 => original.replace("\"schema_version\":1", "\"schema_version\":99"),
            2 => original.replacen('{', "{\"schema_version\":1,", 1),
            _ => "x".repeat(65537),
        };
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            f.store
                .prepare_recording_publication_status(&f.operations, f.request.clone())
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
        assert_eq!(
            std::fs::read(f.directory.join("audio.wav")).unwrap(),
            f.bytes
        );
    }
}

#[tokio::test]
async fn duplicate_unknown_metadata_keys_are_refused_without_normalization_or_staging() {
    for nested in [false, true] {
        let f = Fixture::new().await;
        let path = f.directory.join("_meta.json");
        let before = std::fs::read_to_string(&path).unwrap();
        let bytes = if nested {
            before.replace(
                "\"nested\": {",
                "\"nested\": {\"duplicate\":1,\"duplicate\":2,",
            )
        } else {
            before.replacen('{', "{\"duplicate\":1,\"duplicate\":2,", 1)
        };
        assert_ne!(bytes, before);
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            f.store
                .prepare_recording_publication(
                    &f.operations,
                    f.request.clone(),
                    Cursor::new(f.bytes.clone())
                )
                .await
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), bytes);
        assert!(!f.operations.exists());
        assert!(!f.directory.join("audio.wav").exists());
    }
}

#[cfg(unix)]
#[tokio::test]
async fn unpublished_staging_has_no_group_or_other_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let stage = plan.stage.as_ref().unwrap().path().to_path_buf();
    assert_eq!(
        std::fs::metadata(stage).unwrap().permissions().mode() & 0o077,
        0
    );
    assert_eq!(plan.commit().await.unwrap(), f.request.published());
}

#[tokio::test]
async fn committed_replay_refreshes_the_index_from_current_file_truth() {
    let f = Fixture::new().await;
    f.prepare().await.commit().await.unwrap();
    let reopened = SessionStore::new(f.root.path().join("vault"));
    assert!(reopened.session_get("note").is_none());
    assert_eq!(
        reopened
            .prepare_recording_publication_status(&f.operations, f.request.clone())
            .await
            .unwrap()
            .unwrap()
            .commit()
            .await
            .unwrap(),
        f.request.published()
    );
    assert_eq!(
        reopened.session_get("note").unwrap().meta.started_at,
        Some(io::timestamp(f.request.first_audio_observed_at_ms).unwrap())
    );
}

#[tokio::test]
async fn unknown_hidden_files_are_never_claimed_even_when_their_bytes_match_the_artifact() {
    for complete in [false, true] {
        let f = Fixture::new().await;
        let receipt = io::receipt_path(&f.operations, &f.request);
        let old_name = format!(".{}.tmp", receipt.file_stem().unwrap().to_str().unwrap());
        let unknown = f.directory.join(old_name);
        let bytes = if complete {
            f.bytes.clone()
        } else {
            b"user".to_vec()
        };
        std::fs::write(&unknown, &bytes).unwrap();
        let nonce = f.directory.join(format!(
            ".recording-publication-{}.tmp",
            uuid::Uuid::from_u128(55)
        ));
        std::fs::write(&nonce, b"user hidden attachment").unwrap();
        assert_eq!(
            f.prepare().await.commit().await.unwrap(),
            f.request.published()
        );
        assert_eq!(std::fs::read(&unknown).unwrap(), bytes);
        assert_eq!(std::fs::read(&nonce).unwrap(), b"user hidden attachment");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_session_stage_receipt_and_peaks_are_never_followed() {
    use std::os::unix::fs::symlink;
    for what in 0..4 {
        let f = Fixture::new().await;
        let outside = f.root.path().join("outside");
        std::fs::write(&outside, b"keep").unwrap();
        if what == 1 {
            let plan = f.prepare().await;
            let stage = plan.stage.as_ref().unwrap().path().to_path_buf();
            std::fs::remove_file(&stage).unwrap();
            symlink(&outside, &stage).unwrap();
            assert!(plan.commit().await.is_err());
            assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
            continue;
        }
        match what {
            0 => {
                let target = f.directory.with_file_name("moved");
                std::fs::rename(&f.directory, &target).unwrap();
                symlink(target, &f.directory).unwrap();
            }
            2 => {
                std::fs::create_dir_all(&f.operations).unwrap();
                symlink(&outside, io::receipt_path(&f.operations, &f.request)).unwrap();
            }
            _ => {
                symlink(&outside, f.directory.join("audio.peaks.json")).unwrap();
            }
        }
        let result = f
            .store
            .prepare_recording_publication(
                &f.operations,
                f.request.clone(),
                Cursor::new(f.bytes.clone()),
            )
            .await;
        match result {
            Err(_) => {}
            Ok(plan) => assert!(plan.commit().await.is_err()),
        };
        assert_eq!(std::fs::read(&outside).unwrap(), b"keep");
    }
}

#[path = "recovery_tests.rs"]
mod recovery;

#[tokio::test]
async fn imported_mp3_preserves_timestamps_transcript_and_recovers_without_reencoding() {
    let mut f = Fixture::new().await;
    f.request.expected = RecordingExpectation::AbsentImport;
    f.request.first_audio_observed_at_ms = 0;
    f.request.capture_ended_at_ms = None;
    let transcript = br#"{"transcripts":[],"future":"preserve"}"#;
    std::fs::write(f.directory.join("transcript.json"), transcript).unwrap();
    let before: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.directory.join("_meta.json")).unwrap()).unwrap();
    let plan = f
        .store
        .prepare_recording_publication(
            &f.operations,
            f.request.clone(),
            Cursor::new(f.bytes.clone()),
        )
        .await
        .unwrap();
    assert!(
        plan.commit_with(|step| if step == 2 {
            Err(StoreError::Io("interrupted".into()))
        } else {
            Ok(())
        })
        .await
        .is_err()
    );
    let status = f
        .store
        .prepare_recording_publication_status(&f.operations, f.request.clone())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(status.commit().await.unwrap(), RecordingPublication::Published { filename, .. } if filename == "audio.mp3")
    );
    let after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.directory.join("_meta.json")).unwrap()).unwrap();
    assert_eq!(before["started_at"], after["started_at"]);
    assert_eq!(before["ended_at"], after["ended_at"]);
    assert_eq!(after["audio"]["source"], "import");
    assert_eq!(after["audio"]["layout"], "mixed");
    assert_eq!(
        std::fs::read(f.directory.join("transcript.json")).unwrap(),
        transcript
    );
    assert_eq!(
        std::fs::read(f.directory.join("audio.mp3")).unwrap(),
        f.bytes
    );
    assert!(!f.directory.join("audio.wav").exists());
}

#[test]
fn import_length_is_not_limited_by_classic_wav() {
    let mut input = request(b"mp3");
    input.artifact_size_bytes = u32::MAX as u64 + 1;
    assert!(input.validate().is_err());
    input.expected = RecordingExpectation::AbsentImport;
    input.first_audio_observed_at_ms = 0;
    input.capture_ended_at_ms = None;
    input.validate().unwrap();
}
