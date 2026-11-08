use super::*;
use std::{io::Read, time::Duration};

struct Fixture {
    root: tempfile::TempDir,
    store: SessionStore,
    operations: PathBuf,
    directory: PathBuf,
    identity: TranscriptPublicationIdentity,
}

fn generated() -> TranscriptWithData {
    serde_json::from_value(serde_json::json!({
        "id":"generated-transcript", "user_id":"owner", "created_at":"2026-11-08T10:00:00Z",
        "session_id":"note", "started_at":1000.25, "ended_at":2345.75, "memo_md":"{\"snapshot\":true}",
        "words":[
            {"id":"word-mic","text":" Hello","start_ms":0,"end_ms":400,"channel":0,"speaker":"person-a","metadata":{"capture_source":"recording","timing":{"source":"provider_word"},"future":{"nested":[1,true]}}},
            {"id":"word-system","text":" world.","start_ms":250,"end_ms":500.5,"channel":1,"metadata":{"timing":{"source":"synthetic_speech"}}},
            {"id":"word-mixed","text":" mixed","start_ms":1000,"end_ms":1250,"channel":2}
        ],
        "speaker_hints":[
            {"id":"hint-provider","word_id":"word-mic","type":"provider_speaker_index","value":"{\"provider\":\"whispercpp\",\"channel\":0,\"speaker_index\":1}"},
            {"id":"hint-label","word_id":"word-mic","type":"speaker_label","value":"Alice"},
            {"id":"hint-unknown","word_id":"word-mixed","type":"future_annotation","value":{"future":[true,42]}}
        ]
    })).unwrap()
}

impl Fixture {
    async fn new() -> Self {
        // Receipt roots require real ancestors; macOS aliases its system temp directory.
        let root = tempfile::tempdir_in(std::env::temp_dir().canonicalize().unwrap()).unwrap();
        let store = SessionStore::new(root.path().join("vault"));
        let meta = serde_json::from_value(serde_json::json!({"id":"note","title":"User title","created_at":"2026-11-08T10:00:00Z","tags":["preserve"],"audio":{"source":"recording","layout":"mic_system"},"future":{"nested":[true,42]}})).unwrap();
        store
            .create_note_session(&meta, "User notes remain unchanged")
            .await
            .unwrap();
        let directory = root.path().join("vault/sessions/note");
        std::fs::write(directory.join("audio.wav"), b"Canonical recording source").unwrap();
        std::fs::write(directory.join("unknown.user-file"), b"User attachment").unwrap();
        let operations = root.path().join("operations/transcript-publications");
        let identity = TranscriptPublicationIdentity {
            vault_id: "vault".into(),
            namespace: "member".into(),
            request_id: uuid::Uuid::parse_str("98b2c016-e063-49a7-a9fb-34dd105cbb6e").unwrap(),
        };
        Self {
            root,
            store,
            operations,
            directory,
            identity,
        }
    }

    async fn source(&self) -> Box<TranscriptSourceSnapshot> {
        let Some(TranscriptSourcePreparation::Ready(source)) =
            self.store.prepare_transcript_source("note").await.unwrap()
        else {
            panic!("source not ready")
        };
        source
    }

    async fn prepare(&self) -> TranscriptPublicationPlan {
        self.store
            .prepare_transcript_publication(
                &self.operations,
                self.identity.clone(),
                self.source().await.baseline(),
                generated(),
            )
            .await
            .unwrap()
    }

    async fn status(&self) -> TranscriptPublicationPlan {
        self.store
            .prepare_transcript_publication_status(&self.operations, self.identity.clone())
            .await
            .unwrap()
            .unwrap()
    }

    fn canonical(&self) -> PathBuf {
        self.directory.join("transcript.json")
    }
    fn paths(&self) -> (PathBuf, PathBuf) {
        io::paths(&self.operations, &self.identity)
    }
}

