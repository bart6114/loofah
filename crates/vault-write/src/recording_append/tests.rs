use super::*;
use hypr_storage::vault::{LeaseAttempt, VaultLease};
use std::io::{Cursor, Read};

struct Fixture {
    root: tempfile::TempDir,
    store: SessionStore,
    operations: PathBuf,
    directory: PathBuf,
    identity: AppendIdentity,
}
impl Fixture {
    async fn new(selection: &str, transcript: Option<&[u8]>) -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let vault = base.join("vault");
        std::fs::create_dir(&vault).unwrap();
        let store = SessionStore::new(vault.clone());
        let meta=serde_json::from_value(serde_json::json!({"id":"note","title":"Original","created_at":"2026-11-08T00:00:00Z","tags":[],"future":{"private":true}})).unwrap();
        store
            .create_note_session(&meta, "Original note")
            .await
            .unwrap();
        let directory = vault.join("sessions/note");
        if selection.starts_with("audio/") {
            std::fs::create_dir(directory.join("audio")).unwrap();
        }
        std::fs::write(directory.join(selection), b"original encoded recording").unwrap();
        if let Some(bytes) = transcript {
            std::fs::write(directory.join("transcript.json"), bytes).unwrap();
        }
        std::fs::write(directory.join("user.wav"), b"unknown attachment").unwrap();
        std::fs::write(directory.join(".private"), b"private").unwrap();
        let identity = AppendIdentity {
            server: "https://server.example".into(),
            vault_id: "vault".into(),
            member_id: "member".into(),
            capture_id: uuid::Uuid::new_v4(),
            request_id: uuid::Uuid::new_v4(),
            admission_id: uuid::Uuid::new_v4(),
        };
        Self {
            operations: base.join("operations"),
            root,
            store,
            directory,
            identity,
        }
    }
    async fn source(&self) -> AppendSource {
        self.store
            .prepare_recording_append_source(&self.operations, self.identity.clone(), "note")
            .await
            .unwrap()
    }
    async fn plan(&self, merged: Option<Vec<u8>>) -> RecordingAppendPlan {
        let source = self.source().await;
        let (bytes, artifact) = wave(&[0.25, -0.5, 0.75]);
        let path = self.root.path().join("combined.wav");
        std::fs::write(&path, bytes).unwrap();
        self.store
            .prepare_recording_append(
                &self.operations,
                self.identity.clone(),
                source,
                File::open(path).unwrap(),
                artifact,
                merged,
            )
            .await
            .unwrap()
    }
    fn canonical(&self) -> Vec<u8> {
        std::fs::read(self.directory.join("audio.wav")).unwrap()
    }
}
fn wave(samples: &[f32]) -> (Vec<u8>, RecordingAppendArtifact) {
    let mut bytes = Cursor::new(vec![]);
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: 16000,
                bits_per_sample: 32,
                sample_format: hound::SampleFormat::Float,
            },
        )
        .unwrap();
        for value in samples {
            writer.write_sample(*value).unwrap();
        }
        writer.finalize().unwrap();
    }
    let bytes = bytes.into_inner();
    let artifact = RecordingAppendArtifact {
        sha256: crate::sha256(&bytes),
        size_bytes: bytes.len() as u64,
        frames: samples.len() as u64,
        layout: hypr_vault_read::AudioLayout::Mixed,
    };
    (bytes, artifact)
}

