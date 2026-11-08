use super::*;
use crate::VaultRuntime;
use hypr_loofah_protocol::{
    CaptureChunkDescriptor, CaptureFormat, CaptureIdentity, CaptureIngestRequest,
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Fixture {
    _root: tempfile::TempDir,
    vault: PathBuf,
    operations: PathBuf,
    context: Option<Arc<crate::VaultContext>>,
    input: CaptureIngestRequest,
    seal: CaptureSeal,
    bytes: Vec<u8>,
}

async fn leased_context(
    vault: &std::path::Path,
    operations: &std::path::Path,
) -> Arc<crate::VaultContext> {
    std::fs::create_dir_all(vault).unwrap();
    let lease = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(vault.join(".loofah-vault.lock"))
        .unwrap();
    lease.try_lock().unwrap();
    let store = Arc::new(hypr_vault_write::SessionStore::new(vault.into()));
    let (lease, gates) = crate::VaultContext::bootstrap_recording_publications(
        store.clone(),
        "vault".into(),
        operations.join("recording-publications"),
        lease,
    )
    .await
    .unwrap();
    let mut context = crate::VaultContext::new(
        "vault".into(),
        store,
        crate::startup::StartupState::new(Some(vault)),
        Arc::new(crate::settings::ConfigState::load_or_default(vault)),
        Arc::new(crate::search::SearchService::default()),
    )
    .with_operation_storage(operations.into());
    context.lease = Some(lease);
    context.recording_publications = gates;
    context
        .enable_capture_ingestion(
            "https://capture.example/".into(),
            super::super::CaptureSpoolLimits::default(),
        )
        .unwrap();
    Arc::new(context)
}

impl Fixture {
    async fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let vault = base.join("vault");
        let operations = base.join("operations");
        let context = leased_context(&vault, &operations).await;
        for id in ["note", "other"] {
            let meta = serde_json::from_value(serde_json::json!({"id":id,"title":"User title","created_at":"2026-11-08T10:00:00Z","tags":["keep"],"future":{"nested":[true,null]}})).unwrap();
            context
                .store
                .create_note_session(&meta, "Keep user notes")
                .await
                .unwrap();
        }
        let input = CaptureIngestRequest {
            identity: CaptureIdentity {
                scope: CaptureScope {
                    server: "https://capture.example/".into(),
                    member_id: "member".into(),
                    vault_id: "vault".into(),
                },
                capture_id: uuid::Uuid::new_v4().to_string(),
                request_id: uuid::Uuid::new_v4().to_string(),
                target: CaptureTarget::ExistingSession {
                    session_id: "note".into(),
                    expected_audio_sha256: None,
                },
            },
            format: CaptureFormat::stereo(),
            first_admitted_audio_observed_at_ms: 1_794_000_000_001,
        };
        let mut status = context
            .begin_capture_authorized("member".into(), input.clone(), || async { Ok(()) })
            .await
            .unwrap();
        let mut all = Vec::new();
        for sequence in 0..2 {
            let bytes: Vec<_> = (0..4)
                .flat_map(|_| [0.125_f32.to_le_bytes(), (-0.25_f32).to_le_bytes()].concat())
                .collect();
            let mut descriptor = CaptureChunkDescriptor {
                capture_fingerprint: status.prefix.capture_fingerprint.clone(),
                sequence,
                first_frame: status.prefix.frames,
                frames: 4,
                pcm_offset: status.prefix.pcm_bytes,
                bytes: bytes.len() as u32,
                sha256: hash(&bytes),
                previous_digest: status.prefix.chain_digest.clone(),
                digest: String::new(),
            };
            descriptor.digest = hypr_loofah_protocol::capture_chunk_digest(&descriptor);
            status.prefix = context
                .append_capture_authorized(
                    "member".into(),
                    input.identity.capture_id.clone(),
                    descriptor,
                    bytes.clone(),
                    || async { Ok(()) },
                )
                .await
                .unwrap();
            all.extend(bytes);
        }
        let seal = CaptureSeal {
            capture_fingerprint: status.prefix.capture_fingerprint,
            chunk_count: status.prefix.chunk_count,
            frames: status.prefix.frames,
            pcm_bytes: status.prefix.pcm_bytes,
            chain_digest: status.prefix.chain_digest,
            source_sha256: hash(&all),
        };
        context
            .seal_capture_authorized(
                "member".into(),
                input.identity.capture_id.clone(),
                seal.clone(),
                || async { Ok(()) },
            )
            .await
            .unwrap();
        Self {
            _root: root,
            vault,
            operations,
            context: Some(context),
            input,
            seal,
            bytes: all,
        }
    }

    fn context(&self) -> &Arc<crate::VaultContext> {
        self.context.as_ref().unwrap()
    }

    async fn writer(&self) -> super::super::CaptureWriter {
        self.context()
            .capture_ingestion
            .get()
            .unwrap()
            .spool
            .get()
            .unwrap()
            .resume(self.input.identity.clone())
            .await
            .unwrap()
    }

    async fn publish(&self) -> Result<CaptureIngestStatus, ApplicationError> {
        self.context()
            .publish_capture_authorized(
                "member".into(),
                self.input.identity.capture_id.clone(),
                self.seal.clone(),
                || async { Ok(()) },
            )
            .await
    }

    async fn stop(&mut self) {
        self.context().store.set_startup_pending(true);
        drop(self.context.take());
    }
}