#[tokio::test]
async fn publication_preserves_full_metadata_timings_channels_hints_and_unrelated_files() {
    let f = Fixture::new().await;
    let before_meta = std::fs::read(f.directory.join("_meta.json")).unwrap();
    let mut changes = f.store.subscribe_index_changes();
    let expected = generated();
    let result = f
        .prepare()
        .await
        .recheck()
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    let TranscriptPublicationOutcome::Published {
        revision,
        transcript_id,
    } = result
    else {
        panic!("not published")
    };
    assert_eq!(transcript_id, expected.id);
    let bytes = std::fs::read(f.canonical()).unwrap();
    assert_eq!(revision, crate::sha256(&bytes));
    let file: TranscriptJson = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(file.transcripts, [expected]);
    assert_eq!(std::fs::read(f.paths().1).unwrap(), bytes);
    assert_eq!(
        std::fs::read(f.directory.join("_meta.json")).unwrap(),
        before_meta
    );
    assert_eq!(
        std::fs::read_to_string(f.directory.join("notes.md")).unwrap(),
        "User notes remain unchanged"
    );
    assert_eq!(
        std::fs::read_to_string(f.directory.join("unknown.user-file")).unwrap(),
        "User attachment"
    );
    assert!(f.store.session_has_transcript("note"));
    assert_eq!(
        f.store.session_transcript_metadata("note").speaker_labels,
        ["Alice"]
    );
    let mut saw_transcripts = false;
    while let Ok(change) = changes.try_recv() {
        saw_transcripts |=
            change.entity == crate::IndexEntity::Transcripts && change.ids == ["note"];
    }
    assert!(saw_transcripts);
}

#[tokio::test]
async fn source_snapshots_pin_exact_selection_without_holding_operation_mutexes() {
    let f = Fixture::new().await;
    let source = f.source().await;
    assert_eq!(source.recording_selection(), "audio.wav");
    assert_eq!(
        source.audio().layout,
        hypr_vault_read::AudioLayout::MicSystem
    );
    assert_eq!(source.transcript_revision(), io::absent_revision());
    let mut bytes = Vec::new();
    source.recording_file().read_to_end(&mut bytes).unwrap();
    assert_eq!(crate::sha256(&bytes), source.recording_sha256());
    drop(f.store.lock_session_audio("note").await.unwrap());
    drop(f.store.transcript_operations.lock("note").await);
    let guard = tokio::time::timeout(Duration::from_secs(1), f.store.lock_snapshot())
        .await
        .unwrap();
    drop(guard);
    let before = std::fs::read(f.directory.join("_meta.json")).unwrap();
    std::fs::create_dir(f.directory.join("audio")).unwrap();
    std::fs::rename(
        f.directory.join("audio.wav"),
        f.directory.join("audio/audio.ogg"),
    )
    .unwrap();
    let legacy = f.source().await;
    assert_eq!(legacy.recording_selection(), "audio/audio.ogg");
    assert_eq!(
        std::fs::read(f.directory.join("_meta.json")).unwrap(),
        before
    );
}

#[tokio::test]
async fn every_existing_transcript_shape_conflicts_without_reading_or_rewriting_it() {
    for bytes in [
        b"".as_slice(),
        b"not JSON",
        b"{\"transcripts\":[]}",
        b"{\"transcripts\":null,\"future\":{\"preserve\":true}}",
    ] {
        let f = Fixture::new().await;
        std::fs::write(f.canonical(), bytes).unwrap();
        assert!(matches!(
            f.store.prepare_transcript_source("note").await.unwrap(),
            Some(TranscriptSourcePreparation::Conflict)
        ));
        assert_eq!(std::fs::read(f.canonical()).unwrap(), bytes);
        assert!(!f.operations.exists());
    }
    let f = Fixture::new().await;
    std::fs::create_dir(f.canonical()).unwrap();
    assert!(matches!(
        f.store.prepare_transcript_source("note").await.unwrap(),
        Some(TranscriptSourcePreparation::Conflict)
    ));
    assert!(f.canonical().is_dir());
}

#[cfg(unix)]
#[tokio::test]
async fn unsafe_transcript_and_recording_paths_preserve_the_external_target() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new().await;
    let outside = f.root.path().join("external");
    std::fs::write(&outside, b"External bytes").unwrap();
    symlink(&outside, f.canonical()).unwrap();
    assert!(matches!(
        f.store.prepare_transcript_source("note").await.unwrap(),
        Some(TranscriptSourcePreparation::Conflict)
    ));
    std::fs::remove_file(f.canonical()).unwrap();
    std::fs::remove_file(f.directory.join("audio.wav")).unwrap();
    symlink(&outside, f.directory.join("audio.wav")).unwrap();
    assert!(f.store.prepare_transcript_source("note").await.is_err());
    assert_eq!(std::fs::read(outside).unwrap(), b"External bytes");
}