#[tokio::test]
async fn retained_append_source_retry_never_reseeds_from_new_canonical_content() {
    let fixture = Fixture::new(
        "audio/audio.ogg",
        Some(b"{\"transcripts\":[],\"future\":true}"),
    )
    .await;
    let source = fixture.source().await;
    assert_eq!(source.recording_selection(), "audio/audio.ogg");
    assert!(source.recheck().await.unwrap());
    std::fs::write(fixture.directory.join("audio/audio.ogg"), b"new generation").unwrap();
    std::fs::write(fixture.directory.join("transcript.json"), b"{}").unwrap();
    let retry = fixture.source().await;
    let mut bytes = vec![];
    retry.recording_file().read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"original encoded recording");
    assert_eq!(retry.raw_transcript(), source.raw_transcript());
    assert!(!retry.recheck().await.unwrap());
    let mut foreign = fixture.identity.clone();
    foreign.request_id = uuid::Uuid::new_v4();
    assert!(
        fixture
            .store
            .prepare_recording_append_source(&fixture.operations, foreign, "note")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn compound_append_migrates_exact_owned_selection_preserves_unknown_and_is_historical() {
    for selection in io::RECORDINGS {
        let before = br#"{"transcripts":[],"future":{"keep":true}}"#;
        let after = br#"{"transcripts":[],"future":{"keep":true},"appended":true}"#;
        let fixture = Fixture::new(selection, Some(before)).await;
        let metadata = std::fs::read(fixture.directory.join("_meta.json")).unwrap();
        std::fs::write(fixture.directory.join("audio.peaks.json"), b"stale").unwrap();
        let plan = fixture
            .plan(Some(after.to_vec()))
            .await
            .recheck()
            .await
            .unwrap();
        fixture
            .store
            .save_note(
                "note",
                "Edited while generation was prepared",
                Some("Edited title"),
            )
            .await
            .unwrap();
        let outcome = plan.commit().await.unwrap();
        assert!(matches!(
            outcome,
            RecordingAppendOutcome::Published {
                transcript_present: true,
                ..
            }
        ));
        assert_eq!(fixture.canonical(), wave(&[0.25, -0.5, 0.75]).0);
        assert_eq!(
            std::fs::read(fixture.directory.join("transcript.json")).unwrap(),
            after
        );
        assert_eq!(
            std::fs::read(fixture.directory.join("user.wav")).unwrap(),
            b"unknown attachment"
        );
        assert_eq!(
            std::fs::read(fixture.directory.join(".private")).unwrap(),
            b"private"
        );
        assert!(!fixture.directory.join("audio.peaks.json").exists());
        let original: serde_json::Value = serde_json::from_slice(&metadata).unwrap();
        let current: serde_json::Value =
            serde_json::from_slice(&std::fs::read(fixture.directory.join("_meta.json")).unwrap())
                .unwrap();
        assert_eq!(current["future"], original["future"]);
        assert_eq!(current["title"], "Edited title");
        std::fs::remove_file(fixture.directory.join("audio.wav")).unwrap();
        let status = fixture
            .store
            .prepare_recording_append_status(&fixture.operations, fixture.identity.clone())
            .await
            .unwrap()
            .unwrap()
            .commit()
            .await
            .unwrap();
        assert!(matches!(
            status,
            RecordingAppendOutcome::AlreadyCommitted { .. }
        ));
        assert!(!fixture.directory.join("audio.wav").exists());
    }
}

#[tokio::test]
async fn interrupted_append_recovers_exact_retained_compound_bytes_at_every_step() {
    for step in 1..=6 {
        let fixture = Fixture::new(
            "audio/audio.mp3",
            Some(br#"{"transcripts":[],"before":true}"#),
        )
        .await;
        let after = br#"{"transcripts":[],"after":true}"#.to_vec();
        let plan = fixture.plan(Some(after.clone())).await;
        assert!(
            plan.commit_with(move |at| if at == step {
                Err(io::invalid("injected interruption"))
            } else {
                Ok(())
            })
            .await
            .is_err()
        );
        if step < 6 {
            assert!(fixture.store.session_transcripts("note").await.is_err());
            assert!(
                fixture
                    .store
                    .indexed_transcript_overview("note", None)
                    .is_err()
            );
            assert!(fixture.store.open_recording("note").await.is_err());
            assert!(fixture.store.lock_session_audio("note").await.is_err());
            fixture
                .store
                .save_note("note", "Still editable", Some("Still editable title"))
                .await
                .unwrap();
        }
        let reopened = SessionStore::new(fixture.store.vault_base().into());
        let pending = reopened
            .discover_recording_appends(&fixture.operations, "vault")
            .await
            .unwrap();
        if step < 6 {
            assert_eq!(pending.len(), 1);
            assert!(reopened.session_transcripts("note").await.is_err());
            let outcome = reopened
                .prepare_recording_append_recovery(
                    &fixture.operations,
                    pending.into_iter().next().unwrap(),
                )
                .await
                .unwrap()
                .recheck()
                .await
                .unwrap()
                .commit()
                .await
                .unwrap();
            assert!(matches!(outcome, RecordingAppendOutcome::Published { .. }));
        } else {
            assert!(pending.is_empty());
        }
        assert_eq!(fixture.canonical(), wave(&[0.25, -0.5, 0.75]).0);
        assert_eq!(
            std::fs::read(fixture.directory.join("transcript.json")).unwrap(),
            after
        );
        assert!(
            reopened
                .session_transcripts("note")
                .await
                .unwrap()
                .is_empty()
        );
    }
}

#[tokio::test]
async fn absent_and_empty_transcript_are_retained_and_committed_as_distinct_states() {
    for original in [None, Some(b"".as_slice())] {
        let fixture = Fixture::new("audio.wav", original).await;
        let source = fixture.source().await;
        assert_eq!(source.raw_transcript(), original);
        fixture
            .plan(original.map(Vec::from))
            .await
            .commit()
            .await
            .unwrap();
        assert_eq!(
            fixture.directory.join("transcript.json").exists(),
            original.is_some()
        );
    }
}

#[tokio::test]
async fn diverged_transcript_after_partial_audio_move_requires_review_without_overwrite() {
    let fixture = Fixture::new("audio.mp3", Some(br#"{"transcripts":[]}"#)).await;
    assert!(
        fixture
            .plan(Some(br#"{"transcripts":[],"append":true}"#.to_vec()))
            .await
            .commit_with(|at| if at == 3 {
                Err(io::invalid("interrupt"))
            } else {
                Ok(())
            })
            .await
            .is_err()
    );
    let changed = br#"{"transcripts":[],"external":true}"#;
    std::fs::write(fixture.directory.join("transcript.json"), changed).unwrap();
    let outcome = fixture
        .store
        .prepare_recording_append_status(&fixture.operations, fixture.identity.clone())
        .await
        .unwrap()
        .unwrap()
        .commit()
        .await
        .unwrap();
    assert_eq!(outcome, RecordingAppendOutcome::RequiresReview);
    assert_eq!(
        std::fs::read(fixture.directory.join("transcript.json")).unwrap(),
        changed
    );
    assert!(fixture.store.open_recording("note").await.is_err());
}

#[tokio::test]
async fn future_receipt_schema_and_foreign_recovery_owner_fail_closed() {
    let fixture = Fixture::new("audio.mp3", None).await;
    assert!(
        fixture
            .plan(None)
            .await
            .commit_with(|at| if at == 1 {
                Err(io::invalid("interrupt"))
            } else {
                Ok(())
            })
            .await
            .is_err()
    );
    let pending = fixture
        .store
        .discover_recording_appends(&fixture.operations, "vault")
        .await
        .unwrap()
        .pop()
        .unwrap();
    let foreign = SessionStore::new(fixture.store.vault_base().into());
    assert!(
        foreign
            .prepare_recording_append_recovery(&fixture.operations, pending)
            .await
            .is_err()
    );
    let path = io::root(&fixture.operations, &fixture.identity)
        .unwrap()
        .join("intent.json");
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["schema_version"] = 2.into();
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(
        foreign
            .discover_recording_appends(&fixture.operations, "vault")
            .await
            .is_err()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn append_refuses_symlinks_and_multiple_owned_recordings_without_enumerating_attachments() {
    let fixture = Fixture::new("audio.mp3", None).await;
    std::fs::write(fixture.directory.join("audio.ogg"), b"second").unwrap();
    assert!(
        fixture
            .store
            .prepare_recording_append_source(&fixture.operations, fixture.identity.clone(), "note")
            .await
            .is_err()
    );
    std::fs::remove_file(fixture.directory.join("audio.ogg")).unwrap();
    std::fs::remove_file(fixture.directory.join("audio.mp3")).unwrap();
    std::os::unix::fs::symlink("user.wav", fixture.directory.join("audio.mp3")).unwrap();
    assert!(
        fixture
            .store
            .prepare_recording_append_source(&fixture.operations, fixture.identity.clone(), "note")
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(fixture.directory.join("user.wav")).unwrap(),
        b"unknown attachment"
    );
}

#[tokio::test]
async fn append_caller_abandonment_retains_original_lease_through_actual_io() {
    let fixture = Fixture::new("audio.mp3", None).await;
    let LeaseAttempt::Acquired(lease) =
        VaultLease::try_acquire_existing(fixture.store.vault_base()).unwrap()
    else {
        panic!("owner")
    };
    fixture.store.pin_vault_lease(&lease).unwrap();
    let original = Arc::downgrade(&lease.pin());
    let plan = fixture.plan(None).await;
    let (entered, reached) = tokio::sync::oneshot::channel();
    let entered = std::sync::Mutex::new(Some(entered));
    let (release, wait) = std::sync::mpsc::channel();
    let wait = std::sync::Mutex::new(wait);
    let task = tokio::spawn(async move {
        plan.commit_with(move |at| {
            if at == 2 {
                let _ = entered.lock().unwrap().take().unwrap().send(());
                wait.lock().unwrap().recv().unwrap();
            }
            Ok(())
        })
        .await
    });
    reached.await.unwrap();
    task.abort();
    let vault = fixture.store.vault_base().to_owned();
    let canonical = fixture.directory.join("audio.wav");
    drop(fixture.store);
    drop(lease);
    assert!(original.upgrade().is_some());
    assert!(matches!(
        VaultLease::try_acquire_existing(&vault).unwrap(),
        LeaseAttempt::Busy
    ));
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while original.upgrade().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        std::fs::read(canonical).unwrap(),
        wave(&[0.25, -0.5, 0.75]).0
    );
    assert!(matches!(
        VaultLease::try_acquire_existing(&vault).unwrap(),
        LeaseAttempt::Acquired(_)
    ));
}

#[tokio::test]
async fn changed_generation_between_recheck_and_commit_never_overwrites_canonical_files() {
    for changed in ["audio.mp3", "transcript.json"] {
        let fixture = Fixture::new("audio.mp3", Some(br#"{"transcripts":[]}"#)).await;
        let plan = fixture
            .plan(Some(br#"{"transcripts":[],"new":true}"#.to_vec()))
            .await
            .recheck()
            .await
            .unwrap();
        #[cfg(windows)]
        if changed == "audio.mp3" {
            use std::io::Write;
            let mut in_place = std::fs::OpenOptions::new()
                .write(true)
                .open(fixture.directory.join(changed))
                .unwrap();
            assert_eq!(
                in_place
                    .write_all(b"must stay locked")
                    .unwrap_err()
                    .raw_os_error(),
                Some(33),
                "the original shared lock must still prevent in-place audio writes"
            );
        }
        // A real external save can replace the directory entry while the original
        // source descriptor stays shared-locked (Windows rejects in-place writes).
        hypr_storage::fs::atomic_write(&fixture.directory.join(changed), "external generation")
            .unwrap();
        assert_eq!(
            plan.commit().await.unwrap(),
            RecordingAppendOutcome::Changed
        );
        assert_eq!(
            std::fs::read(fixture.directory.join(changed)).unwrap(),
            b"external generation"
        );
        assert!(!fixture.directory.join("audio.wav").exists());
    }
}

#[tokio::test]
async fn append_validates_wave_frame_count_and_finite_payload_before_canonical_intent() {
    for nonfinite in [false, true] {
        let fixture = Fixture::new("audio.mp3", None).await;
        let source = fixture.source().await;
        let (bytes, mut artifact) = wave(&[if nonfinite { f32::NAN } else { 0.5 }]);
        if !nonfinite {
            artifact.frames += 1;
        }
        let path = fixture.root.path().join("invalid.wav");
        std::fs::write(&path, bytes).unwrap();
        assert!(
            fixture
                .store
                .prepare_recording_append(
                    &fixture.operations,
                    fixture.identity.clone(),
                    source,
                    File::open(path).unwrap(),
                    artifact,
                    None
                )
                .await
                .is_err()
        );
        assert_eq!(
            std::fs::read(fixture.directory.join("audio.mp3")).unwrap(),
            b"original encoded recording"
        );
        assert!(
            !io::root(&fixture.operations, &fixture.identity)
                .unwrap()
                .join("intent.json")
                .exists()
        );
    }
}

#[tokio::test]
async fn startup_append_recovery_requires_original_lease_and_fences_before_readiness() {
    let fixture = Fixture::new("audio.mp3", None).await;
    let LeaseAttempt::Acquired(lease) =
        VaultLease::try_acquire_existing(fixture.store.vault_base()).unwrap()
    else {
        panic!("owner")
    };
    fixture.store.pin_vault_lease(&lease).unwrap();
    assert!(
        fixture
            .plan(None)
            .await
            .commit_with(|step| if step == 2 {
                Err(io::invalid("interrupted migration"))
            } else {
                Ok(())
            })
            .await
            .is_err()
    );
    let reopened = SessionStore::new(fixture.store.vault_base().into());
    reopened.pin_vault_lease(&lease).unwrap();
    reopened.set_startup_pending(true);
    assert!(
        reopened
            .recording_append_startup_recovery(&Arc::new(
                File::open(fixture.directory.join("_meta.json")).unwrap()
            ))
            .is_err()
    );
    let recovery = reopened
        .recording_append_startup_recovery(&lease.pin())
        .unwrap();
    let pending = recovery
        .discover(&fixture.operations, "vault")
        .await
        .unwrap();
    assert!(reopened.session_transcripts("note").await.is_err());
    recovery
        .prepare(&fixture.operations, pending.into_iter().next().unwrap())
        .await
        .unwrap()
        .commit()
        .await
        .unwrap();
    reopened.set_startup_pending(false);
    assert!(reopened.open_recording("note").await.is_ok());
}

#[tokio::test]
async fn invalid_merged_transcript_is_rejected_before_any_canonical_intent() {
    let fixture = Fixture::new("audio.mp3", None).await;
    let source = fixture.source().await;
    let (bytes, artifact) = wave(&[0.5]);
    let path = fixture.root.path().join("output.wav");
    std::fs::write(&path, bytes).unwrap();
    assert!(
        fixture
            .store
            .prepare_recording_append(
                &fixture.operations,
                fixture.identity.clone(),
                source,
                File::open(path).unwrap(),
                artifact,
                Some(vec![0xff])
            )
            .await
            .is_err()
    );
    assert_eq!(
        std::fs::read(fixture.directory.join("audio.mp3")).unwrap(),
        b"original encoded recording"
    );
    assert!(
        !io::root(&fixture.operations, &fixture.identity)
            .unwrap()
            .join("intent.json")
            .exists()
    );
}

#[tokio::test]
async fn equal_admission_ids_cannot_clear_or_share_another_scope_transaction() {
    let fixture = Fixture::new("audio.mp3", None).await;
    let original = fixture.identity.clone();
    fixture
        .store
        .set_append_pending(&original, "note", true)
        .unwrap();
    for changed in 0..5 {
        let mut other = original.clone();
        match changed {
            0 => other.server.push_str("/other"),
            1 => other.vault_id.push_str("other"),
            2 => other.member_id.push_str("other"),
            3 => other.capture_id = uuid::Uuid::new_v4(),
            _ => other.request_id = uuid::Uuid::new_v4(),
        }
        assert_ne!(original.artifact_key(), other.artifact_key());
        assert!(
            fixture
                .store
                .set_append_pending(&other, "note", true)
                .is_err()
        );
        fixture
            .store
            .set_append_pending(&other, "note", false)
            .unwrap();
        assert!(fixture.store.ensure_append_ready("note").is_err());
    }
    fixture
        .store
        .set_append_pending(&original, "note", false)
        .unwrap();
    assert!(fixture.store.ensure_append_ready("note").is_ok());
}