#[tokio::test]
async fn absent_only_publication_preserves_notes_metadata_and_raw_capture_and_replays_exact_receipt()
 {
    let mut fixture = Fixture::new().await;
    let notes = std::fs::read(fixture.vault.join("sessions/note/notes.md")).unwrap();
    let published = fixture.publish().await.unwrap();
    let publication = published.publication.as_ref().unwrap();
    assert_eq!(publication.source, fixture.seal);
    let bytes = std::fs::read(fixture.vault.join("sessions/note/audio.wav")).unwrap();
    assert_eq!(publication.canonical_audio_sha256, hash(&bytes));
    assert_eq!(publication.canonical_audio_bytes, bytes.len() as u64);
    let decoded = hound::WavReader::new(std::io::Cursor::new(bytes))
        .unwrap()
        .samples::<f32>()
        .flat_map(|sample| sample.unwrap().to_le_bytes())
        .collect::<Vec<_>>();
    assert_eq!(decoded, fixture.bytes);
    let meta: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.vault.join("sessions/note/_meta.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["title"], "User title");
    assert_eq!(meta["future"], serde_json::json!({"nested":[true,null]}));
    assert!(meta["ended_at"].is_null());
    assert_eq!(
        std::fs::read(fixture.vault.join("sessions/note/notes.md")).unwrap(),
        notes
    );
    assert_eq!(
        fixture.writer().await.read_chunk(0).await.unwrap().bytes,
        fixture.bytes[..32]
    );
    assert_eq!(fixture.publish().await.unwrap(), published);
    assert!(
        fixture
            .context()
            .ensure_recording_publication_ready("note")
            .is_ok()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn committed_canonical_audio_and_prefix_ack_survive_restart_before_publication_observation() {
    let mut fixture = Fixture::new().await;
    let writer = fixture.writer().await;
    let verified = wav::verify(&writer, &fixture.seal).await.unwrap();
    let request = publication_request(&verified).unwrap();
    let (reader, producer) = verified.reader(fixture.context().clone(), writer.clone());
    let plan = fixture
        .context()
        .store
        .prepare_recording_publication(
            &fixture.operations.join("recording-publications"),
            request.clone(),
            reader,
        )
        .await
        .unwrap();
    producer.await.unwrap().unwrap();
    assert!(matches!(
        plan.recheck().await.unwrap().commit().await.unwrap(),
        RecordingPublication::Published { .. }
    ));
    writer
        .acknowledge_prefix(CapturePrefixReceipt {
            capture_fingerprint: fixture.seal.capture_fingerprint.clone(),
            chunk_count: fixture.seal.chunk_count,
            frames: fixture.seal.frames,
            pcm_bytes: fixture.seal.pcm_bytes,
            chain_digest: fixture.seal.chain_digest.clone(),
        })
        .await
        .unwrap();
    assert!(writer.snapshot().await.unwrap().publication.is_none());
    assert_eq!(
        publication_request(&wav::verify(&writer, &fixture.seal).await.unwrap()).unwrap(),
        request
    );
    let path = fixture.vault.join("sessions/note/audio.wav");
    let bytes = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    drop(writer);
    fixture.stop().await;
    fixture.context = Some(leased_context(&fixture.vault, &fixture.operations).await);
    let completed = fixture.publish().await.unwrap().publication.unwrap();
    assert_eq!(completed.canonical_audio_sha256, hash(&bytes));
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    assert_eq!(
        fixture.writer().await.read_chunk(0).await.unwrap().bytes,
        fixture.bytes[..32]
    );
    fixture.stop().await;
}

#[tokio::test]
async fn authorization_is_rechecked_after_staging_and_denial_never_publishes_or_discards_source() {
    let mut fixture = Fixture::new().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let authorize = calls.clone();
    let failure = fixture
        .context()
        .publish_capture_authorized(
            "member".into(),
            fixture.input.identity.capture_id.clone(),
            fixture.seal.clone(),
            move || {
                let allowed = authorize.fetch_add(1, Ordering::SeqCst) == 0;
                async move {
                    if allowed {
                        Ok(())
                    } else {
                        Err(rejected(ErrorCode::Forbidden, "Access revoked"))
                    }
                }
            },
        )
        .await
        .unwrap_err();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(failure.code, ErrorCode::Forbidden);
    assert_eq!(
        failure.mutation_outcome,
        Some(MutationOutcome::NotCommitted)
    );
    assert!(!fixture.vault.join("sessions/note/audio.wav").exists());
    assert!(
        fixture
            .writer()
            .await
            .snapshot()
            .await
            .unwrap()
            .publication
            .is_none()
    );
    assert_eq!(
        fixture.writer().await.read_chunk(0).await.unwrap().bytes,
        fixture.bytes[..32]
    );
    assert!(
        fixture
            .context()
            .ensure_recording_publication_ready("note")
            .is_ok()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn publication_rejects_conflicting_audio_transcripts_missing_targets_and_changed_seals() {
    for kind in [
        "audio",
        "transcript",
        "missing",
        "seal",
        "append",
        "pending",
    ] {
        let mut fixture = Fixture::new().await;
        let directory = fixture.vault.join("sessions/note");
        match kind {
            "audio" => std::fs::write(directory.join("audio.wav"), b"User recording").unwrap(),
            "transcript" => {
                std::fs::write(directory.join("transcript.json"), b"User transcript canary")
                    .unwrap()
            }
            "missing" => {
                std::fs::rename(&directory, fixture.vault.join("sessions/moved-by-user")).unwrap()
            }
            "seal" => fixture.seal.source_sha256 = "0".repeat(64),
            "append" | "pending" => {
                let mut input = fixture.input.clone();
                input.identity.capture_id = uuid::Uuid::new_v4().to_string();
                input.identity.request_id = uuid::Uuid::new_v4().to_string();
                input.identity.target = if kind == "append" {
                    CaptureTarget::ExistingSession {
                        session_id: "note".into(),
                        expected_audio_sha256: Some("0".repeat(64)),
                    }
                } else {
                    CaptureTarget::PendingSession {
                        create_request_id: uuid::Uuid::new_v4().to_string(),
                    }
                };
                fixture
                    .context()
                    .begin_capture_authorized("member".into(), input.clone(), || async { Ok(()) })
                    .await
                    .unwrap();
                fixture.input = input;
            }
            _ => unreachable!(),
        }
        assert!(fixture.publish().await.is_err(), "{kind}");
        if kind == "audio" {
            assert_eq!(
                std::fs::read(directory.join("audio.wav")).unwrap(),
                b"User recording"
            );
        }
        if kind == "transcript" {
            assert_eq!(
                std::fs::read(directory.join("transcript.json")).unwrap(),
                b"User transcript canary"
            );
            assert!(!directory.join("audio.wav").exists());
        }
        assert!(fixture.context().read_note("other").await.is_ok());
        fixture.stop().await;
    }
}

#[tokio::test]
async fn publication_never_selects_another_authenticated_members_capture() {
    let mut fixture = Fixture::new().await;
    let failure = fixture
        .context()
        .publish_capture_authorized(
            "other-member".into(),
            fixture.input.identity.capture_id.clone(),
            fixture.seal.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap_err();
    assert_eq!(failure.code, ErrorCode::NotFound);
    assert_eq!(
        failure.mutation_outcome,
        Some(MutationOutcome::NotCommitted)
    );
    assert!(!fixture.vault.join("sessions/note/audio.wav").exists());
    assert!(
        fixture
            .writer()
            .await
            .snapshot()
            .await
            .unwrap()
            .publication
            .is_none()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn a_reader_that_passed_admission_before_a_publication_gate_is_rechecked_after_snapshot_wait()
{
    let mut fixture = Fixture::new().await;
    let held = fixture.context().store.lock_snapshot().await;
    let context = fixture.context().clone();
    let caller = tokio::spawn(async move { context.audio_metadata("note").await });
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while fixture.context().media_readers.available_permits() == 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let ticket = fixture
        .context()
        .recording_publications
        .begin("note", "concurrent-publication")
        .unwrap();
    assert!(!caller.is_finished());
    drop(held);
    let failure = caller.await.unwrap().unwrap_err();
    assert_eq!(failure.code, ErrorCode::Unavailable);
    ticket.resolved();
    fixture.stop().await;
}

#[tokio::test]
async fn a_lost_caller_and_owner_shutdown_keep_ownership_until_the_preintent_attempt_drains() {
    let mut fixture = Fixture::new().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let context = fixture.context().clone();
    let capture = fixture.input.identity.capture_id.clone();
    let expected = fixture.seal.clone();
    let caller = tokio::spawn({
        let calls = calls.clone();
        let entered = entered.clone();
        let release = release.clone();
        async move {
            context
                .publish_capture_authorized("member".into(), capture, expected, move || {
                    let final_check = calls.fetch_add(1, Ordering::SeqCst) != 0;
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        if final_check {
                            entered.notify_one();
                            release.notified().await;
                        }
                        Ok(())
                    }
                })
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    assert_eq!(
        fixture
            .context()
            .capture_ingestion
            .get()
            .unwrap()
            .operations
            .available_permits(),
        1
    );
    assert_eq!(
        fixture
            .context()
            .audio_metadata("note")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    assert_eq!(
        fixture
            .context()
            .delete_recording_authorized(
                "note",
                hypr_loofah_protocol::DeleteRecordingRequest {
                    expected_sha256: "0".repeat(64)
                },
                || async { Ok(()) },
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    assert!(fixture.context().read_note("other").await.is_ok());
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    fixture.stop().await;
    let contender = std::fs::File::options()
        .read(true)
        .write(true)
        .open(fixture.vault.join(".loofah-vault.lock"))
        .unwrap();
    assert!(contender.try_lock().is_err());
    release.notify_one();
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            match contender.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => tokio::task::yield_now().await,
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .unwrap();
    let spool = CaptureSpool::open(
        fixture.operations.join("captures"),
        super::super::CaptureSpoolLimits::default(),
    )
    .await
    .unwrap();
    let recorded = spool
        .resume(fixture.input.identity.clone())
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert_eq!(recorded.seal, Some(fixture.seal));
    assert!(recorded.publication.is_none());
    assert!(!fixture.vault.join("sessions/note/audio.wav").exists());
}

#[tokio::test]
async fn dropping_a_network_waiter_does_not_cancel_owned_publication_or_its_capture_observation() {
    let mut fixture = Fixture::new().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let context = fixture.context().clone();
    let capture = fixture.input.identity.capture_id.clone();
    let seal = fixture.seal.clone();
    let caller = tokio::spawn({
        let entered = entered.clone();
        let release = release.clone();
        async move {
            context
                .publish_capture_authorized("member".into(), capture, seal, move || {
                    let final_check = calls.fetch_add(1, Ordering::SeqCst) != 0;
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        if final_check {
                            entered.notify_one();
                            release.notified().await;
                        }
                        Ok(())
                    }
                })
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    release.notify_one();
    let writer = fixture.writer().await;
    let publication = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut retry = tokio::time::interval(std::time::Duration::from_millis(5));
        loop {
            retry.tick().await;
            if let Some(publication) = writer.snapshot().await.unwrap().publication {
                break publication;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(publication.source, fixture.seal);
    assert_eq!(
        hash(&std::fs::read(fixture.vault.join("sessions/note/audio.wav")).unwrap()),
        publication.canonical_audio_sha256
    );
    assert!(
        fixture
            .context()
            .ensure_recording_publication_ready("note")
            .is_ok()
    );
    drop(writer);
    fixture.stop().await;
}

#[tokio::test]
async fn a_recording_created_after_hashing_is_preserved_and_definite_no_intent_releases_only_its_gate()
 {
    let mut fixture = Fixture::new().await;
    let directory = fixture.vault.join("sessions/note");
    let path = directory.join("audio.wav");
    let calls = Arc::new(AtomicUsize::new(0));
    let counting = calls.clone();
    let replacement = path.clone();
    let failure = fixture
        .context()
        .publish_capture_authorized(
            "member".into(),
            fixture.input.identity.capture_id.clone(),
            fixture.seal.clone(),
            move || {
                let final_check = counting.fetch_add(1, Ordering::SeqCst) != 0;
                let replacement = replacement.clone();
                async move {
                    if final_check {
                        std::fs::write(replacement, "Newer user recording").unwrap();
                    }
                    Ok(())
                }
            },
        )
        .await
        .unwrap_err();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(failure.code, ErrorCode::Conflict);
    assert_eq!(std::fs::read(path).unwrap(), b"Newer user recording");
    assert!(
        fixture
            .context()
            .ensure_recording_publication_ready("note")
            .is_ok()
    );
    assert!(
        fixture
            .writer()
            .await
            .snapshot()
            .await
            .unwrap()
            .publication
            .is_none()
    );
    fixture.stop().await;
}

#[tokio::test]
async fn unsafe_publication_inventory_blocks_recording_delivery_but_keeps_text_notes_usable() {
    let mut fixture = Fixture::new().await;
    fixture.stop().await;
    std::fs::write(
        fixture.operations.join("recording-publications"),
        "Unsafe control-plane root",
    )
    .unwrap();
    let runtime = VaultRuntime::open(
        "vault".into(),
        &fixture.vault,
        &fixture._root.path().join("cache"),
        &fixture.operations,
    )
    .await
    .unwrap();
    assert_eq!(
        runtime
            .context
            .audio_metadata("note")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    assert_eq!(
        runtime
            .context
            .audio_metadata("other")
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unavailable
    );
    assert!(runtime.context.read_note("note").await.is_ok());
    assert!(runtime.context.read_note("other").await.is_ok());
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn bootstrap_finishes_admitted_publication_before_readiness_without_reencoding_or_touching_notes()
 {
    let mut fixture = Fixture::new().await;
    let published = fixture.publish().await.unwrap();
    let directory = fixture.vault.join("sessions/note");
    let note = std::fs::read(directory.join("notes.md")).unwrap();
    fixture.stop().await;
    let receipts = fixture.operations.join("recording-publications");
    let path = std::fs::read_dir(&receipts)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap();
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let stage = receipt["stage_name"].as_str().unwrap().to_owned();
    receipt["state"] = serde_json::json!("pending");
    std::fs::rename(directory.join("audio.wav"), directory.join(stage)).unwrap();
    std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    // Metadata already equals the admitted after-image, which recovery also accepts.
    let runtime = VaultRuntime::open(
        "vault".into(),
        &fixture.vault,
        &fixture._root.path().join("cache"),
        &fixture.operations,
    )
    .await
    .unwrap();
    let metadata = runtime
        .context
        .audio_metadata("note")
        .await
        .unwrap()
        .unwrap();
    let publication = published.publication.unwrap();
    assert_eq!(metadata.sha256, publication.canonical_audio_sha256);
    assert_eq!(metadata.size_bytes, publication.canonical_audio_bytes);
    assert_eq!(std::fs::read(directory.join("notes.md")).unwrap(), note);
    let receipt: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(receipt["state"], "committed");
    runtime.shutdown().await.unwrap();
}

#[tokio::test]
async fn dropped_bootstrap_waiter_retains_the_real_vault_lease_through_admitted_recovery_io() {
    let mut fixture = Fixture::new().await;
    fixture.publish().await.unwrap();
    let directory = fixture.vault.join("sessions/note");
    let operations = fixture.operations.join("recording-publications");
    let path = std::fs::read_dir(&operations)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.extension().is_some_and(|ext| ext == "json"))
        .unwrap();
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let stage = receipt["stage_name"].as_str().unwrap().to_owned();
    receipt["state"] = serde_json::json!("pending");
    std::fs::rename(directory.join("audio.wav"), directory.join(stage)).unwrap();
    std::fs::write(&path, serde_json::to_vec(&receipt).unwrap()).unwrap();
    let store = fixture.context().store.clone();
    let held = store.lock_snapshot().await;
    let lease = Arc::get_mut(fixture.context.as_mut().unwrap())
        .unwrap()
        .lease
        .take()
        .unwrap();
    let mut bootstrap = Box::pin(crate::VaultContext::bootstrap_recording_publications(
        store,
        "vault".into(),
        operations,
        lease,
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), &mut bootstrap)
            .await
            .is_err()
    );
    drop(bootstrap);
    let contender = std::fs::File::options()
        .read(true)
        .write(true)
        .open(fixture.vault.join(".loofah-vault.lock"))
        .unwrap();
    assert!(contender.try_lock().is_err());
    drop(held);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut retry = tokio::time::interval(std::time::Duration::from_millis(5));
        loop {
            retry.tick().await;
            match contender.try_lock() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {}
                Err(error) => panic!("{error}"),
            }
        }
    })
    .await
    .unwrap();
    let receipt: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    assert_eq!(receipt["state"], "committed");
    assert!(directory.join("audio.wav").is_file());
    Arc::get_mut(fixture.context.as_mut().unwrap())
        .unwrap()
        .lease = Some(contender);
    fixture.stop().await;
}