#[tokio::test]
async fn a_concurrent_transcript_and_all_its_unknown_fields_survive_with_generated_result_retained()
{
    let f = Fixture::new().await;
    let plan = f.prepare().await.recheck().await.unwrap();
    let external = br#"{"future":{"nested":true},"transcripts":[{"id":"existing","session_id":"note","started_at":1,"unknown":{"nested":[42]},"words":[{"id":"user-word","text":"User","start_ms":0,"end_ms":1,"channel":0,"future":"keep"}],"speaker_hints":[{"word_id":"user-word","type":"speaker_label","value":"User label","future":true}]}]}"#;
    std::fs::write(f.canonical(), external).unwrap();
    assert_eq!(
        plan.commit().await.unwrap(),
        TranscriptPublicationOutcome::Conflict
    );
    assert_eq!(std::fs::read(f.canonical()).unwrap(), external);
    assert!(f.paths().1.is_file());
    std::fs::remove_file(f.canonical()).unwrap();
    assert_eq!(
        f.status().await.commit().await.unwrap(),
        TranscriptPublicationOutcome::Conflict
    );
    assert!(!f.canonical().exists());
}

#[tokio::test]
async fn source_and_layout_changes_conflict_but_unrelated_metadata_edits_do_not() {
    for after_recheck in [false, true] {
        let f = Fixture::new().await;
        let mut plan = f.prepare().await;
        if after_recheck {
            plan = plan.recheck().await.unwrap();
        }
        hypr_storage::fs::atomic_write(&f.directory.join("audio.wav"), "New canonical source")
            .unwrap();
        assert_eq!(
            plan.commit().await.unwrap(),
            TranscriptPublicationOutcome::Conflict
        );
        assert!(!f.canonical().exists());
        assert!(f.paths().1.exists());
    }
    let f = Fixture::new().await;
    let plan = f.prepare().await.recheck().await.unwrap();
    let mut meta = f.store.read_meta("note").await.unwrap().unwrap();
    meta.extra.insert(
        "audio".into(),
        serde_json::json!({"source":"import","layout":"mixed"}),
    );
    f.store.write_meta(&meta).await.unwrap();
    assert_eq!(
        plan.commit().await.unwrap(),
        TranscriptPublicationOutcome::Conflict
    );
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let mut meta = f.store.read_meta("note").await.unwrap().unwrap();
    meta.title = "New user title".into();
    f.store.write_meta(&meta).await.unwrap();
    let before = std::fs::read(f.directory.join("_meta.json")).unwrap();
    assert!(matches!(
        plan.commit().await.unwrap(),
        TranscriptPublicationOutcome::Published { .. }
    ));
    assert_eq!(
        std::fs::read(f.directory.join("_meta.json")).unwrap(),
        before
    );
}

#[tokio::test]
async fn empty_speech_still_requires_unchanged_audio_and_absent_transcript() {
    for changed in ["none", "audio", "transcript"] {
        let f = Fixture::new().await;
        let mut candidate = generated();
        candidate.words.clear();
        candidate.speaker_hints.clear();
        let plan = f
            .store
            .prepare_transcript_publication(
                &f.operations,
                f.identity.clone(),
                f.source().await.baseline(),
                candidate,
            )
            .await
            .unwrap()
            .recheck()
            .await
            .unwrap();
        if changed == "audio" {
            hypr_storage::fs::atomic_write(&f.directory.join("audio.wav"), "Replacement").unwrap();
        }
        if changed == "transcript" {
            std::fs::write(f.canonical(), b"User empty-file canary").unwrap();
        }
        let outcome = plan.commit().await.unwrap();
        assert_eq!(
            outcome,
            if changed == "none" {
                TranscriptPublicationOutcome::Empty
            } else {
                TranscriptPublicationOutcome::Conflict
            }
        );
        if changed != "transcript" {
            assert!(!f.canonical().exists());
        }
        assert!(f.paths().1.exists());
    }
}

