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
        lease.into(),
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

    async fn pending(&mut self) -> (uuid::Uuid, String) {
        let request_id = uuid::Uuid::new_v4();
        let mut input = self.input.clone();
        input.identity.capture_id = uuid::Uuid::new_v4().to_string();
        input.identity.request_id = uuid::Uuid::new_v4().to_string();
        input.identity.target = CaptureTarget::PendingSession {
            create_request_id: request_id.to_string(),
        };
        let mut status = self
            .context()
            .begin_capture_authorized("member".into(), input.clone(), || async { Ok(()) })
            .await
            .unwrap();
        for (sequence, bytes) in self.bytes.chunks(32).enumerate() {
            let mut descriptor = CaptureChunkDescriptor {
                capture_fingerprint: status.prefix.capture_fingerprint.clone(),
                sequence: sequence as u64,
                first_frame: status.prefix.frames,
                frames: (bytes.len() / 8) as u32,
                pcm_offset: status.prefix.pcm_bytes,
                bytes: bytes.len() as u32,
                sha256: hash(bytes),
                previous_digest: status.prefix.chain_digest.clone(),
                digest: String::new(),
            };
            descriptor.digest = hypr_loofah_protocol::capture_chunk_digest(&descriptor);
            status.prefix = self
                .context()
                .append_capture_authorized(
                    "member".into(),
                    input.identity.capture_id.clone(),
                    descriptor,
                    bytes.to_vec(),
                    || async { Ok(()) },
                )
                .await
                .unwrap();
        }
        let seal = CaptureSeal {
            capture_fingerprint: status.prefix.capture_fingerprint,
            chunk_count: status.prefix.chunk_count,
            frames: status.prefix.frames,
            pcm_bytes: status.prefix.pcm_bytes,
            chain_digest: status.prefix.chain_digest,
            source_sha256: hash(&self.bytes),
        };
        self.context()
            .seal_capture_authorized(
                "member".into(),
                input.identity.capture_id.clone(),
                seal.clone(),
                || async { Ok(()) },
            )
            .await
            .unwrap();
        self.input = input;
        self.seal = seal;
        (
            request_id,
            crate::operations::creation_identity("member", request_id).1,
        )
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
    for kind in ["audio", "transcript", "missing", "seal", "append"] {
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
            "append" => {
                let mut input = fixture.input.clone();
                input.identity.capture_id = uuid::Uuid::new_v4().to_string();
                input.identity.request_id = uuid::Uuid::new_v4().to_string();
                input.identity.target = CaptureTarget::ExistingSession {
                    session_id: "note".into(),
                    expected_audio_sha256: Some("0".repeat(64)),
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
        .lease = Some(contender.into());
    fixture.stop().await;
}

#[tokio::test]
async fn pending_capture_creates_one_online_note_at_producer_time_without_retargeting_raw_identity()
{
    let mut fixture = Fixture::new().await;
    let (_, session) = fixture.pending().await;
    assert!(!fixture.vault.join("sessions").join(&session).exists());
    let identity = fixture.input.identity.clone();
    let original = fixture.writer().await.snapshot().await.unwrap();
    assert!(original.session_id.is_none());
    let published = fixture.publish().await.unwrap();
    assert_eq!(published.identity, identity);
    assert_eq!(published.session_id.as_deref(), Some(session.as_str()));
    let meta = fixture
        .context()
        .read_metadata(&session)
        .await
        .unwrap()
        .meta;
    assert_eq!(
        chrono::DateTime::parse_from_rfc3339(&meta.created_at)
            .unwrap()
            .timestamp_millis() as u64,
        fixture.input.first_admitted_audio_observed_at_ms
    );
    assert_eq!(meta.started_at.as_deref(), Some(meta.created_at.as_str()));
    let publication = published.publication.unwrap();
    assert_eq!(publication.session_id, session);
    assert_eq!(publication.source, fixture.seal);
    let audio = fixture
        .vault
        .join("sessions")
        .join(&session)
        .join("audio.wav");
    let bytes = std::fs::read(&audio).unwrap();
    assert_eq!(hash(&bytes), publication.canonical_audio_sha256);
    assert_eq!(
        hound::WavReader::new(std::io::Cursor::new(bytes.clone()))
            .unwrap()
            .samples::<f32>()
            .flat_map(|sample| sample.unwrap().to_le_bytes())
            .collect::<Vec<_>>(),
        fixture.bytes
    );
    let modified = std::fs::metadata(&audio).unwrap().modified().unwrap();
    let replay = fixture.publish().await.unwrap();
    assert_eq!(replay.session_id.as_deref(), Some(session.as_str()));
    assert_eq!(std::fs::read(&audio).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(audio).unwrap().modified().unwrap(),
        modified
    );
    let raw = fixture.writer().await.snapshot().await.unwrap();
    assert_eq!(raw.identity, identity);
    assert_eq!(raw.capture_fingerprint, original.capture_fingerprint);
    assert_eq!(raw.seal, original.seal);
    fixture.stop().await;
}

#[tokio::test]
async fn pending_creation_and_binding_recover_after_restart_without_recreating_or_overwriting_note()
{
    for bound in [false, true] {
        let mut fixture = Fixture::new().await;
        let (request, session) = fixture.pending().await;
        let created = fixture
            .context()
            .create_capture_session(
                "member".into(),
                request,
                fixture.input.first_admitted_audio_observed_at_ms,
            )
            .await
            .unwrap();
        assert_eq!(created.meta.id, session);
        if bound {
            fixture
                .writer()
                .await
                .bind_session(session.clone())
                .await
                .unwrap();
        }
        let directory = fixture.vault.join("sessions").join(&session);
        let metadata = directory.join("_meta.json");
        let mut raw: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&metadata).unwrap()).unwrap();
        raw["title"] = serde_json::json!("User renamed pending capture");
        raw["future"] = serde_json::json!({"preserve":true});
        std::fs::write(&metadata, serde_json::to_vec_pretty(&raw).unwrap()).unwrap();
        std::fs::write(directory.join("unknown-user.txt"), b"keep user attachment").unwrap();
        let note = std::fs::read(directory.join("notes.md")).unwrap();
        fixture.stop().await;
        fixture.context = Some(leased_context(&fixture.vault, &fixture.operations).await);
        let result = fixture.publish().await.unwrap();
        assert_eq!(result.session_id.as_deref(), Some(session.as_str()));
        assert_eq!(result.identity, fixture.input.identity);
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(metadata).unwrap()).unwrap();
        assert_eq!(saved["title"], raw["title"]);
        assert_eq!(saved["future"], raw["future"]);
        assert_eq!(saved["created_at"], raw["created_at"]);
        assert_eq!(std::fs::read(directory.join("notes.md")).unwrap(), note);
        assert_eq!(
            std::fs::read(directory.join("unknown-user.txt")).unwrap(),
            b"keep user attachment"
        );
        let mut ids = std::fs::read_dir(fixture.vault.join("sessions"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        ids.sort();
        let mut expected = vec!["note".to_owned(), "other".to_owned(), session];
        expected.sort();
        assert_eq!(ids, expected);
        fixture.stop().await;
    }
}

#[tokio::test]
async fn pending_bound_mismatch_and_creation_request_collision_fail_without_new_note_or_recording()
{
    for collision in [false, true] {
        let mut fixture = Fixture::new().await;
        let (request, session) = fixture.pending().await;
        if collision {
            fixture
                .context()
                .create_session(
                    "member".into(),
                    request.to_string(),
                    hypr_loofah_protocol::CreateSessionRequest {
                        title: "Previously accepted note".into(),
                        markdown: "Keep prior request data".into(),
                    },
                )
                .await
                .unwrap();
        } else {
            fixture
                .writer()
                .await
                .bind_session("other".into())
                .await
                .unwrap();
        }
        let before = fixture
            .context()
            .read_note(if collision { &session } else { "other" })
            .await
            .unwrap();
        let error = fixture.publish().await.unwrap_err();
        assert_eq!(error.code, ErrorCode::Conflict);
        if !collision {
            assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
            assert!(!fixture.vault.join("sessions").join(&session).exists());
            let (key, _) = crate::operations::creation_identity("member", request);
            assert!(!fixture.operations.join(format!("{key}.json")).exists());
        }
        assert_eq!(
            fixture
                .context()
                .read_note(if collision { &session } else { "other" })
                .await
                .unwrap(),
            before
        );
        assert!(
            !fixture
                .vault
                .join("sessions")
                .join(&session)
                .join("audio.wav")
                .exists()
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
}

#[tokio::test]
async fn pending_commit_denial_reports_partial_persistence_and_retry_preserves_created_note() {
    let mut fixture = Fixture::new().await;
    let (_, session) = fixture.pending().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    let error = fixture
        .context()
        .publish_capture_authorized(
            "member".into(),
            fixture.input.identity.capture_id.clone(),
            fixture.seal.clone(),
            move || {
                let call = counted.fetch_add(1, Ordering::SeqCst);
                async move {
                    if call == 2 {
                        Err::<(), _>(rejected(
                            ErrorCode::Forbidden,
                            "Membership removed before publication",
                        ))
                    } else {
                        Ok(())
                    }
                }
            },
        )
        .await
        .unwrap_err();
    assert_eq!(calls.load(Ordering::SeqCst), 3);
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::Persisted));
    assert!(
        fixture
            .vault
            .join("sessions")
            .join(&session)
            .join("_meta.json")
            .exists()
    );
    assert!(
        !fixture
            .vault
            .join("sessions")
            .join(&session)
            .join("audio.wav")
            .exists()
    );
    assert_eq!(
        fixture
            .writer()
            .await
            .snapshot()
            .await
            .unwrap()
            .session_id
            .as_deref(),
        Some(session.as_str())
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
    let result = fixture.publish().await.unwrap();
    assert_eq!(result.publication.unwrap().session_id, session);
    assert_eq!(
        fixture.writer().await.snapshot().await.unwrap().identity,
        fixture.input.identity
    );
    fixture.stop().await;
}

#[tokio::test]
async fn pending_creation_requires_nonzero_sealed_audio_and_fresh_creation_authority() {
    for kind in ["zero", "changed_seal", "denied_creation"] {
        let mut fixture = Fixture::new().await;
        let (request, session) = fixture.pending().await;
        if kind == "zero" {
            let mut input = fixture.input.clone();
            input.identity.capture_id = uuid::Uuid::new_v4().to_string();
            input.identity.request_id = uuid::Uuid::new_v4().to_string();
            fixture
                .context()
                .begin_capture_authorized("member".into(), input.clone(), || async { Ok(()) })
                .await
                .unwrap();
            fixture.input = input;
            fixture.seal = fixture.writer().await.seal().await.unwrap();
        }
        if kind == "changed_seal" {
            fixture.seal.source_sha256 = "0".repeat(64);
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        let denied = kind == "denied_creation";
        assert!(
            fixture
                .context()
                .publish_capture_authorized(
                    "member".into(),
                    fixture.input.identity.capture_id.clone(),
                    fixture.seal.clone(),
                    move || {
                        let call = counted.fetch_add(1, Ordering::SeqCst);
                        async move {
                            if denied && call == 1 {
                                Err::<(), _>(rejected(ErrorCode::Forbidden, "Creation denied"))
                            } else {
                                Ok(())
                            }
                        }
                    }
                )
                .await
                .is_err()
        );
        assert!(!fixture.vault.join("sessions").join(&session).exists());
        let (key, _) = crate::operations::creation_identity("member", request);
        assert!(!fixture.operations.join(format!("{key}.json")).exists());
        assert!(
            fixture
                .writer()
                .await
                .snapshot()
                .await
                .unwrap()
                .session_id
                .is_none()
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn pending_completed_creation_and_publication_never_resurrect_a_deleted_note() {
    for published in [false, true] {
        let mut fixture = Fixture::new().await;
        let (request, session) = fixture.pending().await;
        if published {
            fixture.publish().await.unwrap();
        } else {
            fixture
                .context()
                .create_capture_session(
                    "member".into(),
                    request,
                    fixture.input.first_admitted_audio_observed_at_ms,
                )
                .await
                .unwrap();
        }
        let directory = fixture.vault.join("sessions").join(&session);
        let deleted = fixture.vault.join("user-removed-session");
        std::fs::rename(&directory, &deleted).unwrap();
        let prior = std::fs::read(deleted.join("notes.md")).unwrap();
        fixture.stop().await;
        fixture.context = Some(leased_context(&fixture.vault, &fixture.operations).await);
        assert_eq!(
            fixture.publish().await.unwrap_err().code,
            ErrorCode::NotFound
        );
        assert!(!directory.exists());
        assert_eq!(std::fs::read(deleted.join("notes.md")).unwrap(), prior);
        assert_eq!(
            fixture.writer().await.read_chunk(0).await.unwrap().bytes,
            fixture.bytes[..32]
        );
        fixture.stop().await;
    }
}

#[tokio::test]
async fn dropping_pending_creation_waiter_keeps_owned_creation_binding_and_publication_alive() {
    let mut fixture = Fixture::new().await;
    let (_, session) = fixture.pending().await;
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
                    let creation = calls.fetch_add(1, Ordering::SeqCst) == 1;
                    let entered = entered.clone();
                    let release = release.clone();
                    async move {
                        if creation {
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
        loop {
            if let Some(publication) = writer.snapshot().await.unwrap().publication {
                break publication;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(publication.session_id, session);
    assert_eq!(
        writer.snapshot().await.unwrap().identity,
        fixture.input.identity
    );
    assert_eq!(
        hash(
            &std::fs::read(
                fixture
                    .vault
                    .join("sessions")
                    .join(&session)
                    .join("audio.wav")
            )
            .unwrap()
        ),
        publication.canonical_audio_sha256
    );
    drop(writer);
    fixture.stop().await;
}

#[tokio::test]
async fn lifecycle_inventory_covers_other_members_pending_future_targets_and_original_bindings() {
    let mut fixture = Fixture::new().await;
    let (_, future) = fixture.pending().await;
    fixture
        .writer()
        .await
        .bind_session("other".into())
        .await
        .unwrap();
    let mut foreign = fixture.input.clone();
    foreign.identity.scope.member_id = "another-member".into();
    foreign.identity.capture_id = uuid::Uuid::new_v4().to_string();
    foreign.identity.request_id = uuid::Uuid::new_v4().to_string();
    foreign.identity.target = CaptureTarget::ExistingSession {
        session_id: "foreign-member-target".into(),
        expected_audio_sha256: None,
    };
    fixture
        .context()
        .begin_capture_authorized("another-member".into(), foreign, || async { Ok(()) })
        .await
        .unwrap();
    fixture.stop().await;
    fixture.context = Some(leased_context(&fixture.vault, &fixture.operations).await);
    for session in ["note", "other", future.as_str(), "foreign-member-target"] {
        let _exclusive = fixture
            .context()
            .try_session_lifecycle_change(session)
            .unwrap();
        let error = fixture
            .context()
            .ensure_capture_session_lifecycle_idle(session)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Unavailable);
        assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    }
    let exclusive = fixture
        .context()
        .try_session_lifecycle_change("unrelated")
        .unwrap();
    fixture
        .context()
        .ensure_capture_session_lifecycle_idle("unrelated")
        .await
        .unwrap();
    drop(exclusive);
    fixture.stop().await;
}

#[tokio::test]
async fn verified_publication_releases_lifecycle_inventory_but_unknown_publication_never_does() {
    let mut fixture = Fixture::new().await;
    fixture.publish().await.unwrap();
    let exclusive = fixture
        .context()
        .try_session_lifecycle_change("note")
        .unwrap();
    fixture
        .context()
        .ensure_capture_session_lifecycle_idle("note")
        .await
        .unwrap();
    fixture.context().recording_publications.block_all();
    assert_eq!(
        fixture
            .context()
            .ensure_capture_session_lifecycle_idle("note")
            .await
            .unwrap_err()
            .mutation_outcome,
        Some(MutationOutcome::NotCommitted)
    );
    assert!(
        fixture
            .context()
            .ensure_capture_session_lifecycle_idle("unrelated")
            .await
            .is_err()
    );
    drop(exclusive);
    fixture.stop().await;
}

#[tokio::test]
async fn unsafe_reopened_capture_inventory_fails_closed_without_touching_raw_files() {
    let mut fixture = Fixture::new().await;
    fixture.stop().await;
    let unsafe_entry = fixture.operations.join("captures").join("A".repeat(64));
    std::fs::write(&unsafe_entry, b"retain unexpected data").unwrap();
    fixture.context = Some(leased_context(&fixture.vault, &fixture.operations).await);
    let exclusive = fixture
        .context()
        .try_session_lifecycle_change("unrelated")
        .unwrap();
    let error = fixture
        .context()
        .ensure_capture_session_lifecycle_idle("unrelated")
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Unavailable);
    assert_eq!(error.mutation_outcome, Some(MutationOutcome::NotCommitted));
    assert_eq!(
        std::fs::read(unsafe_entry).unwrap(),
        b"retain unexpected data"
    );
    drop(exclusive);
    fixture.stop().await;
}

#[tokio::test]
async fn dropped_capture_admission_waiter_cannot_bypass_exclusive_lifecycle_or_lose_durable_start()
{
    let mut fixture = Fixture::new().await;
    let (_, session) = fixture.pending().await;
    let mut input = fixture.input.clone();
    input.identity.capture_id = uuid::Uuid::new_v4().to_string();
    input.identity.request_id = uuid::Uuid::new_v4().to_string();
    let exclusive = fixture
        .context()
        .try_session_lifecycle_change(&session)
        .unwrap();
    let entered = Arc::new(tokio::sync::Notify::new());
    let context = fixture.context().clone();
    let declaration = input.clone();
    let mut caller = tokio::spawn({
        let entered = entered.clone();
        async move {
            context
                .begin_capture_authorized("member".into(), declaration, move || async move {
                    entered.notify_one();
                    Ok(())
                })
                .await
        }
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), entered.notified())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(25), &mut caller)
            .await
            .is_err()
    );
    let spool = fixture
        .context()
        .capture_ingestion
        .get()
        .unwrap()
        .spool
        .get()
        .unwrap()
        .clone();
    assert!(
        !spool
            .request_exists(&input.identity.request_id)
            .await
            .unwrap()
    );
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    drop(exclusive);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if spool
                .request_exists(&input.identity.request_id)
                .await
                .unwrap()
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let snapshot = spool
        .resume(input.identity.clone())
        .await
        .unwrap()
        .snapshot()
        .await
        .unwrap();
    assert_eq!(snapshot.identity, input.identity);
    assert_eq!(
        snapshot.first_admitted_audio_observed_at_ms,
        Some(input.first_admitted_audio_observed_at_ms)
    );
    drop(spool);
    fixture.stop().await;
}

#[tokio::test]
async fn dropped_note_creation_waiter_retains_shared_lifecycle_guard_through_real_commit() {
    let mut fixture = Fixture::new().await;
    let request = uuid::Uuid::new_v4();
    let (key, session) = crate::operations::creation_identity("member", request);
    let held = fixture.context().store.lock_snapshot().await;
    let context = fixture.context().clone();
    let caller = tokio::spawn(async move {
        context
            .create_session(
                "member".into(),
                request.to_string(),
                hypr_loofah_protocol::CreateSessionRequest {
                    title: "Retain owned create".into(),
                    markdown: "Keep accepted content".into(),
                },
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !fixture.operations.join(format!("{key}.json")).exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        fixture
            .context()
            .try_session_lifecycle_change(&session)
            .is_err()
    );
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    assert!(
        fixture
            .context()
            .try_session_lifecycle_change(&session)
            .is_err()
    );
    drop(held);
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Ok(exclusive) = fixture.context().try_session_lifecycle_change(&session) {
                drop(exclusive);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        fixture
            .context()
            .read_note(&session)
            .await
            .unwrap()
            .markdown
            .as_deref(),
        Some("Keep accepted content")
    );
    fixture.stop().await;
}

async fn retirement_request(fixture: &Fixture) -> hypr_loofah_protocol::CaptureRetirementRequest {
    let published = fixture.publish().await.unwrap();
    hypr_loofah_protocol::CaptureRetirementRequest {
        cleanup_request_id: uuid::Uuid::new_v4().to_string(),
        identity: fixture.input.identity.clone(),
        format: fixture.input.format.clone(),
        source: fixture.seal.clone(),
        publication: published.publication.unwrap(),
        processing: None,
    }
}

#[tokio::test]
async fn verified_server_retirement_preserves_canonical_and_original_status_and_never_reopens_raw()
{
    let fixture = Fixture::new().await;
    let request = retirement_request(&fixture).await;
    let canonical = std::fs::read(fixture.vault.join("sessions/note/audio.wav")).unwrap();
    let context = fixture.context();
    let retired = context
        .retire_capture_authorized(
            "member".into(),
            request.identity.capture_id.clone(),
            request.clone(),
            || async { Ok(()) },
        )
        .await
        .unwrap();
    assert_eq!(
        retired.retirement.as_ref().unwrap().state,
        hypr_loofah_protocol::CaptureRetirementState::Retired
    );
    assert_eq!(
        std::fs::read(fixture.vault.join("sessions/note/audio.wav")).unwrap(),
        canonical
    );
    assert_eq!(
        context
            .capture_status_authorized(
                "member".into(),
                request.identity.capture_id.clone(),
                || async { Ok(()) }
            )
            .await
            .unwrap(),
        retired
    );
    assert!(
        context
            .capture_writer("member".into(), request.identity.capture_id.clone())
            .await
            .is_err()
    );
    assert_eq!(
        context
            .retire_capture_authorized(
                "member".into(),
                request.identity.capture_id.clone(),
                request,
                || async { Ok(()) }
            )
            .await
            .unwrap(),
        retired
    );
}

#[tokio::test]
async fn server_retirement_requires_original_source_declaration_scope_and_final_authorization() {
    let fixture = Fixture::new().await;
    let request = retirement_request(&fixture).await;
    let context = fixture.context();
    let initial = fixture.writer().await.read_chunk(0).await.unwrap().bytes;
    for variant in ["member", "publication", "processing"] {
        let mut wrong = request.clone();
        match variant {
            "member" => wrong.identity.scope.member_id = "other".into(),
            "publication" => wrong.publication.canonical_audio_sha256 = "0".repeat(64),
            _ => {
                wrong.processing = Some(hypr_loofah_protocol::CaptureProcessingRequest {
                    capture_fingerprint: wrong.source.capture_fingerprint.clone(),
                    model: None,
                    languages: vec![],
                    keywords: vec![],
                    summary: None,
                })
            }
        }
        assert!(
            context
                .retire_capture_authorized(
                    "member".into(),
                    request.identity.capture_id.clone(),
                    wrong,
                    || async { Ok(()) }
                )
                .await
                .is_err()
        );
        assert_eq!(
            fixture.writer().await.read_chunk(0).await.unwrap().bytes,
            initial
        );
    }
    let authorizations = Arc::new(AtomicUsize::new(0));
    let error = context
        .retire_capture_authorized(
            "member".into(),
            request.identity.capture_id.clone(),
            request.clone(),
            {
                let calls = authorizations.clone();
                move || {
                    let allowed = calls.fetch_add(1, Ordering::SeqCst) == 0;
                    async move {
                        if allowed {
                            Ok(())
                        } else {
                            Err(rejected(ErrorCode::Forbidden, "Access revoked"))
                        }
                    }
                }
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::Forbidden);
    assert_eq!(authorizations.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.writer().await.read_chunk(0).await.unwrap().bytes,
        initial
    );
    let lifetime = context.admit_session_work("note").await.unwrap();
    assert!(
        context
            .retire_capture_authorized(
                "member".into(),
                request.identity.capture_id.clone(),
                request.clone(),
                || async { Ok(()) }
            )
            .await
            .is_err()
    );
    drop(lifetime);
    std::fs::write(
        fixture.vault.join("sessions/note/audio.wav"),
        b"newer canonical recording",
    )
    .unwrap();
    assert!(
        context
            .retire_capture_authorized(
                "member".into(),
                request.identity.capture_id.clone(),
                request,
                || async { Ok(()) }
            )
            .await
            .is_err()
    );
    assert_eq!(
        fixture.writer().await.read_chunk(0).await.unwrap().bytes,
        initial
    );
}