#[tokio::test]
async fn live_buffers_recording_and_import_reservations_refuse_without_discarding_data() {
    for kind in ["live", "recording", "import"] {
        let f = Fixture::new().await;
        let plan = f.prepare().await.recheck().await.unwrap();
        match kind {
            "live" => {
                f.store.live.lock().await.insert(
                    "note".into(),
                    crate::transcript::LiveTranscriptBuffer {
                        transcript_id: "live".into(),
                        words: generated().words,
                        dirty: true,
                        ..Default::default()
                    },
                );
            }
            "recording" => f.store.note_recording_active("note"),
            _ => {
                let mut meta = f.store.read_meta("note").await.unwrap().unwrap();
                meta.extra
                    .insert("audio_import_pending".into(), true.into());
                f.store.write_meta(&meta).await.unwrap();
            }
        }
        assert_eq!(
            plan.commit().await.unwrap(),
            TranscriptPublicationOutcome::Busy
        );
        assert!(matches!(
            f.store.prepare_transcript_source("note").await.unwrap(),
            Some(TranscriptSourcePreparation::Busy)
        ));
        assert!(!f.canonical().exists());
        if kind == "live" {
            assert!(f.store.live.lock().await["note"].dirty);
        }
    }
}

#[tokio::test]
async fn opaque_baselines_and_request_identity_cannot_be_redirected_or_redefined() {
    let f = Fixture::new().await;
    let source = f.source().await;
    let reopened = SessionStore::new(f.root.path().join("vault"));
    assert!(
        reopened
            .prepare_transcript_publication(
                &f.operations,
                f.identity.clone(),
                source.baseline(),
                generated()
            )
            .await
            .is_err()
    );
    let mut candidate = generated();
    candidate.session_id = "other".into();
    assert!(
        f.store
            .prepare_transcript_publication(
                &f.operations,
                f.identity.clone(),
                source.baseline(),
                candidate
            )
            .await
            .is_err()
    );
    drop(f.prepare().await);
    let mut candidate = generated();
    candidate.memo_md = "Changed result".into();
    assert!(
        f.store
            .prepare_transcript_publication(
                &f.operations,
                f.identity.clone(),
                source.baseline(),
                candidate
            )
            .await
            .is_err()
    );
    assert!(!f.canonical().exists());
}

#[tokio::test]
async fn completed_receipts_are_historical_and_never_touch_deleted_or_replaced_canonical_files() {
    for replacement in [false, true] {
        let f = Fixture::new().await;
        let published = f.prepare().await.commit().await.unwrap();
        let TranscriptPublicationOutcome::Published {
            revision,
            transcript_id,
        } = published
        else {
            panic!("not published")
        };
        std::fs::remove_file(f.canonical()).unwrap();
        if replacement {
            std::fs::write(f.canonical(), b"New user transcript").unwrap();
        }
        std::fs::remove_file(f.paths().1).unwrap();
        assert_eq!(
            f.status().await.commit().await.unwrap(),
            TranscriptPublicationOutcome::AlreadyCommitted {
                revision,
                transcript_id
            }
        );
        if replacement {
            assert_eq!(
                std::fs::read(f.canonical()).unwrap(),
                b"New user transcript"
            );
        } else {
            assert!(!f.canonical().exists());
        }
    }
    let f = Fixture::new().await;
    f.prepare().await.commit().await.unwrap();
    std::fs::remove_dir_all(f.root.path().join("vault")).unwrap();
    assert!(matches!(
        f.status().await.commit().await.unwrap(),
        TranscriptPublicationOutcome::AlreadyCommitted { .. }
    ));
    assert!(!f.root.path().join("vault").exists());
}

#[tokio::test]
async fn exact_result_recovers_each_fault_boundary_without_another_generation() {
    for boundary in 0..=3 {
        let f = Fixture::new().await;
        assert!(
            f.prepare()
                .await
                .commit_with(move |at| if at == boundary {
                    Err(StoreError::Io("Injected crash".into()))
                } else {
                    Ok(())
                })
                .await
                .is_err()
        );
        let result = std::fs::read(f.paths().1).unwrap();
        let reopened = SessionStore::new(f.root.path().join("vault"));
        let outcome = reopened
            .prepare_transcript_publication_status(&f.operations, f.identity.clone())
            .await
            .unwrap()
            .unwrap()
            .recheck()
            .await
            .unwrap()
            .commit()
            .await
            .unwrap();
        assert!(
            matches!(
                outcome,
                TranscriptPublicationOutcome::Published { .. }
                    | TranscriptPublicationOutcome::AlreadyCommitted { .. }
            ),
            "boundary {boundary}: {outcome:?}"
        );
        assert_eq!(std::fs::read(f.canonical()).unwrap(), result);
        assert_eq!(std::fs::read(f.paths().1).unwrap(), result);
    }
}

#[tokio::test]
async fn missing_moved_stage_or_divergent_target_never_resurrects_or_overwrites_content() {
    for replacement in [false, true] {
        let f = Fixture::new().await;
        assert!(
            f.prepare()
                .await
                .commit_with(|at| if at == 2 {
                    Err(StoreError::Io("after rename".into()))
                } else {
                    Ok(())
                })
                .await
                .is_err()
        );
        std::fs::remove_file(f.canonical()).unwrap();
        if replacement {
            std::fs::write(f.canonical(), b"New user file").unwrap();
        }
        assert_eq!(
            f.status().await.commit().await.unwrap(),
            TranscriptPublicationOutcome::RequiresReview
        );
        assert!(f.paths().1.exists());
        if replacement {
            assert_eq!(std::fs::read(f.canonical()).unwrap(), b"New user file");
        } else {
            assert!(!f.canonical().exists());
        }
    }
}

#[tokio::test]
async fn invalid_results_and_oversized_actual_serialization_fail_before_any_journal_or_canonical_write()
 {
    for kind in [
        "duplicate",
        "missing",
        "hint",
        "timing",
        "channel",
        "escaped",
    ] {
        let f = Fixture::new().await;
        let mut candidate = generated();
        match kind {
            "duplicate" => candidate.words[1].id = candidate.words[0].id.clone(),
            "missing" => candidate.words[0].id = None,
            "hint" => candidate.speaker_hints[0].word_id = "unknown".into(),
            "timing" => candidate.words[0].end_ms = f64::NAN,
            "channel" => candidate.words[0].channel = 3.0,
            _ => candidate.memo_md = "\0".repeat(12 * 1024 * 1024),
        }
        assert!(
            f.store
                .prepare_transcript_publication(
                    &f.operations,
                    f.identity.clone(),
                    f.source().await.baseline(),
                    candidate
                )
                .await
                .is_err(),
            "{kind}"
        );
        assert!(!f.operations.exists());
        assert!(!f.canonical().exists());
    }
}

#[tokio::test]
async fn corrupt_future_duplicate_and_oversized_receipts_fail_closed_and_preserve_bytes() {
    for kind in [
        "future",
        "duplicate",
        "namespace",
        "oversized",
        "result",
        "result_oversized",
    ] {
        let f = Fixture::new().await;
        drop(f.prepare().await);
        let (path, result) = f.paths();
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        let bytes = match kind {
            "future" => {
                raw["schema_version"] = 999.into();
                serde_json::to_vec(&raw).unwrap()
            }
            "namespace" => {
                raw["identity"]["namespace"] = "other".into();
                serde_json::to_vec(&raw).unwrap()
            }
            "duplicate" => {
                let original = std::fs::read_to_string(&path).unwrap();
                format!("{{\"schema_version\":1,{}", &original[1..]).into_bytes()
            }
            "result_oversized" => {
                std::fs::OpenOptions::new()
                    .write(true)
                    .open(&result)
                    .unwrap()
                    .set_len(io::RESULT_LIMIT as u64 + 1)
                    .unwrap();
                raw["state"] = "pending".into();
                serde_json::to_vec(&raw).unwrap()
            }
            "oversized" => vec![b' '; 64 * 1024 + 1],
            _ => {
                std::fs::write(&result, b"Changed generated result").unwrap();
                serde_json::to_vec(&raw).unwrap()
            }
        };
        std::fs::write(&path, &bytes).unwrap();
        let outcome = f
            .store
            .prepare_transcript_publication_status(&f.operations, f.identity.clone())
            .await;
        assert!(outcome.is_err(), "{kind}");
        if matches!(kind, "oversized" | "result_oversized") {
            assert!(
                matches!(outcome, Err(StoreError::Io(_))),
                "uncertain history cannot claim precommit resource refusal"
            );
            let outcome = f
                .store
                .prepare_transcript_publication(
                    &f.operations,
                    f.identity.clone(),
                    f.source().await.baseline(),
                    generated(),
                )
                .await;
            assert!(
                matches!(outcome, Err(StoreError::Io(_))),
                "same-request confirmation cannot claim precommit resource refusal"
            );
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert!(!f.canonical().exists());
    }
}

#[tokio::test]
async fn dropping_the_waiter_keeps_actual_commit_and_audio_ownership_alive_until_exit() {
    let f = Fixture::new().await;
    let plan = f.prepare().await;
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let started = std::sync::Mutex::new(Some(started_tx));
    let barrier = Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
    let blocked = barrier.clone();
    let waiter = tokio::spawn(async move {
        plan.commit_with(move |at| {
            if at == 1 {
                if let Some(tx) = started.lock().unwrap().take() {
                    let _ = tx.send(());
                }
                let (ready, wake) = &*blocked;
                let mut ready = ready.lock().unwrap();
                while !*ready {
                    ready = wake.wait(ready).unwrap();
                }
            }
            Ok(())
        })
        .await
    });
    tokio::time::timeout(Duration::from_secs(5), started_rx)
        .await
        .unwrap()
        .unwrap();
    waiter.abort();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(20),
            f.store.lock_session_audio("note")
        )
        .await
        .is_err()
    );
    *barrier.0.lock().unwrap() = true;
    barrier.1.notify_all();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Ok(guard) = f.store.lock_session_audio("note").await {
                drop(guard);
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(f.canonical()).unwrap(),
        std::fs::read(f.paths().1).unwrap()
    );
    assert!(matches!(
        f.status().await.commit().await.unwrap(),
        TranscriptPublicationOutcome::AlreadyCommitted { .. }
    ));
}

#[test]
fn publication_crash_child() {
    let Some(root) = std::env::var_os("LOOFAH_TRANSCRIPT_CRASH_ROOT") else {
        return;
    };
    let boundary: u8 = std::env::var("LOOFAH_TRANSCRIPT_CRASH_BOUNDARY")
        .unwrap()
        .parse()
        .unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let root = PathBuf::from(root);
        let store = SessionStore::new(root.join("vault"));
        let Some(TranscriptSourcePreparation::Ready(source)) =
            store.prepare_transcript_source("note").await.unwrap()
        else {
            panic!("missing source")
        };
        let identity = TranscriptPublicationIdentity {
            vault_id: "vault".into(),
            namespace: "member".into(),
            request_id: uuid::Uuid::parse_str("98b2c016-e063-49a7-a9fb-34dd105cbb6e").unwrap(),
        };
        store
            .prepare_transcript_publication(
                &root.join("operations/transcript-publications"),
                identity,
                source.baseline(),
                generated(),
            )
            .await
            .unwrap()
            .commit_with(move |at| {
                if at == boundary {
                    std::fs::write(root.join("child-boundary"), at.to_string()).unwrap();
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
async fn killed_process_recovers_original_staged_result_at_each_real_write_boundary() {
    for boundary in 0..=3 {
        let f = Fixture::new().await;
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "transcript_publication::tests::publication_crash_child",
                "--nocapture",
            ])
            .env("LOOFAH_TRANSCRIPT_CRASH_ROOT", f.root.path())
            .env("LOOFAH_TRANSCRIPT_CRASH_BOUNDARY", boundary.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let observed = tokio::time::timeout(Duration::from_secs(30), async {
            while !f.root.path().join("child-boundary").exists() {
                if let Some(status) = child.try_wait().unwrap() {
                    panic!("child exited before boundary {boundary}: {status}");
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        child.kill().unwrap();
        child.wait().unwrap();
        observed.unwrap();
        let reopened = SessionStore::new(f.root.path().join("vault"));
        let outcome = reopened
            .prepare_transcript_publication_status(&f.operations, f.identity.clone())
            .await
            .unwrap()
            .unwrap()
            .commit()
            .await
            .unwrap();
        assert!(
            matches!(
                outcome,
                TranscriptPublicationOutcome::Published { .. }
                    | TranscriptPublicationOutcome::AlreadyCommitted { .. }
            ),
            "boundary {boundary}: {outcome:?}"
        );
        assert_eq!(
            std::fs::read(f.canonical()).unwrap(),
            std::fs::read(f.paths().1).unwrap()
        );
    }
}

#[tokio::test]
async fn replacement_rechecks_raw_speaker_edits_and_preserves_retained_result() {
    let f = Fixture::new().await;
    let original = serde_json::to_vec(&TranscriptJson {
        transcripts: vec![generated()],
    })
    .unwrap();
    std::fs::write(f.canonical(), &original).unwrap();
    let Some(TranscriptSourcePreparation::Ready(source)) = f
        .store
        .prepare_transcript_replacement_source("note")
        .await
        .unwrap()
    else {
        panic!("missing replacement source")
    };
    let mut replacement = generated();
    replacement.id = "replacement".into();
    let plan = f
        .store
        .prepare_transcript_publication(
            &f.operations,
            f.identity.clone(),
            source.baseline(),
            replacement,
        )
        .await
        .unwrap();
    let mut edited: serde_json::Value = serde_json::from_slice(&original).unwrap();
    edited["transcripts"][0]["words"][0]["speaker"] = serde_json::json!("User correction");
    let edited = serde_json::to_vec(&edited).unwrap();
    std::fs::write(f.canonical(), &edited).unwrap();
    assert_eq!(
        plan.commit().await.unwrap(),
        TranscriptPublicationOutcome::Conflict
    );
    assert_eq!(std::fs::read(f.canonical()).unwrap(), edited);
    assert!(f.paths().1.exists());
}

#[tokio::test]
async fn replacement_recovers_after_canonical_rename_without_replaying_inference() {
    let f = Fixture::new().await;
    let original = serde_json::to_vec(&TranscriptJson {
        transcripts: vec![generated()],
    })
    .unwrap();
    std::fs::write(f.canonical(), &original).unwrap();
    let Some(TranscriptSourcePreparation::Ready(source)) = f
        .store
        .prepare_transcript_replacement_source("note")
        .await
        .unwrap()
    else {
        panic!("missing replacement source")
    };
    let mut replacement = generated();
    replacement.id = "replacement".into();
    let plan = f
        .store
        .prepare_transcript_publication(
            &f.operations,
            f.identity.clone(),
            source.baseline(),
            replacement,
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
    assert!(
        matches!(f.status().await.commit().await.unwrap(), TranscriptPublicationOutcome::Published { transcript_id, .. } if transcript_id == "replacement")
    );
    assert_eq!(
        std::fs::read(io::replacement_backup(&f.operations, &f.identity)).unwrap(),
        original
    );
    let canonical: TranscriptJson =
        serde_json::from_slice(&std::fs::read(f.canonical()).unwrap()).unwrap();
    assert_eq!(canonical.transcripts.len(), 1);
    assert_eq!(canonical.transcripts[0].id, "replacement");
}

#[tokio::test]
async fn replacement_source_uses_canonical_directory_and_recording_precedence() {
    let f = Fixture::new().await;
    for (filename, bytes) in [
        ("audio.wav", b"Canonical recording source".as_slice()),
        ("audio.mp3", b"preferred MP3".as_slice()),
    ] {
        std::fs::write(f.directory.join(filename), bytes).unwrap();
        let Some(TranscriptSourcePreparation::Ready(source)) = f
            .store
            .prepare_transcript_replacement_source("note")
            .await
            .unwrap()
        else {
            panic!("missing canonical recording source");
        };
        assert_eq!(source.recording_selection(), filename);
        let mut actual = Vec::new();
        source.recording_file().read_to_end(&mut actual).unwrap();
        assert_eq!(actual, bytes);
    }
    std::fs::remove_file(f.directory.join("audio.wav")).unwrap();
    std::fs::remove_file(f.directory.join("audio.mp3")).unwrap();
    assert!(matches!(
        f.store
            .prepare_transcript_replacement_source("note")
            .await
            .unwrap(),
        Some(TranscriptSourcePreparation::NoRecording)
    ));
}

async fn subtitle_source(f: &Fixture) -> SubtitleSourceSnapshot {
    let Some(SubtitleSourcePreparation::Ready(source)) = f
        .store
        .prepare_subtitle_source("note", crate::sha256(b"original subtitle"))
        .await
        .unwrap()
    else {
        panic!("subtitle source unavailable")
    };
    source
}

#[tokio::test]
async fn subtitle_import_without_audio_preserves_unknown_prefix_fields_and_historical_receipt() {
    let f = Fixture::new().await;
    std::fs::remove_file(f.directory.join("audio.wav")).unwrap();
    let mut old = serde_json::to_value(generated()).unwrap();
    old["id"] = "prefix".into();
    old["words"] = serde_json::json!([]);
    old["speaker_hints"] = serde_json::json!([]);
    old["future"] = serde_json::json!({"preserve":[1,true]});
    let original = serde_json::json!({"transcripts":[old.clone()],"future":{"root":42}});
    std::fs::write(f.canonical(), serde_json::to_vec(&original).unwrap()).unwrap();
    let source = subtitle_source(&f).await;
    assert!(source.transcript_present());
    let outcome = f
        .store
        .prepare_subtitle_publication(&f.operations, f.identity.clone(), source, generated())
        .await
        .unwrap()
        .recheck()
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        TranscriptPublicationOutcome::Published { .. }
    ));
    let merged: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.canonical()).unwrap()).unwrap();
    assert_eq!(merged["future"], original["future"]);
    assert_eq!(merged["transcripts"][0], old);
    assert_eq!(
        merged["transcripts"][1],
        serde_json::to_value(generated()).unwrap()
    );
    std::fs::remove_file(f.canonical()).unwrap();
    assert!(matches!(
        f.store
            .completed_subtitle_publication(&f.operations, f.identity.clone())
            .await
            .unwrap(),
        Some(TranscriptPublicationOutcome::AlreadyCommitted { .. })
    ));
    assert!(matches!(
        f.status().await.commit().await.unwrap(),
        TranscriptPublicationOutcome::AlreadyCommitted { .. }
    ));
    assert!(!f.canonical().exists());
}

#[tokio::test]
async fn subtitle_import_rejects_changed_raw_presence_and_session_generation() {
    for mutation in 0..3 {
        let f = Fixture::new().await;
        let source = subtitle_source(&f).await;
        assert!(!source.transcript_present());
        let plan = f
            .store
            .prepare_subtitle_publication(&f.operations, f.identity.clone(), source, generated())
            .await
            .unwrap()
            .recheck()
            .await
            .unwrap();
        if mutation == 0 {
            std::fs::write(f.canonical(), b"").unwrap();
        }
        if mutation == 1 {
            std::fs::write(f.canonical(), b"{\"transcripts\":[]}").unwrap();
        }
        if mutation == 2 {
            let path = f.directory.join("_meta.json");
            let mut meta: serde_json::Value =
                serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            meta["created_at"] = "2026-11-09T10:00:00Z".into();
            std::fs::write(path, serde_json::to_vec(&meta).unwrap()).unwrap();
        }
        assert!(matches!(
            plan.commit().await.unwrap(),
            TranscriptPublicationOutcome::Conflict
        ));
        if mutation == 0 {
            assert_eq!(std::fs::read(f.canonical()).unwrap(), b"");
        }
        if mutation == 2 {
            assert!(!f.canonical().exists());
        }
    }
}

#[tokio::test]
async fn subtitle_import_rejects_duplicate_json_and_cannot_upgrade_audio_receipts() {
    let f = Fixture::new().await;
    std::fs::write(f.canonical(), b"{\"transcripts\":[],\"transcripts\":[]}").unwrap();
    let source = subtitle_source(&f).await;
    assert!(
        f.store
            .prepare_subtitle_publication(&f.operations, f.identity.clone(), source, generated())
            .await
            .is_err()
    );
    assert!(!f.paths().0.exists());
    std::fs::remove_file(f.canonical()).unwrap();
    let plan = f.prepare().await;
    drop(plan);
    assert!(
        f.store
            .completed_subtitle_publication(&f.operations, f.identity.clone())
            .await
            .is_err()
    );
    let source = subtitle_source(&f).await;
    assert!(
        f.store
            .prepare_subtitle_publication(&f.operations, f.identity.clone(), source, generated())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn subtitle_import_respects_active_recordings_and_recovers_all_publication_boundaries() {
    let f = Fixture::new().await;
    f.store.note_recording_active("note");
    assert!(matches!(
        f.store
            .prepare_subtitle_source("note", crate::sha256(b"subtitle"))
            .await
            .unwrap(),
        Some(SubtitleSourcePreparation::Busy)
    ));
    for boundary in 0..=3 {
        let f = Fixture::new().await;
        std::fs::remove_file(f.directory.join("audio.wav")).unwrap();
        let source = subtitle_source(&f).await;
        let plan = f
            .store
            .prepare_subtitle_publication(&f.operations, f.identity.clone(), source, generated())
            .await
            .unwrap();
        assert!(
            plan.commit_with(move |at| if at == boundary {
                Err(StoreError::Io("Injected crash".into()))
            } else {
                Ok(())
            })
            .await
            .is_err()
        );
        let result = std::fs::read(f.paths().1).unwrap();
        let reopened = SessionStore::new(f.root.path().join("vault"));
        let outcome = reopened
            .prepare_transcript_publication_status(&f.operations, f.identity.clone())
            .await
            .unwrap()
            .unwrap()
            .commit()
            .await
            .unwrap();
        assert!(matches!(
            outcome,
            TranscriptPublicationOutcome::Published { .. }
                | TranscriptPublicationOutcome::AlreadyCommitted { .. }
        ));
        assert_eq!(std::fs::read(f.canonical()).unwrap(), result);
    }
}
